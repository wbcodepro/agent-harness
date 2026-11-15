//! What the second review found: the remote policy on every session route,
//! a mode change racing a prompt, `..` through a link under Accept edits,
//! older logs, the lock conflict status and confirmation nonces.

mod common;

use std::time::Duration;

use common::{Page, start, tool_call, write_call};
use hame_eval::ScriptedResponse;
use hame_session::{SessionId, SessionStore};
use serde_json::json;

fn write_config(dir: &std::path::Path, text: &str) {
    std::fs::create_dir_all(dir.join("home")).unwrap();
    std::fs::write(dir.join("home/config.toml"), text).unwrap();
}

async fn full_access_session(page: &Page, cwd: Option<&std::path::Path>) -> String {
    let mut body = json!({"mode": "full-access"});
    if let Some(cwd) = cwd {
        body["cwd"] = json!(cwd.to_string_lossy());
    }
    let (status, first) = page.call("POST", "/sessions", Some(body.clone())).await;
    assert_eq!(status, 428, "{first}");
    body["confirm"] = first["confirm"].clone();
    page.session(body).await
}

/// Waits for the end of the turn, answering "deny" to any question, and
/// says whether one was asked.
async fn finish(page: &Page, events: &mut common::Events, session: &str) -> bool {
    let mut asked = false;
    let mut started = false;
    loop {
        let event = events.next().await;
        if event["kind"] == "approval" {
            asked = true;
            let (status, body) = page
                .post(
                    &format!("/sessions/{session}/approval"),
                    json!({"id": event["id"], "allow": false,
                           "tool_call_id": event["tool_call_id"],
                           "args_sha256": event["args_sha256"]}),
                )
                .await;
            assert_eq!(status, 200, "{body}");
        }
        if event["kind"] == "turn" && event["state"] == "started" {
            started = true;
        }
        if started && event["kind"] == "turn" && event["state"] == "ended" {
            assert!(event["error"].is_null(), "turn failed: {event}");
            return asked;
        }
    }
}

#[tokio::test]
async fn a_remote_device_cannot_drive_a_full_access_session_or_one_outside_its_roots() {
    let dir = tempfile::tempdir().unwrap();
    let elsewhere = dir.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::create_dir_all(dir.path().join("projects")).unwrap();
    write_config(
        dir.path(),
        &format!(
            "[remote]\nallow_full_access = false\nroots = [{:?}]\n",
            dir.path().join("projects").to_string_lossy()
        ),
    );
    let target = elsewhere.join("pwned.txt");
    let page = start(
        dir.path(),
        vec![
            write_call(&target.to_string_lossy()),
            ScriptedResponse::text("done"),
        ],
        0,
    )
    .await;
    let session = full_access_session(&page, Some(&elsewhere)).await;
    let (status, _) = page
        .remote(
            "POST",
            "/sessions",
            Some(json!({"cwd": elsewhere.to_string_lossy()})),
        )
        .await;
    assert_eq!(status, 403);
    // Not by prompting the session the page made either, nor any other
    // way of acting on it.
    let base = format!("/sessions/{session}");
    for (method, path, body) in [
        (
            "POST",
            format!("{base}/prompt"),
            Some(json!({"text": "go"})),
        ),
        ("POST", format!("{base}/steer"), Some(json!({"text": "go"}))),
        (
            "POST",
            format!("{base}/steer/withdraw"),
            Some(json!({"index": 0, "text": "go"})),
        ),
        (
            "POST",
            format!("{base}/approval"),
            Some(json!({"id": 1, "allow": true, "tool_call_id": "c1", "args_sha256": "0"})),
        ),
        (
            "PUT",
            format!("{base}/settings"),
            Some(json!({"mode": "ask"})),
        ),
        ("DELETE", format!("{base}/grants/write"), None),
        ("DELETE", base.clone(), None),
    ] {
        let (status, body) = page.remote(method, &path, body).await;
        assert_eq!(status, 403, "{method} {path}: {body}");
    }
    assert!(!target.exists());

    // Inside the roots, a Full access session is still refused...
    let inside = dir.path().join("projects/app");
    std::fs::create_dir_all(&inside).unwrap();
    let full = full_access_session(&page, Some(&inside)).await;
    let (status, body) = page
        .remote(
            "POST",
            &format!("/sessions/{full}/prompt"),
            Some(json!({"text": "go"})),
        )
        .await;
    assert_eq!(status, 403, "{body}");
    assert!(body.as_str().unwrap().contains("Full access"));
    // ...but the device may lower its mode, and then drive it.
    let (status, _) = page
        .remote(
            "PUT",
            &format!("/sessions/{full}/settings"),
            Some(json!({"mode": "ask"})),
        )
        .await;
    assert_eq!(status, 200);
    // Stopping is always allowed.
    assert_eq!(
        page.remote(
            "POST",
            &format!("/sessions/{session}/cancel"),
            Some(json!({}))
        )
        .await
        .0,
        200
    );
}

