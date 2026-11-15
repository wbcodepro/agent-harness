//! The routes the web interface uses beyond running a turn: the session
//! list, settings, resuming the event stream, steering, uploads and the
//! interface itself.

mod common;

use common::{start, tool_call, write_call};
use hame_eval::ScriptedResponse;
use hame_session::{Delta, StopReason, Usage};
use serde_json::{Value, json};

fn turn_ended(event: &Value) -> bool {
    event["kind"] == "turn" && event["state"] == "ended"
}

/// Reads until a turn has started and ended (a new subscription first
/// reports the current state, which may be "ended" from before).
async fn through_turn(events: &mut common::Events) -> Vec<Value> {
    let mut seen = events
        .until("the turn to start", |event| {
            event["kind"] == "turn" && event["state"] == "started"
        })
        .await;
    seen.extend(events.until("the end of the turn", turn_ended).await);
    seen
}

fn slow_text(words: usize) -> ScriptedResponse {
    ScriptedResponse {
        deltas: (0..words)
            .map(|n| Delta::Text {
                text: format!("w{n} "),
            })
            .collect(),
        stop: StopReason::EndTurn,
        usage: Usage::default(),
    }
}

fn find<'a>(list: &'a Value, id: &str) -> &'a Value {
    list.as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == id)
        .unwrap_or_else(|| panic!("{id} is not in {list}"))
}

#[tokio::test]
async fn sessions_are_listed_with_titles_and_can_be_renamed_pinned_and_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let page = start(dir.path(), vec![ScriptedResponse::text("Looking.")], 0).await;
    let other = dir.path().join("other");
    std::fs::create_dir_all(&other).unwrap();

    let (status, info) = page.get("/info").await;
    assert_eq!(status, 200);
    assert!(info["cwd"].as_str().unwrap().ends_with("work"));
    assert_eq!(info["modes"].as_array().unwrap().len(), 5);
    assert_eq!(info["modes"][2]["name"], "ask");
    assert_eq!(info["models"]["fixed"], true);
    assert_eq!(info["limits"]["upload_bytes"], 20 * 1024 * 1024);

    let first = page.session(json!({})).await;
    let second = page
        .session(json!({"cwd": other.to_string_lossy(), "mode": "ask"}))
        .await;
    for (body, why) in [
        (json!({"cwd": "relative/path"}), "a relative folder"),
        (
            json!({"cwd": dir.path().join("missing").to_string_lossy()}),
            "a missing folder",
        ),
        (json!({"mode": "yolo"}), "an unknown mode"),
        (json!({"provider": "nobody"}), "an unknown provider"),
        (json!({"colour": "red"}), "an unknown field"),
    ] {
        assert_eq!(
            page.call("POST", "/sessions", Some(body)).await.0,
            400,
            "{why}"
        );
    }

    let mut events = page.events(&first).await;
    page.post(
        &format!("/sessions/{first}/prompt"),
        json!({"text": "Fix the login bug\nIt fails on empty passwords."}),
    )
    .await;
    through_turn(&mut events).await;

    let (_, list) = page.get("/sessions").await;
    assert_eq!(list[0]["id"], first.as_str(), "latest activity first");
    let row = find(&list, &first);
    assert_eq!(row["title"], "Fix the login bug");
    assert_eq!(row["named"], false);
    assert_eq!(row["status"], "idle");
    assert_eq!(row["turns"], 1);
    assert!(row["last_activity"].as_u64().unwrap() >= row["created"].as_u64().unwrap());
    let row = find(&list, &second);
    assert_eq!(row["title"], Value::Null);
    assert_eq!(row["mode"], "ask");
    assert!(row["cwd"].as_str().unwrap().ends_with("other"));

    let (status, renamed) = page
        .call(
            "PATCH",
            &format!("/sessions/{first}"),
            Some(json!({"title": "  Login  ", "pinned": true})),
        )
        .await;
    assert_eq!(status, 200, "{renamed}");
    assert_eq!(renamed["title"], "Login");
    assert_eq!(renamed["pinned"], true);
    let (_, list) = page.get("/sessions").await;
    assert_eq!(find(&list, &first)["named"], true);
    // Pinning alone keeps the name; `null` goes back to the first message.
    let (_, kept) = page
        .call(
            "PATCH",
            &format!("/sessions/{first}"),
            Some(json!({"pinned": false})),
        )
        .await;
    assert_eq!(kept["title"], "Login");
    let (_, reverted) = page
        .call(
            "PATCH",
            &format!("/sessions/{first}"),
            Some(json!({"title": null})),
        )
        .await;
    assert_eq!(reverted["title"], "Fix the login bug");
    assert_eq!(
        page.call(
            "PATCH",
            &format!("/sessions/{first}"),
            Some(json!({"title": "x".repeat(201)}))
        )
        .await
        .0,
        400
    );
    assert_eq!(
        page.call("PATCH", "/sessions/nosuch", Some(json!({"title": "x"})))
            .await
            .0,
        404
    );

    let (status, detail) = page.get(&format!("/sessions/{first}")).await;
    assert_eq!(status, 200);
    assert_eq!(detail["busy"], false);
    assert_eq!(detail["mode"], "default");
    assert_eq!(
        detail["cost_usd"],
        Value::Null,
        "an unknown price is not zero"
    );
    assert!(detail["context"]["tokens"].as_u64().unwrap() > 0);
    assert_eq!(detail["context"]["compact_at"], 100_000);
    assert_eq!(detail["queued"], json!([]));
    assert_eq!(detail["approvals"], json!([]));
    assert_eq!(page.get("/sessions/nosuch").await.0, 404);

    let (status, _) = page
        .call("DELETE", &format!("/sessions/{second}"), None)
        .await;
    assert_eq!(status, 200);
    let (_, list) = page.get("/sessions").await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(
        page.call("DELETE", &format!("/sessions/{second}"), None)
            .await
            .0,
        404
    );
    assert_eq!(page.call("DELETE", "/sessions/..%2Fx", None).await.0, 400);
    assert!(other.exists(), "deleting a session leaves its folder alone");
}

