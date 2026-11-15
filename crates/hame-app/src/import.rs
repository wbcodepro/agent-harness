//! Bringing in sessions recorded by other harnesses.

use std::collections::BTreeMap;
use std::path::Path;

use base64::Engine as _;
use hame_session::{
    ArtifactStore, AttemptReason, Block, CreateOptions, Event, SessionId, SessionStore, StopReason,
    TurnEndReason, Usage,
};
use serde_json::Value;

/// An image block of an imported transcript, stored in the artifact store.
///
/// Understands the shapes transcripts carry images in: `data` with
/// `mimeType`/`mediaType`/`mime_type` beside it, an Anthropic-style
/// `source: {type: "base64", media_type, data}`, and an OpenAI-style
/// `image_url` holding a `data:` URL. An image that cannot be decoded or
/// stored becomes an `unsupported` block saying so, never silence.
pub fn import_image(item: &Value, store: &ArtifactStore) -> Block {
    let source = &item["source"];
    let url = item["image_url"]["url"]
        .as_str()
        .or_else(|| item["image_url"].as_str())
        .or_else(|| item["url"].as_str());
    let (mime, data) = if let Some(data) = item["data"].as_str() {
        let mime = ["mimeType", "mediaType", "mime_type", "media_type"]
            .iter()
            .find_map(|key| item[*key].as_str())
            .unwrap_or("image/png");
        (mime.to_string(), data.to_string())
    } else if source["type"] == "base64" {
        (
            source["media_type"]
                .as_str()
                .unwrap_or("image/png")
                .to_string(),
            source["data"].as_str().unwrap_or("").to_string(),
        )
    } else if let Some(rest) = url.and_then(|url| url.strip_prefix("data:")) {
        match rest.split_once(";base64,") {
            Some((mime, data)) => (mime.to_string(), data.to_string()),
            None => (String::new(), String::new()),
        }
    } else {
        return Block::Unsupported {
            kind: "image".into(),
            note: "the imported log refers to an image by location, not by its bytes".into(),
        };
    };
    let bytes = match base64::engine::general_purpose::STANDARD.decode(data.trim()) {
        Ok(bytes) if !bytes.is_empty() => bytes,
        _ => {
            return Block::Unsupported {
                kind: "image".into(),
                note: "the imported log holds image data that is not valid base64".into(),
            };
        }
    };
    match store.put(&bytes, &mime) {
        Ok(artifact) if artifact.is_image() => Block::Image { artifact },
        Ok(artifact) => Block::Artifact { artifact },
        Err(e) => Block::Unsupported {
            kind: "image".into(),
            note: format!("could not be stored on import: {e}"),
        },
    }
}

/// The content blocks of a message: its text parts as one text block, as
/// before, followed by its images, stored in the artifact store. Parts of
/// other kinds are named in the text.
fn content_of(content: &Value, store: &ArtifactStore) -> Vec<Block> {
    let mut texts = Vec::new();
    let mut media = Vec::new();
    for item in content.as_array().into_iter().flatten() {
        match item["type"].as_str() {
            Some("text") => texts.push(item["text"].as_str().unwrap_or("").to_string()),
            Some("image" | "image_url" | "input_image") => media.push(import_image(item, store)),
            Some(other) => texts.push(format!("[{other} omitted on import]")),
            None => {}
        }
    }
    let mut blocks = Vec::new();
    if !texts.is_empty() || media.is_empty() {
        blocks.push(Block::text(texts.join("\n")));
    }
    blocks.extend(media);
    blocks
}

fn joined(content: &Value) -> String {
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| match item["type"].as_str() {
            Some("text") => Some(item["text"].as_str().unwrap_or("").to_string()),
            Some(other) => Some(format!("[{other} omitted on import]")),
            None => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Converts a DeepSeek Harness session log (JSONL, format v2 to v4) into a
/// new hame session, and returns its id.
///
/// The conversation carries over: system prompt, user and assistant
/// messages with their reasoning, tool calls and results, turn structure
/// and compaction summaries. Images in messages and tool results are stored
/// in the artifact store and kept as image blocks. Events with no
/// counterpart here (approval policy, titles, jobs, subagent bookkeeping)
/// are left out. Anything the source left unfinished is closed the way a
/// crashed hame session is.
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
    let artifacts = writer.artifacts().clone();

    // Source event number -> the last hame record written for it or before
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
                content: content_of(&data["content"], &artifacts),
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
                content: content_of(&data["message"]["content"], &artifacts),
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
                        usage: Default::default(),
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
    use hame_session::{Role, derive_history, plan_recovery};
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
        // An image whose data is not valid is kept as visibly unsupported.
        assert!(matches!(&history.messages[2].content[0],
            Block::ToolResult { content, .. }
                if matches!(&content[..], [Block::Text { text }, Block::Unsupported { kind, .. }]
                    if text == "a.rs" && kind == "image")));

        assert!(import_dsh(&store, &dir.path().join("missing"), None).is_err());

        // A real image, base64 in the source, is stored and referenced.
        let png: &[u8] = &[
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0x0d, 0x49, 0x48, 0x44, 0x52,
            0, 0, 0, 2, 0, 0, 0, 2, 8, 6, 0, 0, 0,
        ];
        let encoded = base64::engine::general_purpose::STANDARD.encode(png);
        let lines = [
            json!({"type": "session", "version": 4, "id": "img", "cwd": "/src"}),
            json!({"type": "turn/start", "data": {"turn": 1}}),
            json!({"type": "user/message", "data": {"content": [
                {"type": "text", "text": "What is this?"},
                {"type": "image", "data": encoded, "mimeType": "image/png"}]}}),
            json!({"type": "assistant/message", "data": {"message": {"content": [
                {"type": "tool-call", "id": "t1", "name": "screenshot", "arguments": "{}"}]}}}),
            json!({"type": "tool/result", "data": {"message": {"toolCallId": "t1", "content": [
                {"type": "image", "source": {"type": "base64", "media_type": "image/png",
                    "data": encoded}}]}}}),
            json!({"type": "turn/end", "data": {"turn": 1, "reason": {"kind": "completed"}}}),
        ];
        let text: String = lines.iter().map(|line| format!("{line}\n")).collect();
        std::fs::write(&source, text).unwrap();
        let id = import_dsh(&store, &source, None).unwrap();
        let records = store.read(&id).unwrap();
        let images = hame_session::session_artifacts(&records);
        assert_eq!(images.len(), 1, "the same bytes are stored once");
        assert_eq!((images[0].width, images[0].height), (Some(2), Some(2)));
        assert_eq!(store.artifacts().get(&images[0]).unwrap(), png);
        let history = derive_history(&records);
        assert!(matches!(&history.messages[0].content[..],
            [Block::Text { text }, Block::Image { .. }] if text == "What is this?"));
        assert!(matches!(&history.messages[2].content[0],
            Block::ToolResult { content, .. } if matches!(&content[..], [Block::Image { .. }])));
        assert!(store.check_artifacts(&id).unwrap().is_empty());
        std::fs::write(&source, "{\"type\":\"session\",\"version\":9}\n").unwrap();
        assert!(
            import_dsh(&store, &source, None)
                .unwrap_err()
                .contains("v9")
        );
    }
}
