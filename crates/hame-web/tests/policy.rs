//! Who may do what: remote callers, "Full access" confirmation, grants,
//! approvals bound to their tool call, stopping, the session lock, and
//! uploads that must stay inside the session's folder.

mod common;

use std::time::{Duration, Instant};

use common::{Page, start, tool_call, write_call};
use hame_eval::ScriptedResponse;
use hame_session::{Delta, Event, SessionId, SessionStore, StopReason, Usage};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn turn_ended(event: &Value) -> bool {
    event["kind"] == "turn" && event["state"] == "ended"
}

fn turn_started(event: &Value) -> bool {
    event["kind"] == "turn" && event["state"] == "started"
}

fn store(page: &Page) -> SessionStore {
    SessionStore::new(page.dir.join("home").join("sessions"))
}

fn mode_changes(page: &Page, session: &str) -> Vec<(String, String, String, Option<String>)> {
    store(page)
        .read(&SessionId::parse(session).unwrap())
        .unwrap()
        .into_iter()
        .filter_map(|record| match record.event {
            Event::ModeChanged {
                mode,
                previous,
                by,
                label,
            } => Some((mode, previous, by, label)),
            _ => None,
        })
        .collect()
}

fn write_config(dir: &std::path::Path, text: &str) {
    std::fs::create_dir_all(dir.join("home")).unwrap();
    std::fs::write(dir.join("home/config.toml"), text).unwrap();
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

#[tokio::test]
async fn remote_callers_get_the_remote_policy() {
    let dir = tempfile::tempdir().unwrap();
    let page = start(dir.path(), vec![], 0).await;
    let elsewhere = dir.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();

    // In process, marked as remote: no token needed.
    let (status, _) = page.remote("GET", "/sessions", None).await;
    assert_eq!(status, 200);
    // Over HTTP without the token: refused as before.
    let refused = page
        .http
        .get(format!("{}/api/sessions", page.base))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 403);

    // Remote sessions start in the folder hame web started in, or one that
    // already has a session, unless roots allow more.
    let (status, created) = page.remote("POST", "/sessions", Some(json!({}))).await;
    assert_eq!(status, 200, "{created}");
    let session = created["id"].as_str().unwrap().to_string();
    let (status, body) = page
        .remote(
            "POST",
            "/sessions",
            Some(json!({"cwd": elsewhere.to_string_lossy()})),
        )
        .await;
    assert_eq!(status, 403, "{body}");
    assert!(body.as_str().unwrap().contains("[remote] roots"));
    // The page on this computer may.
    let local = page
        .session(json!({"cwd": elsewhere.to_string_lossy()}))
        .await;
    // Now that folder has a session, a remote device may use it too.
    assert_eq!(
        page.remote(
            "POST",
            "/sessions",
            Some(json!({"cwd": elsewhere.to_string_lossy()}))
        )
        .await
        .0,
        200
    );
    let deeper = dir.path().join("projects/app");
    std::fs::create_dir_all(&deeper).unwrap();
    let body = Some(json!({"cwd": deeper.to_string_lossy()}));
    assert_eq!(page.remote("POST", "/sessions", body.clone()).await.0, 403);
    write_config(
        dir.path(),
        &format!(
            "[remote]\nroots = [{:?}]\n",
            dir.path().join("projects").to_string_lossy()
        ),
    );
    assert_eq!(page.remote("POST", "/sessions", body).await.0, 200);
    // With roots set, the start folder is outside them: the device may no
    // longer act on the session there.
    let (status, _) = page
        .remote(
            "PUT",
            &format!("/sessions/{session}/settings"),
            Some(json!({"mode": "ask"})),
        )
        .await;
    assert_eq!(status, 403);
    write_config(dir.path(), "");

    // No Full access for a remote device unless allowed.
    let settings = format!("/sessions/{session}/settings");
    let (status, body) = page
        .remote("PUT", &settings, Some(json!({"mode": "full-access"})))
        .await;
    assert_eq!(status, 403, "{body}");
    let (status, _) = page
        .remote("POST", "/sessions", Some(json!({"mode": "full-access"})))
        .await;
    assert_eq!(status, 403);
    // Other modes are fine, and recorded with the device.
    let (status, body) = page
        .remote("PUT", &settings, Some(json!({"mode": "ask"})))
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        mode_changes(&page, &session),
        vec![(
            "ask".into(),
            "default".into(),
            "phone-1".into(),
            Some("Ana's phone".into())
        )]
    );
    write_config(dir.path(), "[remote]\nallow_full_access = true\n");
    let (status, body) = page
        .remote("PUT", &settings, Some(json!({"mode": "full-access"})))
        .await;
    assert_eq!(status, 428, "allowed, but still confirmed: {body}");
    let (status, _) = page
        .remote(
            "PUT",
            &settings,
            Some(json!({"mode": "full-access", "confirm": body["confirm"]})),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(page.get(&format!("/sessions/{local}")).await.0, 200);
}

