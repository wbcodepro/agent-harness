use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use hx_app::{Options, SandboxMode};
use hx_eval::{Recording, ScriptedLlm, ScriptedResponse};
use hx_session::{Delta, StopReason, Usage};
use hx_tools::InProcess;
use serde_json::{Value, json};
use tokio::io::{
    AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines, ReadHalf, WriteHalf,
};

struct Client {
    to_server: WriteHalf<DuplexStream>,
    from_server: Lines<BufReader<ReadHalf<DuplexStream>>>,
}

impl Client {
    async fn send(&mut self, message: Value) {
        self.to_server
            .write_all(format!("{message}\n").as_bytes())
            .await
            .unwrap();
    }

    async fn next(&mut self) -> Value {
        let line = tokio::time::timeout(Duration::from_secs(20), self.from_server.next_line())
            .await
            .expect("the server answers in time")
            .unwrap()
            .expect("the server is still connected");
        serde_json::from_str(&line).unwrap()
    }

    /// Reads until the response to request `id`, returning it and every
    /// message that came before it.
    async fn until_response(&mut self, id: u64) -> (Value, Vec<Value>) {
        let mut before = Vec::new();
        loop {
            let message = self.next().await;
            if message["id"] == id && message.get("method").is_none() {
                return (message, before);
            }
            before.push(message);
        }
    }
}

/// Starts a server whose model replays `responses`, with unconfined
/// in-process tools so approval is what gates them.
fn start(home: &Path, responses: Vec<ScriptedResponse>, delay_ms: u64) -> Client {
    let options = Options {
        sandbox: Some(SandboxMode::DangerFullAccess),
        max_steps: 10,
        llm: Some(Arc::new(
            ScriptedLlm::new(Recording { responses }).with_delay(Duration::from_millis(delay_ms)),
        )),
        runner: Some(Arc::new(InProcess)),
        ..Options::default()
    };
    let (client_side, server_side) = tokio::io::duplex(1 << 16);
    let (server_read, server_write) = tokio::io::split(server_side);
    tokio::spawn(hx_acp::serve(
        home.to_path_buf(),
        options,
        server_read,
        server_write,
    ));
    let (client_read, client_write) = tokio::io::split(client_side);
    Client {
        to_server: client_write,
        from_server: BufReader::new(client_read).lines(),
    }
}

fn write_call(path: &str, content: &str) -> ScriptedResponse {
    ScriptedResponse {
        deltas: vec![
            Delta::ToolUseStart {
                id: "c1".into(),
                name: "write".into(),
            },
            Delta::ToolInput {
                json: json!({"path": path, "content": content}).to_string(),
            },
        ],
        stop: StopReason::ToolUse,
        usage: Usage::default(),
    }
}

async fn new_session(client: &mut Client, cwd: &Path) -> String {
    client
        .send(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": 1, "clientCapabilities": {}}}))
        .await;
    let (initialized, _) = client.until_response(1).await;
    assert_eq!(initialized["result"]["protocolVersion"], 1);
    assert_eq!(initialized["result"]["agentInfo"]["name"], "hx");

    client
        .send(json!({"jsonrpc": "2.0", "id": 2, "method": "session/new",
            "params": {"cwd": cwd, "mcpServers": []}}))
        .await;
    let (created, _) = client.until_response(2).await;
    created["result"]["sessionId"].as_str().unwrap().to_string()
}

fn prompt(id: u64, session: &str, text: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "session/prompt",
        "params": {"sessionId": session, "prompt": [{"type": "text", "text": text}]}})
}

