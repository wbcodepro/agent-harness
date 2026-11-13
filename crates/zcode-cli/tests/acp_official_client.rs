//! Drives the real `zcode acp` binary with the official Agent Client Protocol
//! client runtime: the same connection code an editor built on the SDK
//! uses, spawning the agent as a subprocess and talking to it over stdio.
#![cfg(unix)]

use std::fs;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::v1::ConnectionTo;
use agent_client_protocol::v1::schema::{
    ContentBlock, InitializeRequest, NewSessionRequest, PermissionOptionKind, PromptRequest,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, SessionNotification, SessionUpdate, StopReason, TextContent,
};
use agent_client_protocol::{AcpAgent, Agent};
use serde_json::json;
use zcode_eval::{Recording, ScriptedResponse};
use zcode_session::{Delta, Usage};

#[tokio::test(flavor = "multi_thread")]
async fn the_official_client_runtime_can_run_a_turn_with_permission() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let (home, work) = (root.join("home"), root.join("work"));
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&work).unwrap();
    // Ask before anything that is not read-only, so the permission round
    // trip is exercised even though the tools are confined.
    fs::write(home.join("config.toml"), "[approval]\ndefault = \"ask\"\n").unwrap();
    let script = root.join("script.jsonl");
    Recording {
        responses: vec![
            ScriptedResponse {
                deltas: vec![
                    Delta::Text {
                        text: "Writing the file.".into(),
                    },
                    Delta::ToolUseStart {
                        id: "call-1".into(),
                        name: "write".into(),
                    },
                    Delta::ToolInput {
                        json: json!({"path": "from-editor.txt", "content": "hello"}).to_string(),
                    },
                ],
                stop: zcode_session::StopReason::ToolUse,
                usage: Usage::default(),
            },
            ScriptedResponse::text("Done."),
        ],
    }
    .save(&script)
    .unwrap();

    let command = format!(
        "env ZCODE_SCRIPT={} {} --home {} acp",
        script.display(),
        env!("CARGO_BIN_EXE_zcode"),
        home.display()
    );
    let agent = AcpAgent::from_str(&command).unwrap();

    let updates: Arc<Mutex<Vec<&'static str>>> = Arc::default();
    let asked: Arc<Mutex<Vec<String>>> = Arc::default();
    let (seen, questions, cwd) = (Arc::clone(&updates), Arc::clone(&asked), work.clone());

    let stop_reason = agent_client_protocol::Client
        .v1()
        .on_receive_notification(
            async move |notification: SessionNotification, _cx| {
                seen.lock().unwrap().push(match notification.update {
                    SessionUpdate::AgentMessageChunk(_) => "message",
                    SessionUpdate::ToolCall(_) => "tool_call",
                    SessionUpdate::ToolCallUpdate(_) => "tool_call_update",
                    _ => "other",
                });
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async move |request: RequestPermissionRequest, responder, _connection| {
                questions
                    .lock()
                    .unwrap()
                    .push(format!("{:?}", request.tool_call.tool_call_id));
                let allow = request
                    .options
                    .iter()
                    .find(|option| option.kind == PermissionOptionKind::AllowOnce)
                    .map(|option| option.option_id.clone())
                    .expect("an allow-once option is offered");
                responder.respond(RequestPermissionResponse::new(
                    RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(allow)),
                ))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(agent, |connection: ConnectionTo<Agent>| async move {
            let initialized = connection
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            assert_eq!(initialized.protocol_version, ProtocolVersion::V1);
            let session = connection
                .send_request(NewSessionRequest::new(cwd))
                .block_task()
                .await?;
            let finished = connection
                .send_request(PromptRequest::new(
                    session.session_id.clone(),
                    vec![ContentBlock::Text(TextContent::new("Write the file."))],
                ))
                .block_task()
                .await?;
            Ok(finished.stop_reason)
        })
        .await
        .expect("the official client completes the exchange");

    assert!(matches!(stop_reason, StopReason::EndTurn));
    assert_eq!(
        fs::read_to_string(work.join("from-editor.txt")).unwrap(),
        "hello"
    );
    assert_eq!(asked.lock().unwrap().len(), 1, "the write asked once");
    assert_eq!(
        *updates.lock().unwrap(),
        ["message", "tool_call", "tool_call_update", "message"]
    );
}
