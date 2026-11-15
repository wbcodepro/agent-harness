//! Remote sessions ask before every change: with `ask_before_changes`, a
//! confined sandbox, `[approval] default = "allow"` and a rule allowing the
//! tool do not let a write run unasked. Without it they do (control).

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::Request;
use futures_util::StreamExt;
use hame_app::{Options, SandboxMode};
use hame_eval::{Recording, ScriptedLlm, ScriptedResponse};
use hame_session::{Delta, StopReason, Usage};
use hame_tools::InProcess;
use serde_json::{Value, json};
use tower::ServiceExt;

fn write_call() -> ScriptedResponse {
    ScriptedResponse {
        deltas: vec![
            Delta::ToolUseStart {
                id: "c1".into(),
                name: "write".into(),
            },
            Delta::ToolInput {
                json: json!({"path": "a.txt", "content": "x"}).to_string(),
            },
        ],
        stop: StopReason::ToolUse,
        usage: Usage::default(),
    }
}

async fn call(
    router: &axum::Router,
    token: &str,
    method: &str,
    path: &str,
) -> axum::response::Response {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("host", "127.0.0.1:0")
        .header("x-hame-token", token)
        .header("content-type", "application/json")
        .body(Body::from(
            if method == "POST" && path.ends_with("prompt") {
                json!({"text": "write"}).to_string()
            } else {
                "{}".to_string()
            },
        ))
        .unwrap();
    router.clone().oneshot(request).await.unwrap()
}

/// Runs one turn and returns the kinds of events seen until it ends or an
/// approval is asked.
async fn turn(ask_before_changes: bool) -> (Vec<String>, bool) {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let work = dir.path().join("work");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&work).unwrap();
    std::fs::write(
        home.join("config.toml"),
        "[approval]\ndefault = \"allow\"\n\n[[rule]]\ntool = \"write\"\naction = \"allow\"\n",
    )
    .unwrap();
    let options = Options {
        sandbox: Some(SandboxMode::WorkspaceWrite),
        max_steps: 10,
        yes: true,
        ask_before_changes,
        llm: Some(Arc::new(ScriptedLlm::new(Recording {
            responses: vec![write_call(), ScriptedResponse::text("done")],
        }))),
        runner: Some(Arc::new(InProcess)),
        ..Options::default()
    };
    let (router, token) = hame_web::router(home, work.clone(), options);
    let created = call(&router, &token, "POST", "/api/sessions").await;
    let body = axum::body::to_bytes(created.into_body(), 4096)
        .await
        .unwrap();
    let session = serde_json::from_slice::<Value>(&body).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let events = call(
        &router,
        &token,
        "GET",
        &format!("/api/sessions/{session}/events"),
    )
    .await;
    let mut stream = events.into_body().into_data_stream();
    call(
        &router,
        &token,
        "POST",
        &format!("/api/sessions/{session}/prompt"),
    )
    .await;
    let mut kinds = Vec::new();
    let mut buffer = String::new();
    'outer: loop {
        let chunk = tokio::time::timeout(Duration::from_secs(20), stream.next())
            .await
            .expect("events in time")
            .expect("the stream stays open")
            .unwrap();
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(end) = buffer.find("\n\n") {
            let frame: String = buffer.drain(..end + 2).collect();
            let Some(data) = frame.lines().find_map(|l| l.strip_prefix("data: ")) else {
                continue;
            };
            let event: Value = serde_json::from_str(data).unwrap();
            let kind = event["kind"].as_str().unwrap_or("").to_string();
            let ended = kind == "approval" || (kind == "turn" && event["state"] == "ended");
            kinds.push(kind);
            if ended {
                break 'outer;
            }
        }
    }
    (kinds, work.join("a.txt").exists())
}

#[tokio::test]
async fn remote_sessions_ask_whatever_the_configuration_allows() {
    let (kinds, written) = turn(true).await;
    assert_eq!(kinds.last().map(String::as_str), Some("approval"));
    assert!(!written, "nothing runs before the answer");

    // Control: the same configuration lets the write run unasked locally.
    let (kinds, written) = turn(false).await;
    assert!(!kinds.iter().any(|k| k == "approval"));
    assert!(written);
}
