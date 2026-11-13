//! Bringing in sessions recorded by other harnesses.

use std::collections::BTreeMap;
use std::path::Path;

use hx_session::{
    AttemptReason, Block, CreateOptions, Event, SessionId, SessionStore, StopReason, TurnEndReason,
    Usage,
};
use serde_json::Value;

fn text_of(content: &Value) -> Vec<Block> {
    let mut blocks = Vec::new();
    for item in content.as_array().into_iter().flatten() {
        match item["type"].as_str() {
            Some("text") => blocks.push(Block::text(item["text"].as_str().unwrap_or(""))),
            Some(other) => blocks.push(Block::text(format!("[{other} omitted on import]"))),
            None => {}
        }
    }
    if blocks.is_empty() {
        blocks.push(Block::text(""));
    }
    blocks
}

fn joined(content: &Value) -> String {
    text_of(content)
        .into_iter()
        .filter_map(|block| match block {
            Block::Text { text } => Some(text),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Converts a DeepSeek Harness session log (JSONL, format v2 to v4) into a
/// new hx session, and returns its id.
///
/// The conversation carries over: system prompt, user and assistant
/// messages with their reasoning, tool calls and results, turn structure
/// and compaction summaries. Events with no counterpart here (approval
/// policy, titles, jobs, subagent bookkeeping) are left out. Anything the
/// source left unfinished is closed the way a crashed hx session is.
pub fn import_dsh(
    store: &SessionStore,
    source: &Path,
    cwd: Option<&Path>,
) -> Result<SessionId, String> {
    let text = std::fs::read_to_string(source)
        .map_err(|e| format!("cannot read {}: {e}", source.display()))?;
    let mut lines = text.lines().enumerate();
    let header: Value = lines
        .next()
        .and_then(|(_, line)| serde_json::from_str(line).ok())
        .filter(|header: &Value| header["type"] == "session")
        .ok_or_else(|| format!("{} is not a DeepSeek Harness session log", source.display()))?;
    let version = header["version"].as_u64().unwrap_or(0);
    if !(2..=4).contains(&version) {
        return Err(format!(
            "session format v{version} is not supported; v2 to v4 are"
        ));
    }

    let mut writer = store
        .create(CreateOptions {
            id: None,
            cwd: match cwd {
                Some(cwd) => cwd.to_string_lossy().into_owned(),
                None => header["cwd"].as_str().unwrap_or("").to_string(),
            },
        })
        .map_err(|e| e.to_string())?;
    let id = writer.id().clone();

    // Source event number -> the last hx record written for it or before
    // it, so a compaction's range can be carried over.
    let mut written_through: BTreeMap<usize, u64> = BTreeMap::new();
    for (index, line) in lines {
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            // A torn last line, as after a crash.
            continue;
        };
        let data = &record["data"];
        let number = |name: &str| data[name].as_u64().unwrap_or(0);
        let event = match record["type"].as_str().unwrap_or("") {
            "turn/start" => Some(Event::TurnStart {
                turn: number("turn"),
            }),
            "turn/end" => Some(Event::TurnEnd {
                turn: number("turn"),
                reason: match data["reason"]["kind"].as_str() {
                    Some("completed") => TurnEndReason::Completed,
                    Some("aborted") => TurnEndReason::Cancelled,
                    _ => TurnEndReason::Failed,
                },
            }),
            "step/start" => Some(Event::StepStart {
                turn: number("turn"),
                step: number("step"),
            }),
            "step/end" => Some(Event::StepEnd {
                turn: number("turn"),
                step: number("step"),
            }),
            "system/message" => Some(Event::SystemMessage {
                text: joined(&data["message"]["content"]),
            }),
            "user/message" | "developer/message" => Some(Event::UserMessage {
                content: text_of(&data["content"]),
            }),
            "assistant/message" => {
                let mut content = Vec::new();
                for block in data["message"]["content"].as_array().into_iter().flatten() {
                    match block["type"].as_str() {
                        Some("text") => {
                            content.push(Block::text(block["text"].as_str().unwrap_or("")));
                        }
                        Some("reasoning") => content.push(Block::Thinking {
                            text: block["text"].as_str().unwrap_or("").to_string(),
                            signature: None,
                        }),
                        Some("tool-call") => {
                            let raw = block["arguments"].as_str().unwrap_or("{}");
                            content.push(Block::ToolUse {
                                id: block["id"].as_str().unwrap_or("").to_string(),
                                name: block["name"].as_str().unwrap_or("").to_string(),
                                input: serde_json::from_str(raw)
                                    .unwrap_or_else(|_| Value::String(raw.to_string())),
                            });
                        }
                        _ => {}
                    }
                }
                let finish = data["stream"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .rev()
                    .find(|chunk| chunk["chunk"]["type"] == "finish")
                    .and_then(|chunk| chunk["chunk"]["reason"]["kind"].as_str());
                let calls_tools = content.iter().any(|b| matches!(b, Block::ToolUse { .. }));
                Some(Event::AssistantMessage {
                    content,
                    stop: match finish {
                        Some("max-tokens") => StopReason::MaxTokens,
                        _ if calls_tools => StopReason::ToolUse,
                        _ => StopReason::EndTurn,
                    },
                    usage: Usage::default(),
                })
            }
            "assistant/attempt" => Some(Event::AssistantAttempt {
                content: Vec::new(),
                reason: AttemptReason::Failed,
            }),
            "tool/call" => {
                let raw = data["arguments"].as_str().unwrap_or("{}");
                Some(Event::ToolCall {
                    call_id: data["callId"].as_str().unwrap_or("").to_string(),
                    name: data["name"].as_str().unwrap_or("").to_string(),
                    input: serde_json::from_str(raw)
                        .unwrap_or_else(|_| Value::String(raw.to_string())),
                })
            }
            "tool/result" => Some(Event::ToolResult {
                call_id: data["message"]["toolCallId"]
                    .as_str()
                    .unwrap_or("")
                    .to_string(),
                content: vec![Block::text(joined(&data["message"]["content"]))],
                is_error: data["message"]["isError"].as_bool().unwrap_or(false),
            }),
            "compaction/summary" => {
                let end = data["shadowedRange"]["end"].as_u64().unwrap_or(0) as usize;
                written_through
                    .range(..=end)
                    .next_back()
                    .map(|(_, seq)| Event::Compaction {
                        summary: joined(&data["summary"]),
                        up_to_seq: *seq,
                    })
            }
            _ => None,
        };
        if let Some(event) = event {
            let record = writer.append(event).map_err(|e| e.to_string())?;
            written_through.insert(index, record.seq);
        } else if let Some(last) = writer.next_seq().checked_sub(1) {
            written_through.insert(index, last);
        }
    }
    drop(writer);
    // Reopening closes anything the source left open.
    store.open(&id).map_err(|e| e.to_string())?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hx_session::{Role, derive_history, plan_recovery};
    use serde_json::json;

    #[test]
    fn a_dsh_log_becomes_a_session_the_model_can_continue() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("session.v4.jsonl");
        let assistant = |content: Value, finish: &str| {
            json!({"type": "assistant/message", "data": {"turn": 1, "step": 1,
                "message": {"role": "assistant", "content": content},
                "stream": [{"type": "chunk", "chunk": {"type": "finish", "reason": {"kind": finish}}}]}})
        };
        let lines = [
            json!({"type": "session", "version": 4, "id": "abc", "cwd": "/src/project"}),
            json!({"type": "sandbox/mode", "data": {"mode": "workspace-write"}}),
            json!({"type": "turn/start", "data": {"turn": 1}}),
            json!({"type": "step/start", "data": {"turn": 1, "step": 1}}),
            json!({"type": "system/message", "data": {"message": {"content": [{"type": "text", "text": "You are DSH."}]}}}),
            json!({"type": "user/message", "data": {"content": [{"type": "text", "text": "List files"}], "role": "user"}}),
            assistant(
                json!([{"type": "reasoning", "text": "need ls"},
                    {"type": "tool-call", "id": "t1", "name": "bash", "arguments": "{\"command\":\"ls\"}"}]),
                "tool-calls",
            ),
            json!({"type": "tool/call", "data": {"callId": "t1", "name": "bash", "arguments": "{\"command\":\"ls\"}"}}),
            json!({"type": "tool/result", "data": {"message": {"toolCallId": "t1",
                "content": [{"type": "text", "text": "a.rs"}, {"type": "image", "data": "..."}], "isError": false}}}),
            json!({"type": "step/end", "data": {"turn": 1, "step": 1}}),
            json!({"type": "step/start", "data": {"turn": 1, "step": 2}}),
            // The source stopped here, mid-step: a second call never answered.
            assistant(
                json!([{"type": "tool-call", "id": "t2", "name": "bash", "arguments": "{\"command\":\"cat a.rs\"}"}]),
                "tool-calls",
            ),
        ];
        let text: String = lines.iter().map(|line| format!("{line}\n")).collect();
        std::fs::write(&source, text + "{\"type\":\"tool/ca").unwrap();

        let store = SessionStore::new(dir.path().join("sessions"));
        let id = import_dsh(&store, &source, None).unwrap();
        let records = store.read(&id).unwrap();

        let Event::Session(header) = &records[0].event else {
            panic!("header first");
        };
        assert_eq!(header.cwd, "/src/project");
        assert!(plan_recovery(&records).is_empty(), "nothing is left open");
        let history = derive_history(&records);
        assert_eq!(history.system.as_deref(), Some("You are DSH."));
        let roles: Vec<Role> = history.messages.iter().map(|m| m.role).collect();
        assert_eq!(
            roles,
            [
                Role::User,
                Role::Assistant,
                Role::Tool,
                Role::Assistant,
                Role::Tool
            ]
        );
        assert!(history.dangling_tool_calls().is_empty());
        assert_eq!(
            history.messages[1].content,
            vec![
                Block::Thinking {
                    text: "need ls".into(),
                    signature: None
                },
                Block::ToolUse {
                    id: "t1".into(),
                    name: "bash".into(),
                    input: json!({"command": "ls"})
                },
            ]
        );
        assert!(matches!(&history.messages[2].content[0],
            Block::ToolResult { content, .. }
                if content == &vec![Block::text("a.rs\n[image omitted on import]")]));

        assert!(import_dsh(&store, &dir.path().join("missing"), None).is_err());
        std::fs::write(&source, "{\"type\":\"session\",\"version\":9}\n").unwrap();
        assert!(
            import_dsh(&store, &source, None)
                .unwrap_err()
                .contains("v9")
        );
    }
}
