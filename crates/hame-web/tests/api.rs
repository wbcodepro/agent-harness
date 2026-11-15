mod common;

use common::{start, write_call};
use hame_eval::ScriptedResponse;
use hame_session::{Delta, StopReason, Usage};
use serde_json::{Value, json};

#[tokio::test]
async fn a_page_runs_a_turn_answers_an_approval_and_can_reload() {
    let dir = tempfile::tempdir().unwrap();
    let page = start(
        dir.path(),
        vec![write_call("a.txt"), ScriptedResponse::text("Written.")],
        0,
    )
    .await;

    let (status, body) = page.post("/sessions", json!({})).await;
    assert_eq!(status, 200, "{body}");
    let session = serde_json::from_str::<Value>(&body).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let mut events = page.events(&session).await;

    assert_eq!(
        page.post(
            &format!("/sessions/{session}/prompt"),
            json!({"text": "write a.txt"})
        )
        .await
        .0,
        200
    );
    let before = events
        .until("the approval question", |e| e["kind"] == "approval")
        .await;
    let question = before.last().unwrap();
    assert!(
        question["question"]
            .as_str()
            .unwrap()
            .starts_with("Allow write")
    );
    assert!(
        !dir.path().join("work/a.txt").exists(),
        "nothing runs before the answer"
    );
    // A second prompt while the turn runs is refused.
    assert_eq!(
        page.post(
            &format!("/sessions/{session}/prompt"),
            json!({"text": "again"})
        )
        .await
        .0,
        409
    );

    let answer =
        json!({"id": question["id"], "allow": true, "tool_call_id": question["tool_call_id"]});
    assert_eq!(
        page.post(&format!("/sessions/{session}/approval"), answer.clone())
            .await
            .0,
        200
    );
    assert_eq!(
        page.post(&format!("/sessions/{session}/approval"), answer)
            .await
            .0,
        400
    );
    let rest = events
        .until("the end of the turn", |e| {
            e["kind"] == "turn" && e["state"] == "ended"
        })
        .await;
    assert_eq!(rest.last().unwrap()["error"], Value::Null);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("work/a.txt")).unwrap(),
        "from the web"
    );
    let streamed: Vec<&str> = before
        .iter()
        .chain(&rest)
        .filter_map(|e| e["record"]["type"].as_str())
        .collect();
    assert!(streamed.contains(&"tool/call") && streamed.contains(&"tool/result"));
    assert_eq!(streamed.last(), Some(&"turn/end"));

    // A reloaded page gets the whole conversation again, in order.
    let mut reloaded = page.events(&session).await;
    let replayed = reloaded
        .until("the replayed end of turn", |e| {
            e["record"]["type"] == "turn/end"
        })
        .await;
    let seqs: Vec<u64> = replayed
        .iter()
        .filter_map(|e| e["record"]["seq"].as_u64())
        .collect();
    assert_eq!(seqs, (0..seqs.len() as u64).collect::<Vec<_>>());
}

#[tokio::test]
async fn the_api_needs_the_token_and_a_turn_can_be_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let slow = ScriptedResponse {
        deltas: (0..2000)
            .map(|n| Delta::Text {
                text: format!("w{n} "),
            })
            .collect(),
        stop: StopReason::EndTurn,
        usage: Usage::default(),
    };
    let page = start(dir.path(), vec![slow], 5).await;

    // The page itself is public; the API is not.
    let home = page
        .http
        .get(format!("{}/", page.base))
        .send()
        .await
        .unwrap();
    assert!(home.text().await.unwrap().contains("<title>hame</title>"));
    for request in [
        page.http.get(format!("{}/api/sessions", page.base)),
        page.http
            .get(format!("{}/api/sessions", page.base))
            .header("x-hame-token", "wrong"),
        // Right token, but addressed to another host name (DNS rebinding).
        page.http
            .get(format!("{}/api/sessions", page.base))
            .header("x-hame-token", &page.token)
            .header("host", "evil.example:80"),
    ] {
        assert_eq!(request.send().await.unwrap().status(), 403);
    }

    let (_, body) = page.post("/sessions", json!({})).await;
    let session = serde_json::from_str::<Value>(&body).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let mut events = page.events(&session).await;
    page.post(
        &format!("/sessions/{session}/prompt"),
        json!({"text": "talk"}),
    )
    .await;
    events
        .until("some streamed text", |e| {
            e["record"]["type"] == "assistant/delta"
        })
        .await;
    assert_eq!(
        page.post(&format!("/sessions/{session}/cancel"), json!({}))
            .await
            .0,
        200
    );
    let rest = events
        .until("the turn to end", |e| {
            e["kind"] == "turn" && e["state"] == "ended"
        })
        .await;
    assert!(
        rest.iter()
            .any(|e| e["record"]["data"]["reason"] == "cancelled")
    );

    assert_eq!(page.post("/sessions/..%2Fx/cancel", json!({})).await.0, 400);
}