#[tokio::test]
async fn a_default_session_without_a_sandbox_counts_as_full_access_for_remote_devices() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), "[sandbox]\nmode = \"danger-full-access\"\n");
    // The test server itself runs without a sandbox too; a default session
    // is then effectively Full access, an Ask session is not.
    let page = start(dir.path(), vec![ScriptedResponse::text("ok")], 0).await;
    let default = page.session(json!({})).await;
    let (status, _) = page
        .remote(
            "POST",
            &format!("/sessions/{default}/prompt"),
            Some(json!({"text": "go"})),
        )
        .await;
    assert_eq!(status, 403);
    let ask = page.session(json!({"mode": "ask"})).await;
    let mut events = page.events(&ask).await;
    let (status, body) = page
        .remote(
            "POST",
            &format!("/sessions/{ask}/prompt"),
            Some(json!({"text": "go"})),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    finish(&page, &mut events, &ask).await;
}

#[tokio::test]
async fn grants_given_on_this_computer_do_not_answer_for_a_remote_turn() {
    let dir = tempfile::tempdir().unwrap();
    let page = start(
        dir.path(),
        vec![
            write_call("one.txt"),
            ScriptedResponse::text("one"),
            tool_call("c2", "write", json!({"path": "two.txt", "content": "2"})),
            ScriptedResponse::text("two"),
        ],
        0,
    )
    .await;
    let session = page.session(json!({"mode": "ask"})).await;
    let mut events = page.events(&session).await;
    page.post(
        &format!("/sessions/{session}/prompt"),
        json!({"text": "write one"}),
    )
    .await;
    let question = events
        .until("the question", |event| event["kind"] == "approval")
        .await
        .pop()
        .unwrap();
    page.post(
        &format!("/sessions/{session}/approval"),
        json!({"id": question["id"], "allow": true, "always": true,
               "tool_call_id": question["tool_call_id"], "args_sha256": question["args_sha256"]}),
    )
    .await;
    events
        .until("the end", |event| {
            event["kind"] == "turn" && event["state"] == "ended"
        })
        .await;
    assert!(dir.path().join("work/one.txt").exists());
    // The same tool in a turn a remote device started is asked about.
    let (status, body) = page
        .remote(
            "POST",
            &format!("/sessions/{session}/prompt"),
            Some(json!({"text": "write two"})),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert!(finish(&page, &mut events, &session).await, "asked again");
    assert!(!dir.path().join("work/two.txt").exists());
}

/// A mode change racing a prompt: the agent is built from the mode the
/// log and the page show, never from the one before.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mode_change_racing_a_prompt_never_keeps_full_access() {
    let dir = tempfile::tempdir().unwrap();
    let rounds = 40;
    let mut responses = Vec::new();
    for i in 0..rounds {
        responses.push(write_call(&format!("race-{i}.txt")));
        responses.push(ScriptedResponse::text("done"));
    }
    let page = start(dir.path(), responses, 0).await;
    for i in 0..rounds {
        let session = full_access_session(&page, None).await;
        let mut events = page.events(&session).await;
        let settings_path = format!("/sessions/{session}/settings");
        let settings = page.call("PUT", &settings_path, Some(json!({"mode": "ask"})));
        let prompt = async {
            tokio::time::sleep(Duration::from_micros((i % 30) as u64 * 40)).await;
            page.call(
                "POST",
                &format!("/sessions/{session}/prompt"),
                Some(json!({"text": "go"})),
            )
            .await
        };
        let ((settings_status, _), (prompt_status, body)) = tokio::join!(settings, prompt);
        assert_eq!(prompt_status, 200, "{body}");
        let asked = finish(&page, &mut events, &session).await;
        let written = dir
            .path()
            .join("work")
            .join(format!("race-{i}.txt"))
            .exists();
        let (_, detail) = page.get(&format!("/sessions/{session}")).await;
        assert!(
            !(settings_status == 200 && detail["mode"] == "ask" && !asked && written),
            "round {i}: the mode is Ask but the write ran without a question"
        );
    }
}