#[tokio::test]
async fn full_access_needs_a_one_time_confirmation_and_every_change_is_logged() {
    let dir = tempfile::tempdir().unwrap();
    let page = start(dir.path(), vec![], 0).await;
    let session = page.session(json!({})).await;
    let settings = format!("/sessions/{session}/settings");

    let (status, first) = page
        .call("PUT", &settings, Some(json!({"mode": "full-access"})))
        .await;
    assert_eq!(status, 428, "{first}");
    let nonce = first["confirm"].as_str().unwrap().to_string();
    assert!(
        first["message"]
            .as_str()
            .unwrap()
            .contains("without the sandbox")
    );
    // A made-up nonce is not enough, and gets a fresh one.
    let (status, again) = page
        .call(
            "PUT",
            &settings,
            Some(json!({"mode": "full-access", "confirm": "guess"})),
        )
        .await;
    assert_eq!(status, 428);
    assert_ne!(again["confirm"], first["confirm"]);
    // A nonce issued for another session does not count here.
    let other = page.session(json!({})).await;
    assert_eq!(
        page.call(
            "PUT",
            &format!("/sessions/{other}/settings"),
            Some(json!({"mode": "full-access", "confirm": nonce}))
        )
        .await
        .0,
        428
    );
    let (status, body) = page
        .call(
            "PUT",
            &settings,
            Some(json!({"mode": "full-access", "confirm": nonce})),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["mode"], "full-access");
    // Used once only.
    page.call("PUT", &settings, Some(json!({"mode": "ask"})))
        .await;
    assert_eq!(
        page.call(
            "PUT",
            &settings,
            Some(json!({"mode": "full-access", "confirm": nonce}))
        )
        .await
        .0,
        428
    );
    assert_eq!(
        mode_changes(&page, &session),
        vec![
            ("full-access".into(), "default".into(), "local".into(), None),
            ("ask".into(), "full-access".into(), "local".into(), None),
        ]
    );

    // Creating a session in Full access takes the same two steps, and a
    // session created in another mode starts with that change in its log.
    let (status, first) = page
        .call("POST", "/sessions", Some(json!({"mode": "full-access"})))
        .await;
    assert_eq!(status, 428);
    let (status, created) = page
        .call(
            "POST",
            "/sessions",
            Some(json!({"mode": "full-access", "confirm": first["confirm"]})),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        mode_changes(&page, created["id"].as_str().unwrap()),
        vec![("full-access".into(), "default".into(), "local".into(), None)]
    );
}