#[tokio::test]
async fn artifacts_are_served_only_to_a_session_that_refers_to_them() {
    use hame_session::{Block, CreateOptions, Event, SessionStore};

    let dir = tempfile::tempdir().unwrap();
    let page = start(dir.path(), vec![], 0).await;
    let store = SessionStore::new(dir.path().join("home").join("sessions"));
    let png: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0x0d, 0x49, 0x48, 0x44, 0x52, 0,
        0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0,
    ];
    let shown = store.artifacts().put(png, "image/png").unwrap();
    let report = store
        .artifacts()
        .put(b"<html>x</html>", "text/html")
        .unwrap();
    let other = store
        .artifacts()
        .put(b"%PDF-1.4 secret", "application/pdf")
        .unwrap();
    let gone = store
        .artifacts()
        .put(b"GIF89a\x02\x00\x02\x00", "image/gif")
        .unwrap();
    std::fs::remove_file(store.artifacts().path_of(&gone.sha256).unwrap()).unwrap();
    let mut writer = store
        .create(CreateOptions {
            id: None,
            cwd: dir.path().join("work").to_string_lossy().into_owned(),
        })
        .unwrap();
    let session = writer.id().to_string();
    writer
        .append(Event::UserMessage {
            content: vec![
                Block::text("Look"),
                Block::Image {
                    artifact: shown.clone(),
                },
                Block::Artifact {
                    artifact: report.clone(),
                },
                Block::Image {
                    artifact: gone.clone(),
                },
            ],
        })
        .unwrap();
    drop(writer);
    // Another session refers to the PDF; this one does not.
    let mut elsewhere = store.create(CreateOptions::default()).unwrap();
    elsewhere
        .append(Event::UserMessage {
            content: vec![Block::Artifact {
                artifact: other.clone(),
            }],
        })
        .unwrap();
    drop(elsewhere);

    let get = |path: String, token: bool| {
        let mut request = page.http.get(format!("{}/api{path}", page.base));
        if token {
            request = request.header("x-hame-token", &page.token);
        }
        async move { request.send().await.unwrap() }
    };
    let served = get(
        format!("/artifacts/{}?session={session}", shown.sha256),
        true,
    )
    .await;
    assert_eq!(served.status(), 200);
    assert_eq!(served.headers()["content-type"], "image/png");
    assert_eq!(served.headers()["x-content-type-options"], "nosniff");
    assert_eq!(served.bytes().await.unwrap().as_ref(), png);

    // Not an image: a download with the recorded type, never rendered here.
    let download = get(
        format!("/artifacts/{}?session={session}", report.sha256),
        true,
    )
    .await;
    assert_eq!(download.status(), 200);
    assert_eq!(download.headers()["content-type"], "text/html");
    assert_eq!(download.headers()["content-disposition"], "attachment");

    for (path, status) in [
        // Bad ids.
        (format!("/artifacts/..%2F..%2Fetc?session={session}"), 400),
        (
            format!("/artifacts/{}?session={session}", "A".repeat(64)),
            400,
        ),
        (
            format!("/artifacts/{}?session={session}", &shown.sha256[..63]),
            400,
        ),
        // No session named, or a bad one.
        (format!("/artifacts/{}", shown.sha256), 400),
        (format!("/artifacts/{}?session=..%2Fx", shown.sha256), 400),
        // Stored, but not referenced by this session.
        (
            format!("/artifacts/{}?session={session}", other.sha256),
            404,
        ),
        // Referenced, but gone from the store.
        (format!("/artifacts/{}?session={session}", gone.sha256), 404),
        // An unknown session.
        (format!("/artifacts/{}?session=nosuch", shown.sha256), 404),
    ] {
        assert_eq!(get(path.clone(), true).await.status(), status, "{path}");
    }
    assert_eq!(
        get(
            format!("/artifacts/{}?session={session}", shown.sha256),
            false
        )
        .await
        .status(),
        403
    );

    // Loading the session warns about the missing image.
    let mut events = page.events(&session).await;
    let seen = events
        .until("a warning", |event| event["kind"] == "warning")
        .await;
    let warning = seen.last().unwrap();
    assert_eq!(
        warning["missing_artifacts"][0]["sha256"],
        gone.sha256.as_str()
    );
}