#[tokio::test]
async fn the_event_stream_resumes_after_the_last_record_seen() {
    let dir = tempfile::tempdir().unwrap();
    let page = start(dir.path(), vec![ScriptedResponse::text("One two.")], 0).await;
    let session = page.session(json!({})).await;
    let mut events = page.events(&session).await;
    page.post(
        &format!("/sessions/{session}/prompt"),
        json!({"text": "hi"}),
    )
    .await;
    let seen = through_turn(&mut events).await;
    // Records carry their seq as the event id; other events carry none.
    for (event, id) in seen.iter().zip(&events.ids) {
        assert_eq!(event["record"]["seq"].as_u64(), *id, "{event}");
    }
    let seqs: Vec<u64> = seen
        .iter()
        .filter_map(|event| event["record"]["seq"].as_u64())
        .collect();
    let middle = seqs[seqs.len() / 2];
    let last = *seqs.last().unwrap();

    for mut resumed in [
        page.events_with(&session, &format!("&after={middle}"), None)
            .await,
        // What a browser sends when it reconnects by itself.
        page.events_with(&session, "", Some(middle)).await,
    ] {
        let replayed = resumed
            .until("the last record", |event| event["record"]["seq"] == last)
            .await;
        let replayed: Vec<u64> = replayed
            .iter()
            .filter_map(|event| event["record"]["seq"].as_u64())
            .collect();
        assert_eq!(replayed, (middle + 1..=last).collect::<Vec<_>>());
    }
}