/// The same race on a session with a long log, where opening the log to
/// record the change takes a while.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mode_change_racing_a_prompt_on_a_long_session() {
    let dir = tempfile::tempdir().unwrap();
    let rounds = 12;
    let mut responses = vec![ScriptedResponse::text("lorem ipsum ".repeat(200_000))];
    for i in 0..rounds {
        responses.push(write_call(&format!("race-{i}.txt")));
        responses.push(ScriptedResponse::text("done"));
    }
    let page = start(dir.path(), responses, 0).await;
    let session = page.session(json!({})).await;
    let mut events = page.events(&session).await;
    page.post(
        &format!("/sessions/{session}/prompt"),
        json!({"text": "warm up"}),
    )
    .await;
    finish(&page, &mut events, &session).await;
    let settings_path = format!("/sessions/{session}/settings");
    for i in 0..rounds {
        let (status, first) = page
            .call("PUT", &settings_path, Some(json!({"mode": "full-access"})))
            .await;
        assert_eq!(status, 428, "{first}");
        let (status, body) = page
            .call(
                "PUT",
                &settings_path,
                Some(json!({"mode": "full-access", "confirm": first["confirm"]})),
            )
            .await;
        assert_eq!(status, 200, "{body}");
        let settings = page.call("PUT", &settings_path, Some(json!({"mode": "ask"})));
        let prompt = async {
            tokio::time::sleep(Duration::from_micros((i % 20) as u64 * 500)).await;
            page.call(
                "POST",
                &format!("/sessions/{session}/prompt"),
                Some(json!({"text": "go"})),
            )
            .await
        };
        let ((settings_status, _), (prompt_status, body)) = tokio::join!(settings, prompt);
        assert_eq!(prompt_status, 200, "{body}");
        let asked = finish(&page, &mut events, &session).await;
        let written = dir
            .path()
            .join("work")
            .join(format!("race-{i}.txt"))
            .exists();
        let (_, detail) = page.get(&format!("/sessions/{session}")).await;
        assert!(
            !(settings_status == 200 && detail["mode"] == "ask" && !asked && written),
            "round {i}: the mode is Ask but the write ran without a question"
        );
        if settings_status != 200 {
            // The prompt won; put the mode back for the next round.
            page.call("PUT", &settings_path, Some(json!({"mode": "ask"})))
                .await;
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn accept_edits_asks_about_a_dotdot_through_a_link() {
    let dir = tempfile::tempdir().unwrap();
    let outside = dir.path().join("outside");
    std::fs::create_dir_all(outside.join("sub")).unwrap();
    let page = start(
        dir.path(),
        vec![
            write_call("link/../escaped.txt"),
            ScriptedResponse::text("done"),
        ],
        0,
    )
    .await;
    std::os::unix::fs::symlink(outside.join("sub"), dir.path().join("work/link")).unwrap();
    let session = page.session(json!({"mode": "accept-edits"})).await;
    let mut events = page.events(&session).await;
    let (status, body) = page
        .post(
            &format!("/sessions/{session}/prompt"),
            json!({"text": "go"}),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert!(
        finish(&page, &mut events, &session).await,
        "the write is asked about"
    );
    assert!(!outside.join("escaped.txt").exists());
}

#[tokio::test]
async fn a_mode_change_marks_an_older_log_with_the_current_format() {
    let dir = tempfile::tempdir().unwrap();
    let page = start(dir.path(), vec![], 0).await;
    let session = page.session(json!({})).await;
    let store = SessionStore::new(dir.path().join("home/sessions"));
    let id = SessionId::parse(&session).unwrap();
    let path = store.log_path(&id);
    let current = format!("\"version\":{}", hame_session::FORMAT_VERSION);
    let log = std::fs::read_to_string(&path).unwrap();
    // As hame 0.1.0-rc.1 wrote it.
    std::fs::write(&path, log.replacen(&current, "\"version\":2", 1)).unwrap();
    let (status, body) = page
        .call(
            "PUT",
            &format!("/sessions/{session}/settings"),
            Some(json!({"mode": "ask"})),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    let log = std::fs::read_to_string(&path).unwrap();
    assert!(log.lines().next().unwrap().contains(&current));
    assert!(log.contains("\"mode/changed\""));
    assert_eq!(store.read(&id).unwrap().len(), 2);
}

#[tokio::test]
async fn a_session_open_in_another_process_is_423_not_409() {
    let dir = tempfile::tempdir().unwrap();
    let page = start(dir.path(), vec![ScriptedResponse::text("ok")], 0).await;
    let session = page.session(json!({})).await;
    let store = SessionStore::new(dir.path().join("home/sessions"));
    // What a terminal has while it runs the session.
    let (_, writer) = store.open(&SessionId::parse(&session).unwrap()).unwrap();
    let path = |tail: &str| format!("/sessions/{session}{tail}");
    let (status, body) = page
        .call("POST", &path("/prompt"), Some(json!({"text": "go"})))
        .await;
    assert_eq!(status, 423, "{body}");
    assert!(body.as_str().unwrap().contains("another hame process"));
    let (status, _) = page
        .call("PUT", &path("/settings"), Some(json!({"mode": "ask"})))
        .await;
    assert_eq!(status, 423);
    assert_eq!(page.call("DELETE", &path(""), None).await.0, 423);
    // Nothing is left marked as running.
    let (_, detail) = page.get(&path("")).await;
    assert_eq!(detail["busy"], false);
    drop(writer);
    assert_eq!(
        page.call("POST", &path("/prompt"), Some(json!({"text": "go"})))
            .await
            .0,
        200
    );
}

#[tokio::test]
async fn a_confirmation_is_used_up_only_by_a_change_that_happens() {
    let dir = tempfile::tempdir().unwrap();
    let page = start(
        dir.path(),
        vec![ScriptedResponse {
            deltas: (0..400)
                .map(|n| hame_session::Delta::Text {
                    text: format!("w{n} "),
                })
                .collect(),
            stop: hame_session::StopReason::EndTurn,
            usage: hame_session::Usage::default(),
        }],
        5,
    )
    .await;
    let session = page.session(json!({})).await;
    let settings = format!("/sessions/{session}/settings");
    let (_, first) = page
        .call("PUT", &settings, Some(json!({"mode": "full-access"})))
        .await;
    let nonce = first["confirm"].clone();
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
    // Refused while the turn runs; the nonce still counts afterwards.
    let (status, _) = page
        .call(
            "PUT",
            &settings,
            Some(json!({"mode": "full-access", "confirm": nonce})),
        )
        .await;
    assert_eq!(status, 409);
    page.post(&format!("/sessions/{session}/cancel"), json!({}))
        .await;
    events
        .until("the end", |event| {
            event["kind"] == "turn" && event["state"] == "ended"
        })
        .await;
    let (status, body) = page
        .call(
            "PUT",
            &settings,
            Some(json!({"mode": "full-access", "confirm": nonce})),
        )
        .await;
    assert_eq!(status, 200, "{body}");

    // A nonce for a new session counts only for the folder and mode it was
    // issued for.
    let other = dir.path().join("other");
    std::fs::create_dir_all(&other).unwrap();
    let (_, first) = page
        .call("POST", "/sessions", Some(json!({"mode": "full-access"})))
        .await;
    let (status, _) = page
        .call(
            "POST",
            "/sessions",
            Some(
                json!({"mode": "full-access", "cwd": other.to_string_lossy(),
                        "confirm": first["confirm"]}),
            ),
        )
        .await;
    assert_eq!(status, 428, "another folder needs its own confirmation");
    let (status, _) = page
        .call(
            "POST",
            "/sessions",
            Some(json!({"mode": "full-access", "confirm": first["confirm"]})),
        )
        .await;
    assert_eq!(status, 200);
}

fn slow_text(words: usize) -> ScriptedResponse {
    ScriptedResponse {
        deltas: (0..words)
            .map(|n| hame_session::Delta::Text {
                text: format!("w{n} "),
            })
            .collect(),
        stop: hame_session::StopReason::EndTurn,
        usage: hame_session::Usage::default(),
    }
}

/// A message a remote device queued into a turn that was then stopped
/// stays remote: the next turn, started on this computer, picks it up, and
/// a grant given on this computer does not answer for what it asks.
#[tokio::test]
async fn a_remote_message_left_queued_keeps_its_turn_remote() {
    let dir = tempfile::tempdir().unwrap();
    let page = start(
        dir.path(),
        vec![
            write_call("one.txt"),
            ScriptedResponse::text("one"),
            slow_text(2000),
            tool_call("c2", "write", json!({"path": "two.txt", "content": "2"})),
            ScriptedResponse::text("two"),
        ],
        5,
    )
    .await;
    let session = page.session(json!({"mode": "ask"})).await;
    let mut events = page.events(&session).await;
    // A grant for `write`, given on this computer.
    page.post(
        &format!("/sessions/{session}/prompt"),
        json!({"text": "write one"}),
    )
    .await;
    let question = events
        .until("the question", |event| event["kind"] == "approval")
        .await
        .pop()
        .unwrap();
    page.post(
        &format!("/sessions/{session}/approval"),
        json!({"id": question["id"], "allow": true, "always": true,
               "tool_call_id": question["tool_call_id"], "args_sha256": question["args_sha256"]}),
    )
    .await;
    events
        .until("the end", |event| {
            event["kind"] == "turn" && event["state"] == "ended"
        })
        .await;
    // A local turn; a remote device queues into it; it is stopped.
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
    let (status, body) = page
        .remote(
            "POST",
            &format!("/sessions/{session}/steer"),
            Some(json!({"text": "write two"})),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    let queued = events
        .until("the queue", |event| {
            event["kind"] == "steering" && event["queued"] == json!(["write two"])
        })
        .await
        .pop()
        .unwrap();
    assert_eq!(
        queued["origins"],
        json!(["Ana's phone"]),
        "the origin is shown"
    );
    page.post(&format!("/sessions/{session}/cancel"), json!({}))
        .await;
    events
        .until("the stop", |event| {
            event["kind"] == "turn" && event["state"] == "ended"
        })
        .await;
    // The next local turn runs the remote message: the write is asked about.
    let (status, body) = page
        .post(
            &format!("/sessions/{session}/prompt"),
            json!({"text": "go on"}),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert!(
        finish(&page, &mut events, &session).await,
        "the local grant did not answer for the remote message"
    );
    assert!(!dir.path().join("work/two.txt").exists());
}

#[tokio::test]
async fn artifacts_stream_and_a_damaged_one_does_not_complete() {
    use hame_session::{Block, CreateOptions, Event};
    let dir = tempfile::tempdir().unwrap();
    let page = start(dir.path(), vec![], 0).await;
    let store = SessionStore::new(dir.path().join("home/sessions"));
    let png: Vec<u8> = [
        &[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a][..],
        &vec![7u8; 300_000][..],
    ]
    .concat();
    let stored = store.artifacts().put(&png, "image/png").unwrap();
    let mut writer = store
        .create(CreateOptions {
            id: None,
            cwd: dir.path().join("work").to_string_lossy().into_owned(),
        })
        .unwrap();
    let session = writer.id().to_string();
    writer
        .append(Event::UserMessage {
            content: vec![Block::Image {
                artifact: stored.clone(),
            }],
        })
        .unwrap();
    drop(writer);
    let get = || {
        page.http
            .get(format!(
                "{}/api/artifacts/{}?session={session}",
                page.base, stored.sha256
            ))
            .header("x-hame-token", &page.token)
            .send()
    };
    let served = get().await.unwrap();
    assert_eq!(served.status(), 200);
    assert_eq!(served.headers()["content-length"], png.len().to_string());
    assert_eq!(served.headers()["content-type"], "image/png");
    assert_eq!(served.bytes().await.unwrap().as_ref(), png.as_slice());

    // Same size, other bytes: the stream ends with an error.
    let path = store.artifacts().path_of(&stored.sha256).unwrap();
    let mut damaged = png.clone();
    damaged[200_000] ^= 0xff;
    std::fs::write(&path, &damaged).unwrap();
    let response = get().await.unwrap();
    assert_eq!(response.status(), 200, "the damage shows only at the end");
    assert!(
        response.bytes().await.is_err(),
        "the body does not complete"
    );
    // Another size: refused before anything is sent.
    std::fs::write(&path, &png[..1000]).unwrap();
    assert_eq!(get().await.unwrap().status(), 500);
}
