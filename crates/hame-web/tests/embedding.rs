//! What an app that embeds the server (the desktop app) reads from it:
//! activity across sessions, the running turns, stopping them all, and a
//! token of its own.
mod common;

use std::time::Duration;

use common::{serve, write_call};
use hame_app::Options;
use hame_eval::ScriptedResponse;
use hame_session::{Delta, StopReason, Usage};
use hame_web::{Activity, RunningTurn};
use serde_json::json;
use tokio::sync::broadcast;

async fn next(activity: &mut broadcast::Receiver<Activity>) -> Activity {
    tokio::time::timeout(Duration::from_secs(20), activity.recv())
        .await
        .expect("activity arrives in time")
        .expect("the channel stays open")
}

#[tokio::test]
async fn activity_reports_turns_and_questions_and_running_turns_are_listed() {
    let dir = tempfile::tempdir().unwrap();
    let (server, page) = serve(
        dir.path(),
        vec![write_call("a.txt"), ScriptedResponse::text("Written.")],
        0,
        |_| {},
    )
    .await;
    let mut activity = server.activity();
    let session = page.session(json!({"mode": "ask"})).await;
    assert!(server.running().is_empty());
    page.post(
        &format!("/sessions/{session}/prompt"),
        json!({"text": "write a file please"}),
    )
    .await;
    assert_eq!(
        next(&mut activity).await,
        Activity::TurnStarted {
            session: session.clone()
        }
    );
    let Activity::Approval {
        session: asked,
        tool,
        question,
    } = next(&mut activity).await
    else {
        panic!("an approval question");
    };
    assert_eq!((asked.as_str(), tool.as_str()), (session.as_str(), "write"));
    assert!(!question.is_empty());
    assert_eq!(
        server.running(),
        vec![RunningTurn {
            session: session.clone(),
            title: Some("write a file please".into()),
            needs_you: true,
        }]
    );
    assert_eq!(
        server.title(&session).as_deref(),
        Some("write a file please")
    );

    let (_, detail) = page.get(&format!("/sessions/{session}")).await;
    let asked = &detail["approvals"][0];
    let (status, _) = page
        .post(
            &format!("/sessions/{session}/approval"),
            json!({"id": asked["id"], "allow": true, "tool_call_id": asked["tool_call_id"], "args_sha256": asked["args_sha256"]}),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        next(&mut activity).await,
        Activity::TurnEnded {
            session: session.clone(),
            error: None,
            cancelled: false,
        }
    );
    assert!(server.running().is_empty());
}

#[tokio::test]
async fn stopping_the_turns_ends_them_as_cancelled() {
    let dir = tempfile::tempdir().unwrap();
    let slow = ScriptedResponse {
        deltas: (0..5000)
            .map(|n| Delta::Text {
                text: format!("w{n} "),
            })
            .collect(),
        stop: StopReason::EndTurn,
        usage: Usage::default(),
    };
    let (server, page) = serve(dir.path(), vec![slow], 5, |_| {}).await;
    let mut activity = server.activity();
    let session = page.session(json!({})).await;
    page.post(
        &format!("/sessions/{session}/prompt"),
        json!({"text": "talk"}),
    )
    .await;
    assert!(matches!(
        next(&mut activity).await,
        Activity::TurnStarted { .. }
    ));
    assert_eq!(server.running().len(), 1);
    assert_eq!(server.running_count(), 1);
    assert_eq!(server.stop_turns(Duration::from_secs(10)).await, 0);
    assert_eq!(
        next(&mut activity).await,
        Activity::TurnEnded {
            session,
            error: None,
            cancelled: true,
        }
    );
    assert!(server.running().is_empty());
}

#[tokio::test]
async fn a_stop_right_after_the_prompt_is_not_lost() {
    let dir = tempfile::tempdir().unwrap();
    let slow = ScriptedResponse {
        deltas: (0..5000)
            .map(|n| Delta::Text {
                text: format!("w{n} "),
            })
            .collect(),
        stop: StopReason::EndTurn,
        usage: Usage::default(),
    };
    let (server, page) = serve(dir.path(), vec![slow], 5, |_| {}).await;
    let mut activity = server.activity();
    let session = page.session(json!({})).await;
    // The prompt answers once the run is spawned; the stop comes before
    // the run has had a chance to wait for one.
    let (status, _) = page
        .post(
            &format!("/sessions/{session}/prompt"),
            json!({"text": "talk"}),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(server.stop_turns(Duration::from_secs(10)).await, 0);
    loop {
        if let Activity::TurnEnded { cancelled, .. } = next(&mut activity).await {
            assert!(cancelled, "the turn was stopped, not finished");
            break;
        }
    }
    assert_eq!(server.running_count(), 0);
}

#[tokio::test]
async fn a_turn_a_remote_device_drives_is_reported_and_asks_on_this_computer() {
    let dir = tempfile::tempdir().unwrap();
    let (server, page) = serve(
        dir.path(),
        vec![write_call("a.txt"), ScriptedResponse::text("Written.")],
        0,
        // As `hame connect` serves: sandboxed (Full access is closed to
        // remote devices) and asking before changes.
        |options| {
            options.sandbox = Some(hame_app::SandboxMode::WorkspaceWrite);
            options.ask_before_changes = true;
        },
    )
    .await;
    let mut activity = server.activity();
    // A remote device starts the session and the turn through the router,
    // as the relay connector does; the desktop app's own page is local.
    let (status, created) = page.remote("POST", "/sessions", Some(json!({}))).await;
    assert_eq!(status, 200, "{created}");
    let session = created["id"].as_str().unwrap().to_string();
    let (status, body) = page
        .remote(
            "POST",
            &format!("/sessions/{session}/prompt"),
            Some(json!({"text": "write it from the phone"})),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        next(&mut activity).await,
        Activity::TurnStarted {
            session: session.clone()
        }
    );
    // Remote turns ask before a change: the embedding app hears the question.
    let asked = next(&mut activity).await;
    let Activity::Approval { tool, .. } = asked.clone() else {
        panic!("an approval question, not {asked:?}");
    };
    assert_eq!(tool, "write");
    assert_eq!(server.running_count(), 1);
    assert!(server.running()[0].needs_you);
    let (_, detail) = page.get(&format!("/sessions/{session}")).await;
    let asked = &detail["approvals"][0];
    let (status, _) = page
        .post(
            &format!("/sessions/{session}/approval"),
            json!({"id": asked["id"], "allow": true, "tool_call_id": asked["tool_call_id"], "args_sha256": asked["args_sha256"]}),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        next(&mut activity).await,
        Activity::TurnEnded {
            session,
            error: None,
            cancelled: false,
        }
    );
}

#[tokio::test]
async fn the_embedding_app_can_choose_the_token() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    for weak in ["", "short", "has spaces in it but is long enough to pass"] {
        assert!(
            hame_web::start_with_token(
                dir.path().join("home"),
                work.clone(),
                Options::default(),
                0,
                weak.into(),
            )
            .await
            .is_err(),
            "{weak:?} is refused"
        );
    }
    let token = "a".repeat(20) + &"0123456789".repeat(3);
    let server = hame_web::start_with_token(
        dir.path().join("home"),
        work,
        Options::default(),
        0,
        token.clone(),
    )
    .await
    .unwrap();
    assert_eq!(server.token, token);
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let info = http
        .get(format!("http://{}/api/info", server.addr))
        .header("x-hame-token", &token)
        .send()
        .await
        .unwrap();
    assert_eq!(info.status(), 200);
}
