//! Checks the wire format against the Agent Client Protocol's own type
//! definitions: requests are built from the official types, and everything
//! the server sends must deserialize into them.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::v1::schema::{
    InitializeRequest, InitializeResponse, LoadSessionRequest, NewSessionRequest,
    NewSessionResponse, PromptRequest, PromptResponse, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse, SelectedPermissionOutcome,
    SessionNotification, SessionUpdate, StopReason,
};
use hx_app::{Options, SandboxMode};
use hx_eval::{Recording, ScriptedLlm, ScriptedResponse};
use hx_session::{Delta, Usage};
use hx_tools::InProcess;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::io::{
    AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines, ReadHalf, WriteHalf,
};

struct Editor {
    to_agent: WriteHalf<DuplexStream>,
    from_agent: Lines<BufReader<ReadHalf<DuplexStream>>>,
    next_id: u64,
    /// Every notification and request the agent sent, typed.
    updates: Vec<SessionNotification>,
    permissions: Vec<RequestPermissionRequest>,
}

fn typed<T: DeserializeOwned>(what: &str, value: &Value) -> T {
    serde_json::from_value(value.clone())
        .unwrap_or_else(|e| panic!("{what} does not match the protocol schema: {e}\n{value:#}"))
}

impl Editor {
    fn connect(home: &Path, responses: Vec<ScriptedResponse>) -> Self {
        let options = Options {
            sandbox: Some(SandboxMode::DangerFullAccess),
            max_steps: 10,
            llm: Some(Arc::new(ScriptedLlm::new(Recording { responses }))),
            runner: Some(Arc::new(InProcess)),
            ..Options::default()
        };
        let (editor_side, agent_side) = tokio::io::duplex(1 << 16);
        let (agent_read, agent_write) = tokio::io::split(agent_side);
        tokio::spawn(hx_acp::serve(
            home.to_path_buf(),
            options,
            agent_read,
            agent_write,
        ));
        let (read, write) = tokio::io::split(editor_side);
        Self {
            to_agent: write,
            from_agent: BufReader::new(read).lines(),
            next_id: 0,
            updates: Vec::new(),
            permissions: Vec::new(),
        }
    }

    async fn send(&mut self, message: Value) {
        self.to_agent
            .write_all(format!("{message}\n").as_bytes())
            .await
            .unwrap();
    }

    /// Sends a request built from the official type and returns the typed
    /// response, answering permission requests with `allow` on the way.
    async fn call<P: Serialize, R: DeserializeOwned>(
        &mut self,
        method: &str,
        params: &P,
        allow: bool,
    ) -> R {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await;
        loop {
            let line = tokio::time::timeout(Duration::from_secs(20), self.from_agent.next_line())
                .await
                .expect("the agent answers in time")
                .unwrap()
                .expect("the agent is connected");
            let message: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(message["jsonrpc"], "2.0");
            match message["method"].as_str() {
                Some("session/update") => {
                    self.updates
                        .push(typed("session/update", &message["params"]));
                }
                Some("session/request_permission") => {
                    let request: RequestPermissionRequest =
                        typed("session/request_permission", &message["params"]);
                    let wanted = if allow { "allow" } else { "reject" };
                    let option = request
                        .options
                        .iter()
                        .find(|option| format!("{:?}", option.kind).to_lowercase().contains(wanted))
                        .expect("an option of the wanted kind is offered")
                        .option_id
                        .clone();
                    self.permissions.push(request);
                    let response = RequestPermissionResponse::new(
                        RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(option)),
                    );
                    self.send(json!({"jsonrpc": "2.0", "id": message["id"], "result": response}))
                        .await;
                }
                Some(other) => panic!("unexpected method from the agent: {other}"),
                None => {
                    assert_eq!(message["id"], id, "{message}");
                    assert!(message.get("error").is_none(), "{message}");
                    return typed(method, &message["result"]);
                }
            }
        }
    }
}

fn write_call(path: &str) -> ScriptedResponse {
    ScriptedResponse {
        deltas: vec![
            Delta::Thinking {
                text: "I should write the file.".into(),
            },
            Delta::Text {
                text: "Writing.".into(),
            },
            Delta::ToolUseStart {
                id: "call-1".into(),
                name: "write".into(),
            },
            Delta::ToolInput {
                json: json!({"path": path, "content": "x"}).to_string(),
            },
        ],
        stop: hx_session::StopReason::ToolUse,
        usage: Usage::default(),
    }
}

#[tokio::test]
async fn every_message_matches_the_official_protocol_types() {
    let dir = tempfile::tempdir().unwrap();
    let (home, work) = (dir.path().join("home"), dir.path().join("work"));
    std::fs::create_dir_all(&work).unwrap();
    let mut editor = Editor::connect(
        &home,
        vec![write_call("a.txt"), ScriptedResponse::text("Done.")],
    );

    let initialized: InitializeResponse = editor
        .call(
            "initialize",
            &InitializeRequest::new(ProtocolVersion::V1),
            true,
        )
        .await;
    assert_eq!(initialized.protocol_version, ProtocolVersion::V1);
    assert!(initialized.agent_capabilities.load_session);

    let created: NewSessionResponse = editor
        .call("session/new", &NewSessionRequest::new(&work), true)
        .await;
    let session = created.session_id.clone();

    let prompt = PromptRequest::new(session.clone(), vec!["Write a.txt".into()]);
    let finished: PromptResponse = editor.call("session/prompt", &prompt, true).await;
    assert!(matches!(finished.stop_reason, StopReason::EndTurn));
    assert!(work.join("a.txt").exists());

    // The stream carried a thought, text, a tool call and its completion,
    // all for this session.
    assert_eq!(editor.permissions.len(), 1);
    assert_eq!(editor.permissions[0].session_id, session);
    assert!(
        editor
            .updates
            .iter()
            .all(|update| update.session_id == session)
    );
    let kinds: Vec<&str> = editor
        .updates
        .iter()
        .map(|update| match &update.update {
            SessionUpdate::AgentThoughtChunk(_) => "thought",
            SessionUpdate::AgentMessageChunk(_) => "message",
            SessionUpdate::ToolCall(_) => "tool_call",
            SessionUpdate::ToolCallUpdate(_) => "tool_call_update",
            SessionUpdate::UserMessageChunk(_) => "user",
            _ => "other",
        })
        .collect();
    assert_eq!(
        kinds,
        [
            "thought",
            "message",
            "tool_call",
            "tool_call_update",
            "message"
        ]
    );

    // Once the first editor disconnects and its lock is released, a second
    // one can load the session; the replay is typed too.
    drop(editor);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut second = Editor::connect(&home, vec![]);
    let _: InitializeResponse = second
        .call(
            "initialize",
            &InitializeRequest::new(ProtocolVersion::V1),
            true,
        )
        .await;
    let load = LoadSessionRequest::new(session.clone(), &work);
    let _: Value = second.call("session/load", &load, true).await;
    assert!(
        second
            .updates
            .iter()
            .any(|update| matches!(update.update, SessionUpdate::UserMessageChunk(_)))
    );
}