#[tokio::test]
async fn approvals_name_their_call_and_grants_end_with_a_mode_change() {
    let dir = tempfile::tempdir().unwrap();
    let page = start(
        dir.path(),
        vec![
            write_call("a.txt"),
            tool_call("c2", "write", json!({"path": "b.txt", "content": "two"})),
            ScriptedResponse::text("Done."),
            tool_call("c3", "write", json!({"path": "c.txt", "content": "three"})),
            ScriptedResponse::text("Done again."),
        ],
        0,
    )
    .await;
    let session = page.session(json!({})).await;
    let mut events = page.events(&session).await;
    page.post(
        &format!("/sessions/{session}/prompt"),
        json!({"text": "write"}),
    )
    .await;
    let question = events
        .until("the question", |event| event["kind"] == "approval")
        .await
        .pop()
        .unwrap();
    // The exact arguments, and the sha256 of their canonical JSON
    // (RFC 8785: members sorted, no spaces), as the relay connector and
    // devices compute it.
    assert_eq!(
        question["arguments"],
        json!({"path": "a.txt", "content": "from the web"})
    );
    assert_eq!(question["tool"], "write");
    let canonical = r#"{"content":"from the web","path":"a.txt"}"#;
    let hash: String = Sha256::digest(canonical.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(question["args_sha256"], hash.as_str());
    assert_eq!(question["tool_call_id"], "c1");

    let approval = format!("/sessions/{session}/approval");
    for (answer, why) in [
        (json!({"id": question["id"], "allow": true}), "no call id"),
        (
            json!({"id": question["id"], "allow": true, "tool_call_id": "c9"}),
            "another call",
        ),
        (
            json!({"id": question["id"], "allow": true, "tool_call_id": "c1", "args_sha256": "00"}),
            "other arguments",
        ),
        (
            json!({"id": question["id"], "allow": true, "tool_call_id": "c1"}),
            "no argument hash",
        ),
    ] {
        let status = page.post(&approval, answer).await.0;
        assert!(status == 400 || status == 422, "{why}: {status}");
    }
    // A remote device may not allow a tool for the whole session.
    let (status, _) = page
        .remote(
            "POST",
            &approval,
            Some(json!({"id": question["id"], "allow": true, "always": true,
                        "tool_call_id": "c1", "args_sha256": hash})),
        )
        .await;
    assert_eq!(status, 403);
    let (status, body) = page
        .post(
            &approval,
            json!({"id": question["id"], "allow": true, "always": true,
                   "tool_call_id": "c1", "args_sha256": hash}),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    events.until("the end of the turn", turn_ended).await;
    let (_, detail) = page.get(&format!("/sessions/{session}")).await;
    assert_eq!(detail["grants"], json!(["write"]));

    // A new mode ends the grants: the next write is asked about again.
    page.call(
        "PUT",
        &format!("/sessions/{session}/settings"),
        Some(json!({"mode": "ask"})),
    )
    .await;
    let (_, detail) = page.get(&format!("/sessions/{session}")).await;
    assert_eq!(detail["grants"], json!([]));
    assert_eq!(
        page.call("DELETE", &format!("/sessions/{session}/grants/write"), None)
            .await
            .0,
        404
    );
    page.post(
        &format!("/sessions/{session}/prompt"),
        json!({"text": "write again"}),
    )
    .await;
    let asked = events
        .until("the next question", |event| event["kind"] == "approval")
        .await
        .pop()
        .unwrap();
    assert_eq!(asked["tool_call_id"], "c3");
    let (status, _) = page
        .post(
            &approval,
            json!({"id": asked["id"], "allow": true, "always": true, "tool_call_id": "c3",
                   "args_sha256": asked["args_sha256"]}),
        )
        .await;
    assert_eq!(status, 200);
    events.until("the end of the turn", turn_ended).await;
    let (status, revoked) = page
        .call("DELETE", &format!("/sessions/{session}/grants/write"), None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(revoked["grants"], json!([]));
}

#[tokio::test]
async fn a_stop_sent_right_after_the_prompt_is_honoured() {
    let dir = tempfile::tempdir().unwrap();
    let page = start(dir.path(), vec![slow_text(2000), slow_text(2000)], 5).await;
    let session = page.session(json!({})).await;
    let mut events = page.events(&session).await;
    let began = Instant::now();
    page.post(
        &format!("/sessions/{session}/prompt"),
        json!({"text": "talk"}),
    )
    .await;
    // No waiting for the turn to begin.
    let (status, body) = page
        .post(&format!("/sessions/{session}/cancel"), json!({}))
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["stopping"],
        true
    );
    // The state replayed on subscribing, then the end of the stopped run.
    events.until("the replayed state", turn_ended).await;
    events.until("the run's end", turn_ended).await;
    assert!(
        began.elapsed() < Duration::from_secs(8),
        "the 10-second answer was cut short"
    );
    let (_, detail) = page.get(&format!("/sessions/{session}")).await;
    assert_eq!(detail["busy"], false);
    // With nothing running, a stop changes nothing.
    let (_, body) = page
        .post(&format!("/sessions/{session}/cancel"), json!({}))
        .await;
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["stopping"],
        false
    );
}

#[tokio::test]
async fn messages_queued_when_a_turn_is_stopped_are_kept() {
    let dir = tempfile::tempdir().unwrap();
    let page = start(dir.path(), vec![slow_text(2000)], 5).await;
    let session = page.session(json!({})).await;
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
    page.post(
        &format!("/sessions/{session}/steer"),
        json!({"text": "keep me"}),
    )
    .await;
    page.post(&format!("/sessions/{session}/cancel"), json!({}))
        .await;
    let rest = events.until("the end", turn_ended).await;
    assert!(
        rest.iter()
            .any(|e| e["kind"] == "steering" && e["queued"] == json!(["keep me"]))
    );
    let (_, detail) = page.get(&format!("/sessions/{session}")).await;
    assert_eq!(detail["queued"], json!(["keep me"]));
    // A message sent now is refused as steering (send it as a prompt), and
    // the queue state is still announced.
    let (status, _) = page
        .post(
            &format!("/sessions/{session}/steer"),
            json!({"text": "late"}),
        )
        .await;
    assert_eq!(status, 409);
    events
        .until("the queue state", |e| e["kind"] == "steering")
        .await;
}

#[tokio::test]
async fn a_viewed_session_is_not_locked_and_an_idle_agent_lets_go() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), "[web]\nidle_seconds = 1\n");
    let page = start(dir.path(), vec![ScriptedResponse::text("Hi.")], 0).await;
    let session = page.session(json!({})).await;
    let id = SessionId::parse(&session).unwrap();
    let mut events = page.events(&session).await;
    assert_eq!(page.get(&format!("/sessions/{session}")).await.0, 200);
    // What `hame tui --session` does: take the lock. Viewing does not hold it.
    let (_, writer) = store(&page)
        .open(&id)
        .expect("a viewed session can be resumed");
    drop(writer);

    page.post(
        &format!("/sessions/{session}/prompt"),
        json!({"text": "hi"}),
    )
    .await;
    events.until("the turn to start", turn_started).await;
    events.until("the end", turn_ended).await;
    // Right after the turn the agent keeps the lock...
    assert!(matches!(
        store(&page).open(&id),
        Err(hame_session::SessionError::Locked(_))
    ));
    // ...and lets it go once idle.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match store(&page).open(&id) {
            Ok((_, writer)) => {
                drop(writer);
                break;
            }
            Err(_) if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            Err(e) => panic!("still locked: {e}"),
        }
    }
    // A session changed by another process shows its new state in the list.
    let (mut records, mut writer) = store(&page).open(&id).unwrap();
    writer
        .append(Event::TurnStart { turn: 9 })
        .map(|record| records.push(record))
        .unwrap();
    drop(writer);
    let (_, list) = page.get("/sessions").await;
    assert_eq!(list[0]["turns"], 2);
}