#[tokio::test]
async fn a_prompt_streams_updates_and_asks_permission_for_changes() {
    let dir = tempfile::tempdir().unwrap();
    let (home, work) = (dir.path().join("home"), dir.path().join("work"));
    std::fs::create_dir_all(&work).unwrap();
    let mut client = start(
        &home,
        vec![
            write_call("a.txt", "hello"),
            write_call("b.txt", "nope"),
            ScriptedResponse::text("Wrote a.txt; b.txt was refused."),
        ],
        0,
    );
    let session = new_session(&mut client, &work).await;
    client.send(prompt(3, &session, "Write two files")).await;

    // First write: the editor is asked, and allows it.
    let mut seen = Vec::new();
    let ask = loop {
        let message = client.next().await;
        if message["method"] == "session/request_permission" {
            break message;
        }
        seen.push(message);
    };
    assert_eq!(ask["params"]["sessionId"], session.as_str());
    assert_eq!(ask["params"]["toolCall"]["toolCallId"], "c1");
    assert_eq!(
        seen.last().unwrap()["params"]["update"]["sessionUpdate"],
        "tool_call"
    );
    assert_eq!(seen.last().unwrap()["params"]["update"]["kind"], "edit");
    assert!(
        !work.join("a.txt").exists(),
        "nothing runs before the answer"
    );
    client
        .send(json!({"jsonrpc": "2.0", "id": ask["id"],
            "result": {"outcome": {"outcome": "selected", "optionId": "allow-once"}}}))
        .await;

    // Second write: refused.
    let ask = loop {
        let message = client.next().await;
        if message["method"] == "session/request_permission" {
            break message;
        }
        seen.push(message);
    };
    client
        .send(json!({"jsonrpc": "2.0", "id": ask["id"],
            "result": {"outcome": {"outcome": "selected", "optionId": "reject-once"}}}))
        .await;

    let (done, rest) = client.until_response(3).await;
    seen.extend(rest);
    assert_eq!(done["result"]["stopReason"], "end_turn", "{done}");
    assert_eq!(
        std::fs::read_to_string(work.join("a.txt")).ok().as_deref(),
        Some("hello"),
        "{seen:#?}"
    );
    assert!(!work.join("b.txt").exists());

    let updates: Vec<(&str, &Value)> = seen
        .iter()
        .filter(|m| m["method"] == "session/update")
        .map(|m| {
            let update = &m["params"]["update"];
            (update["sessionUpdate"].as_str().unwrap(), update)
        })
        .collect();
    let kinds: Vec<&str> = updates.iter().map(|(kind, _)| *kind).collect();
    assert_eq!(
        kinds,
        [
            "tool_call",
            "tool_call_update",
            "tool_call",
            "tool_call_update",
            "agent_message_chunk"
        ]
    );
    assert_eq!(updates[1].1["status"], "completed");
    assert_eq!(updates[3].1["status"], "failed");
    assert_eq!(
        updates[4].1["content"]["text"],
        "Wrote a.txt; b.txt was refused."
    );
}

#[tokio::test]
async fn a_turn_can_be_cancelled_and_the_session_loaded_again() {
    let dir = tempfile::tempdir().unwrap();
    let (home, work) = (dir.path().join("home"), dir.path().join("work"));
    std::fs::create_dir_all(&work).unwrap();
    let slow = ScriptedResponse {
        deltas: (0..500)
            .map(|n| Delta::Text {
                text: format!("w{n} "),
            })
            .collect(),
        stop: StopReason::EndTurn,
        usage: Usage::default(),
    };
    let mut client = start(
        &home,
        vec![slow, ScriptedResponse::text("Second answer.")],
        10,
    );
    let session = new_session(&mut client, &work).await;

    client.send(prompt(3, &session, "Talk")).await;
    // Wait for some output, then cancel.
    loop {
        if client.next().await["params"]["update"]["sessionUpdate"] == "agent_message_chunk" {
            break;
        }
    }
    client
        .send(json!({"jsonrpc": "2.0", "method": "session/cancel",
            "params": {"sessionId": session}}))
        .await;
    let (cancelled, _) = client.until_response(3).await;
    assert_eq!(cancelled["result"]["stopReason"], "cancelled");

    // The session is still usable.
    client.send(prompt(4, &session, "Again")).await;
    let (done, _) = client.until_response(4).await;
    assert_eq!(done["result"]["stopReason"], "end_turn");

    // A fresh connection can load it and gets the conversation replayed.
    drop(client);
    let mut client = start(&home, vec![], 0);
    client
        .send(json!({"jsonrpc": "2.0", "id": 1, "method": "session/load",
            "params": {"sessionId": session, "cwd": work, "mcpServers": []}}))
        .await;
    let (loaded, replayed) = client.until_response(1).await;
    assert!(loaded.get("error").is_none(), "{loaded}");
    let texts: Vec<(&str, &str)> = replayed
        .iter()
        .map(|m| {
            let update = &m["params"]["update"];
            (
                update["sessionUpdate"].as_str().unwrap(),
                update["content"]["text"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        texts,
        [
            ("user_message_chunk", "Talk"),
            ("user_message_chunk", "Again"),
            ("agent_message_chunk", "Second answer."),
        ]
    );

    client
        .send(json!({"jsonrpc": "2.0", "id": 9, "method": "no/such", "params": {}}))
        .await;
    let (unknown, _) = client.until_response(9).await;
    assert_eq!(unknown["error"]["code"], -32601);
}