#[tokio::test]
async fn a_question_reaches_a_page_that_connects_later_and_always_allows_the_tool() {
    let dir = tempfile::tempdir().unwrap();
    let page = start(
        dir.path(),
        vec![
            write_call("a.txt"),
            tool_call("c2", "write", json!({"path": "b.txt", "content": "two"})),
            ScriptedResponse::text("Both written."),
        ],
        0,
    )
    .await;
    let session = page.session(json!({})).await;
    let mut events = page.events(&session).await;
    page.post(
        &format!("/sessions/{session}/prompt"),
        json!({"text": "write two files"}),
    )
    .await;
    let asked = events
        .until("the question", |event| event["kind"] == "approval")
        .await;
    let question = asked.last().unwrap().clone();
    assert_eq!(question["tool"], "write");
    assert_eq!(question["tool_call_id"], "c1");
    assert_eq!(question["arguments"]["path"], "a.txt");
    assert_eq!(question["effect"], "write");
    assert!(question["scope"].as_str().unwrap().starts_with("write"));

    // A page opened now is asked too, and the list says the session needs
    // someone.
    let mut later = page.events(&session).await;
    let replayed = later
        .until("the waiting questions", |event| {
            event["kind"] == "approvals"
        })
        .await;
    assert_eq!(replayed.last().unwrap()["pending"][0]["id"], question["id"]);
    let (_, list) = page.get("/sessions").await;
    assert_eq!(find(&list, &session)["status"], "needs_you");
    let (_, detail) = page.get(&format!("/sessions/{session}")).await;
    assert_eq!(detail["approvals"][0]["id"], question["id"]);

    let (status, _) = page
        .post(
            &format!("/sessions/{session}/approval"),
            json!({"id": question["id"], "allow": true, "always": true,
                   "tool_call_id": question["tool_call_id"],
                   "args_sha256": question["args_sha256"]}),
        )
        .await;
    assert_eq!(status, 200);
    let rest = events.until("the end of the turn", turn_ended).await;
    assert!(
        rest.iter()
            .any(|event| event["kind"] == "approval_resolved" && event["id"] == question["id"])
    );
    assert!(
        !rest.iter().any(|event| event["kind"] == "approval"),
        "the second write is not asked about"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("work/b.txt")).unwrap(),
        "two"
    );
}

#[tokio::test]
async fn messages_sent_while_a_turn_runs_are_queued_and_can_be_withdrawn() {
    let dir = tempfile::tempdir().unwrap();
    let page = start(
        dir.path(),
        vec![slow_text(300), ScriptedResponse::text("Got your note.")],
        5,
    )
    .await;
    let session = page.session(json!({})).await;
    let steer = async |text: &str| {
        page.post(&format!("/sessions/{session}/steer"), json!({"text": text}))
            .await
    };
    assert_eq!(steer("too early").await.0, 409, "no turn is running");

    let mut events = page.events(&session).await;
    page.post(
        &format!("/sessions/{session}/prompt"),
        json!({"text": "talk"}),
    )
    .await;
    events
        .until("streamed text", |event| {
            event["record"]["type"] == "assistant/delta"
        })
        .await;
    assert_eq!(steer("first").await.0, 200);
    let (status, body) = steer("second").await;
    assert_eq!(status, 200);
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["queued"],
        json!(["first", "second"])
    );
    assert_eq!(steer("  ").await.0, 400);
    let withdraw = async |index: usize, text: &str| {
        page.post(
            &format!("/sessions/{session}/steer/withdraw"),
            json!({"index": index, "text": text}),
        )
        .await
    };
    assert_eq!(
        withdraw(0, "second").await.0,
        409,
        "the index names another message"
    );
    assert_eq!(withdraw(1, "second").await.0, 200);
    let (_, detail) = page.get(&format!("/sessions/{session}")).await;
    assert_eq!(detail["queued"], json!(["first"]));
    assert_eq!(detail["busy"], true);
    let (_, list) = page.get("/sessions").await;
    assert_eq!(find(&list, &session)["status"], "running");
    // A running session cannot be deleted or reconfigured.
    assert_eq!(
        page.call("DELETE", &format!("/sessions/{session}"), None)
            .await
            .0,
        409
    );
    assert_eq!(
        page.call(
            "PUT",
            &format!("/sessions/{session}/settings"),
            Some(json!({"mode": "ask"}))
        )
        .await
        .0,
        409
    );

    let rest = events.until("the end of the turn", turn_ended).await;
    assert!(
        rest.iter()
            .any(|event| { event["kind"] == "steering" && event["queued"] == json!(["first"]) })
    );
    let delivered: Vec<&Value> = rest
        .iter()
        .filter(|event| event["record"]["type"] == "user/message")
        .collect();
    assert_eq!(delivered.len(), 1);
    assert_eq!(
        delivered[0]["record"]["data"]["content"][0]["text"],
        "first"
    );
    assert!(
        rest.iter()
            .any(|event| { event["kind"] == "steering" && event["queued"] == json!([]) })
    );
    assert!(rest.iter().any(|event| {
        event["record"]["type"] == "assistant/message"
            && event["record"]["data"]["content"][0]["text"] == "Got your note."
    }));
}