#[tokio::test]
async fn a_new_subscription_gets_the_whole_live_state() {
    let dir = tempfile::tempdir().unwrap();
    let page = start(dir.path(), vec![ScriptedResponse::text("Hi.")], 0).await;
    let session = page.session(json!({})).await;
    let mut events = page.events(&session).await;
    let state = events
        .until("the grants", |event| event["kind"] == "grants")
        .await;
    // After the log's records (here only the session header).
    let state: Vec<&Value> = state
        .iter()
        .filter(|event| event["kind"] != "record")
        .collect();
    let kinds: Vec<&str> = state
        .iter()
        .filter_map(|event| event["kind"].as_str())
        .collect();
    assert_eq!(kinds, ["turn", "approvals", "steering", "grants"]);
    assert_eq!(state[0]["state"], "ended");
    assert_eq!(state[1]["pending"], json!([]));
}

#[tokio::test]
async fn uploads_stay_inside_the_session_folder() {
    let dir = tempfile::tempdir().unwrap();
    let page = start(dir.path(), vec![], 0).await;
    let session = page.session(json!({})).await;
    let upload = async |name: &str, mime: &str, bytes: &[u8]| {
        page.http
            .post(format!(
                "{}/api/sessions/{session}/attachments?name={name}",
                page.base
            ))
            .header("x-hame-token", &page.token)
            .header("content-type", mime)
            .body(bytes.to_vec())
            .send()
            .await
            .unwrap()
            .status()
            .as_u16()
    };
    let artifacts = page.dir.join("home/sessions/artifacts");
    let count = || {
        walk(&artifacts)
            .into_iter()
            .filter(|path| path.is_file())
            .count()
    };
    let before = count();
    assert_eq!(upload("x.png", "image/png", b"not an image").await, 400);
    assert_eq!(
        count(),
        before,
        "bytes that are not an image are not stored"
    );

    assert_eq!(upload("notes.txt", "text/plain", b"hi").await, 200);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("work/.hame/attachments/.gitignore")).unwrap(),
        "*\n"
    );

    #[cfg(unix)]
    {
        let other = page.session(json!({"cwd": dir.path().join("work2").to_string_lossy()}));
        std::fs::create_dir_all(dir.path().join("work2")).unwrap();
        let other = other.await;
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, dir.path().join("work2/.hame")).unwrap();
        let status = page
            .http
            .post(format!(
                "{}/api/sessions/{other}/attachments?name=x.txt",
                page.base
            ))
            .header("x-hame-token", &page.token)
            .header("content-type", "text/plain")
            .body("secret")
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, 400);
        assert!(
            walk(&outside).is_empty(),
            "nothing is written through the link"
        );
    }
}

fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            found.push(path.clone());
            if path.is_dir() {
                found.extend(walk(&path));
            }
        }
    }
    found
}

#[tokio::test]
async fn accept_edits_asks_about_a_write_outside_the_folder() {
    let dir = tempfile::tempdir().unwrap();
    let page = start(
        dir.path(),
        vec![
            tool_call(
                "c1",
                "write",
                json!({"path": "../outside.txt", "content": "x"}),
            ),
            ScriptedResponse::text("Ok."),
        ],
        0,
    )
    .await;
    let session = page.session(json!({"mode": "accept-edits"})).await;
    let mut events = page.events(&session).await;
    page.post(
        &format!("/sessions/{session}/prompt"),
        json!({"text": "write outside"}),
    )
    .await;
    let question = events
        .until("the question", |event| event["kind"] == "approval")
        .await
        .pop()
        .unwrap();
    assert_eq!(question["tool_call_id"], "c1");
    page.post(
        &format!("/sessions/{session}/approval"),
        json!({"id": question["id"], "allow": false, "tool_call_id": "c1",
               "args_sha256": question["args_sha256"]}),
    )
    .await;
    events.until("the end", turn_ended).await;
    assert!(!dir.path().join("outside.txt").exists());
}
