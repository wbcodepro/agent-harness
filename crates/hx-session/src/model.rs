use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::SessionError;

/// Version of the on-disk log format this build reads and writes.
pub const FORMAT_VERSION: u32 = 1;

/// Identifies a session. Restricted to `[A-Za-z0-9_-]{1,64}` because it is
/// used as a directory name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SessionId(String);

impl SessionId {
    /// A fresh id that sorts by creation time.
    pub fn generate() -> Self {
        Self(uuid::Uuid::now_v7().simple().to_string())
    }

    pub fn parse(raw: &str) -> Result<Self, SessionError> {
        let valid = (1..=64).contains(&raw.len())
            && raw
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
        if valid {
            Ok(Self(raw.to_string()))
        } else {
            Err(SessionError::InvalidId(raw.to_string()))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for SessionId {
    type Error = SessionError;

    fn try_from(raw: String) -> Result<Self, Self::Error> {
        Self::parse(&raw)
    }
}

impl From<SessionId> for String {
    fn from(id: SessionId) -> Self {
        id.0
    }
}

/// One line of the log: a sequence number, a wall-clock timestamp in
/// milliseconds, and the event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub seq: u64,
    pub ts: u64,
    #[serde(flatten)]
    pub event: Event,
}

/// A durable fact about a session.
///
/// Everything the model is shown must be derivable from these events; see
/// [`crate::derive_history`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum Event {
    /// Always the first record, and only the first.
    #[serde(rename = "session")]
    Session(Header),
    #[serde(rename = "turn/start")]
    TurnStart { turn: u64 },
    #[serde(rename = "turn/end")]
    TurnEnd { turn: u64, reason: TurnEndReason },
    #[serde(rename = "step/start")]
    StepStart { turn: u64, step: u64 },
    #[serde(rename = "step/end")]
    StepEnd { turn: u64, step: u64 },
    /// Replaces the system prompt. Empty text clears it.
    #[serde(rename = "system/message")]
    SystemMessage { text: String },
    /// Replaces the set of tools offered to the model.
    #[serde(rename = "tools/declared")]
    ToolsDeclared { tools: Vec<ToolSpec> },
    #[serde(rename = "user/message")]
    UserMessage { content: Vec<Block> },
    /// One streamed chunk of the response in flight. Journalled so a crash
    /// mid-stream keeps what had arrived; superseded by the
    /// `assistant/message` or `assistant/attempt` that settles the stream.
    #[serde(rename = "assistant/delta")]
    AssistantDelta { delta: Delta },
    #[serde(rename = "assistant/message")]
    AssistantMessage {
        content: Vec<Block>,
        stop: StopReason,
        #[serde(default)]
        usage: Usage,
    },
    /// A response that did not complete. Kept for inspection; never part of
    /// model history.
    #[serde(rename = "assistant/attempt")]
    AssistantAttempt {
        content: Vec<Block>,
        reason: AttemptReason,
    },
    /// Logged before a tool starts executing.
    #[serde(rename = "tool/call")]
    ToolCall {
        call_id: String,
        name: String,
        input: Value,
    },
    #[serde(rename = "tool/result")]
    ToolResult {
        call_id: String,
        content: Vec<Block>,
        #[serde(default)]
        is_error: bool,
    },
    /// Model history up to and including `up_to_seq` is represented by
    /// `summary` from here on. The earlier records stay in the log.
    #[serde(rename = "compaction")]
    Compaction { summary: String, up_to_seq: u64 },
    /// The workspace as it was at this point is saved under `commit` in the
    /// session's checkpoint store.
    #[serde(rename = "checkpoint")]
    Checkpoint { commit: String },
}

impl Event {
    /// Whether the event must reach stable storage before `append` returns.
    /// Stream deltas are written to the OS immediately but not fsynced: they
    /// survive a process crash, and losing a few to power failure only
    /// shortens a response that would be discarded anyway.
    pub fn is_settled(&self) -> bool {
        !matches!(self, Event::AssistantDelta { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Header {
    pub version: u32,
    pub id: SessionId,
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<ParentRef>,
}

/// A tool as the model is told about it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// JSON Schema of the tool's input object.
    pub input_schema: Value,
}

/// Where a forked session branched from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParentRef {
    pub session: SessionId,
    pub seq: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnEndReason {
    Completed,
    Cancelled,
    Failed,
    /// The process stopped mid-turn; written by recovery.
    Interrupted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptReason {
    Cancelled,
    Failed,
    Interrupted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_read_tokens: u64,
    #[serde(default)]
    pub cache_write_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Block {
    Text {
        text: String,
    },
    Thinking {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        call_id: String,
        content: Vec<Block>,
        #[serde(default)]
        is_error: bool,
    },
}

impl Block {
    pub fn text(text: impl Into<String>) -> Self {
        Block::Text { text: text.into() }
    }
}

/// A streamed fragment of an assistant response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Delta {
    Text {
        text: String,
    },
    Thinking {
        text: String,
    },
    /// Opens a tool call; following `ToolInput` deltas belong to it.
    ToolUseStart {
        id: String,
        name: String,
    },
    /// A fragment of the open tool call's JSON input.
    ToolInput {
        json: String,
    },
}

/// Folds stream deltas into content blocks.
///
/// Tool input that is not valid JSON (a stream cut short) is kept as a JSON
/// string holding the raw fragment, so nothing that arrived is dropped.
pub fn assemble<'a>(deltas: impl IntoIterator<Item = &'a Delta>) -> Vec<Block> {
    enum Open {
        Text(String),
        Thinking(String),
        Tool {
            id: String,
            name: String,
            json: String,
        },
    }

    fn close(open: Open) -> Block {
        match open {
            Open::Text(text) => Block::Text { text },
            Open::Thinking(text) => Block::Thinking { text },
            Open::Tool { id, name, json } => {
                let input = if json.trim().is_empty() {
                    Value::Object(Default::default())
                } else {
                    serde_json::from_str(&json).unwrap_or(Value::String(json))
                };
                Block::ToolUse { id, name, input }
            }
        }
    }

    let mut blocks = Vec::new();
    let mut open: Option<Open> = None;
    for delta in deltas {
        match (delta, &mut open) {
            (Delta::Text { text }, Some(Open::Text(buf)))
            | (Delta::Thinking { text }, Some(Open::Thinking(buf))) => buf.push_str(text),
            (Delta::ToolInput { json }, Some(Open::Tool { json: buf, .. })) => buf.push_str(json),
            // Input with no open tool call has nothing to attach to.
            (Delta::ToolInput { .. }, _) => {}
            (delta, _) => {
                blocks.extend(open.take().map(close));
                open = Some(match delta {
                    Delta::Text { text } => Open::Text(text.clone()),
                    Delta::Thinking { text } => Open::Thinking(text.clone()),
                    Delta::ToolUseStart { id, name } => Open::Tool {
                        id: id.clone(),
                        name: name.clone(),
                        json: String::new(),
                    },
                    Delta::ToolInput { .. } => unreachable!("handled above"),
                });
            }
        }
    }
    blocks.extend(open.map(close));
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn records_use_stable_wire_names() {
        let record = Record {
            seq: 3,
            ts: 1000,
            event: Event::ToolResult {
                call_id: "c1".into(),
                content: vec![Block::text("ok")],
                is_error: false,
            },
        };
        let wire = serde_json::to_value(&record).unwrap();
        assert_eq!(
            wire,
            json!({
                "seq": 3,
                "ts": 1000,
                "type": "tool/result",
                "data": {
                    "call_id": "c1",
                    "content": [{"type": "text", "text": "ok"}],
                    "is_error": false
                }
            })
        );
        assert_eq!(serde_json::from_value::<Record>(wire).unwrap(), record);
    }

    #[test]
    fn session_ids_cannot_escape_the_store() {
        assert!(SessionId::parse("../etc").is_err());
        assert!(SessionId::parse("").is_err());
        assert!(SessionId::parse("a/b").is_err());
        assert!(serde_json::from_str::<SessionId>("\"..\"").is_err());
        assert!(SessionId::parse(SessionId::generate().as_str()).is_ok());
    }

    #[test]
    fn assemble_merges_fragments_and_keeps_truncated_tool_input() {
        let deltas = [
            Delta::Thinking { text: "hm".into() },
            Delta::Text { text: "Hel".into() },
            Delta::Text { text: "lo".into() },
            Delta::ToolUseStart {
                id: "c1".into(),
                name: "read".into(),
            },
            Delta::ToolInput {
                json: "{\"path\":".into(),
            },
            Delta::ToolInput {
                json: "\"a.rs\"}".into(),
            },
            Delta::ToolUseStart {
                id: "c2".into(),
                name: "bash".into(),
            },
            Delta::ToolInput {
                json: "{\"cmd\":\"ls".into(),
            },
        ];
        assert_eq!(
            assemble(&deltas),
            vec![
                Block::Thinking { text: "hm".into() },
                Block::text("Hello"),
                Block::ToolUse {
                    id: "c1".into(),
                    name: "read".into(),
                    input: json!({"path": "a.rs"}),
                },
                Block::ToolUse {
                    id: "c2".into(),
                    name: "bash".into(),
                    input: json!("{\"cmd\":\"ls"),
                },
            ]
        );
    }
}