#[tokio::test]
async fn the_permission_mode_changes_between_turns_and_is_remembered() {
    let dir = tempfile::tempdir().unwrap();
    let page = start(
        dir.path(),
        vec![
            write_call("a.txt"),
            ScriptedResponse::text("Written."),
            tool_call("c2", "write", json!({"path": "b.txt", "content": "no"})),
            ScriptedResponse::text("Refused."),
        ],
        0,
    )
    .await;
    let session = page.session(json!({})).await;
    let settings = async |body: Value| {
        page.call("PUT", &format!("/sessions/{session}/settings"), Some(body))
            .await
    };
    assert_eq!(settings(json!({"mode": "yolo"})).await.0, 400);
    assert_eq!(settings(json!({"provider": "nobody"})).await.0, 400);
    assert_eq!(settings(json!({"model": " "})).await.0, 400);
    let (status, changed) = settings(json!({"mode": "accept-edits"})).await;
    assert_eq!(status, 200, "{changed}");
    assert_eq!(changed["mode"], "accept-edits");

    let mut events = page.events(&session).await;
    page.post(
        &format!("/sessions/{session}/prompt"),
        json!({"text": "write"}),
    )
    .await;
    let seen = through_turn(&mut events).await;
    assert!(
        !seen.iter().any(|event| event["kind"] == "approval"),
        "accept-edits lets file edits through"
    );
    assert!(dir.path().join("work/a.txt").exists());

    let (status, changed) = settings(json!({"mode": "read-only"})).await;
    assert_eq!(status, 200, "{changed}");
    assert_eq!(changed["mode"], "read-only");
    page.post(
        &format!("/sessions/{session}/prompt"),
        json!({"text": "write again"}),
    )
    .await;
    let seen = through_turn(&mut events).await;
    assert!(
        !dir.path().join("work/b.txt").exists(),
        "read-only refuses changes"
    );
    assert!(seen.iter().any(|event| {
        event["record"]["type"] == "tool/result" && event["record"]["data"]["is_error"] == true
    }));
    let (_, detail) = page.get(&format!("/sessions/{session}")).await;
    assert_eq!(detail["mode"], "read-only");
    let (_, list) = page.get("/sessions").await;
    assert_eq!(find(&list, &session)["mode"], "read-only");
}

