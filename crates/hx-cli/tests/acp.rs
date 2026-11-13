//! The ACP server through the real binary: stdin/stdout framing and the
//! default sandbox.
#![cfg(unix)]

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use hx_eval::{Recording, ScriptedResponse};
use hx_session::{Delta, StopReason, Usage};
use serde_json::{Value, json};

#[test]
fn an_editor_can_run_a_confined_turn_over_stdio() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let (home, work) = (root.join("home"), root.join("work"));
    fs::create_dir_all(&work).unwrap();
    fs::create_dir_all(&home).unwrap();
    fs::write(
        home.join("config.toml"),
        "[sandbox]\nwritable_tmp = false\n",
    )
    .unwrap();
    let call = |id: &str, path: String| ScriptedResponse {
        deltas: vec![
            Delta::ToolUseStart {
                id: id.into(),
                name: "write".into(),
            },
            Delta::ToolInput {
                json: json!({"path": path, "content": "x"}).to_string(),
            },
        ],
        stop: StopReason::ToolUse,
        usage: Usage::default(),
    };
    let script = root.join("script.jsonl");
    Recording {
        responses: vec![
            call("in", "inside.txt".into()),
            call("out", root.join("outside.txt").display().to_string()),
            ScriptedResponse::text("One written, one blocked."),
        ],
    }
    .save(&script)
    .unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_hx"))
        .arg("--home")
        .arg(&home)
        .arg("acp")
        .env("HX_SCRIPT", &script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut request = |message: Value| writeln!(stdin, "{message}").unwrap();
    let mut response_to = |id: u64| -> (Value, Vec<Value>) {
        let mut before = Vec::new();
        loop {
            let message: Value = serde_json::from_str(&stdout.next().unwrap().unwrap()).unwrap();
            if message["id"] == id {
                return (message, before);
            }
            before.push(message);
        }
    };

    request(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": 1}}));
    assert_eq!(response_to(1).0["result"]["protocolVersion"], 1);
    request(json!({"jsonrpc": "2.0", "id": 2, "method": "session/new",
        "params": {"cwd": work, "mcpServers": []}}));
    let session = response_to(2).0["result"]["sessionId"]
        .as_str()
        .unwrap()
        .to_string();
    request(
        json!({"jsonrpc": "2.0", "id": 3, "method": "session/prompt",
        "params": {"sessionId": session, "prompt": [{"type": "text", "text": "write"}]}}),
    );
    let (done, updates) = response_to(3);

    assert_eq!(done["result"]["stopReason"], "end_turn");
    let statuses: Vec<&str> = updates
        .iter()
        .filter_map(|m| m["params"]["update"]["status"].as_str())
        .collect();
    // Confined writes need no permission round trip; the sandbox decides.
    assert_eq!(
        statuses,
        ["in_progress", "completed", "in_progress", "failed"]
    );
    assert!(work.join("inside.txt").exists());
    assert!(!root.join("outside.txt").exists());

    drop(request);
    drop(stdin);
    assert!(
        child.wait().unwrap().success(),
        "the server exits when stdin closes"
    );
}