#[tokio::test]
async fn uploads_reach_the_model_as_images_and_files() {
    let dir = tempfile::tempdir().unwrap();
    let page = start(dir.path(), vec![ScriptedResponse::text("Seen.")], 0).await;
    let session = page.session(json!({})).await;
    let upload = |name: &str, mime: &str, bytes: Vec<u8>| {
        let request = page
            .http
            .post(format!(
                "{}/api/sessions/{session}/attachments?name={name}",
                page.base
            ))
            .header("x-hame-token", &page.token)
            .header("content-type", mime)
            .body(bytes);
        async move {
            let response = request.send().await.unwrap();
            let status = response.status().as_u16();
            let text = response.text().await.unwrap();
            (
                status,
                serde_json::from_str::<Value>(&text).unwrap_or(Value::String(text)),
            )
        }
    };
    let png: Vec<u8> = vec![
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0x0d, 0x49, 0x48, 0x44, 0x52, 0,
        0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0,
    ];
    let (status, image) = upload("shot.png", "image/png", png.clone()).await;
    assert_eq!(status, 200, "{image}");
    assert_eq!(image["kind"], "image");
    assert_eq!(image["artifact"]["mime"], "image/png");
    let sha = image["artifact"]["sha256"].as_str().unwrap().to_string();

    let (status, file) = upload("..%2Fnotes.txt", "text/plain", b"hello".to_vec()).await;
    assert_eq!(status, 200, "{file}");
    assert_eq!(file["kind"], "file");
    assert_eq!(file["path"], ".hame/attachments/notes.txt");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("work/.hame/attachments/notes.txt")).unwrap(),
        "hello"
    );
    let (_, again) = upload("notes.txt", "text/plain", b"again".to_vec()).await;
    assert_eq!(
        again["path"], ".hame/attachments/notes-1.txt",
        "nothing is overwritten"
    );

    assert_eq!(
        upload("x.png", "image/png", b"not a png".to_vec()).await.0,
        400
    );
    assert_eq!(upload("empty.txt", "text/plain", Vec::new()).await.0, 400);
    assert_eq!(
        upload(
            "big.bin",
            "application/octet-stream",
            vec![0; 20 * 1024 * 1024 + 1]
        )
        .await
        .0,
        413
    );

    let prompt = async |body: Value| {
        page.post(&format!("/sessions/{session}/prompt"), body)
            .await
    };
    assert_eq!(
        prompt(json!({"text": "look", "images": ["0".repeat(64)]}))
            .await
            .0,
        400,
        "only images uploaded to this session"
    );
    assert_eq!(
        prompt(json!({"text": "look", "files": ["/etc/passwd"]}))
            .await
            .0,
        400
    );
    let mut events = page.events(&session).await;
    let (status, body) = prompt(json!({
        "text": "What is in these?",
        "images": [sha],
        "files": [file["path"]],
    }))
    .await;
    assert_eq!(status, 200, "{body}");
    let seen = through_turn(&mut events).await;
    let message = seen
        .iter()
        .find(|event| event["record"]["type"] == "user/message")
        .unwrap();
    let content = &message["record"]["data"]["content"];
    assert!(
        content[0]["text"]
            .as_str()
            .unwrap()
            .contains(".hame/attachments/notes.txt")
    );
    assert_eq!(content[1]["type"], "image");
    assert_eq!(content[1]["artifact"]["sha256"], sha.as_str());
    let served = page
        .http
        .get(format!(
            "{}/api/artifacts/{sha}?session={session}",
            page.base
        ))
        .header("x-hame-token", &page.token)
        .send()
        .await
        .unwrap();
    assert_eq!(served.status(), 200);
    assert_eq!(served.bytes().await.unwrap().as_ref(), png.as_slice());
}

#[tokio::test]
async fn the_interface_is_served_with_a_strict_policy() {
    let dir = tempfile::tempdir().unwrap();
    let page = start(dir.path(), vec![], 0).await;
    let index = page
        .http
        .get(format!("{}/", page.base))
        .send()
        .await
        .unwrap();
    assert_eq!(index.status(), 200);
    assert_eq!(index.headers()["content-type"], "text/html; charset=utf-8");
    let policy = index.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .to_string();
    assert!(policy.contains("frame-ancestors 'none'"));
    assert!(policy.contains("script-src 'self'"));
    assert_eq!(index.headers()["referrer-policy"], "no-referrer");
    let html = index.text().await.unwrap();
    assert!(html.contains("<title>hame</title>"));
    let script = html
        .split('"')
        .find(|part| part.starts_with("/assets/") && part.ends_with(".js"))
        .expect("the page loads a hashed script");
    let asset = page
        .http
        .get(format!("{}{script}", page.base))
        .send()
        .await
        .unwrap();
    assert_eq!(asset.status(), 200);
    assert_eq!(
        asset.headers()["content-type"],
        "text/javascript; charset=utf-8"
    );
    assert!(
        asset.headers()["cache-control"]
            .to_str()
            .unwrap()
            .contains("immutable")
    );
    // The notices of the npm packages in the page ship with it.
    let notices = page
        .http
        .get(format!("{}/licenses.txt", page.base))
        .send()
        .await
        .unwrap();
    assert_eq!(notices.status(), 200);
    assert_eq!(
        notices.headers()["content-type"],
        "text/plain; charset=utf-8"
    );
    let notices = notices.text().await.unwrap();
    for package in ["react 19", "micromark", "highlight.js", "lucide-react"] {
        assert!(notices.contains(package), "{package} is listed");
    }
    let missing = page
        .http
        .get(format!("{}/assets/nope.js", page.base))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);
}
