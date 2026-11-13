//! Scripted model responses for tests, evals and replay.
//!
//! A [`Recording`] is the model's side of a session: the stream of each
//! response, in order. It can be written by hand, or lifted out of a real
//! session log with [`Recording::from_log`] and played back through
//! [`ScriptedLlm`] to re-run the harness without a network or a model.

use std::collections::VecDeque;
use std::fs;
use std::io;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use zcode_llm::{BoxFuture, Completion, DeltaSink, Llm, LlmError, Request};
use zcode_session::{Block, Delta, Event, Record, StopReason, Usage, assemble};

#[derive(Debug, thiserror::Error)]
pub enum EvalError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("recording line {line} is invalid: {reason}")]
    InvalidRecording { line: usize, reason: String },
    #[error("the script has no response left for request {request}")]
    ScriptExhausted { request: usize },
}

/// One complete model response, as it streamed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScriptedResponse {
    pub deltas: Vec<Delta>,
    pub stop: StopReason,
    #[serde(default)]
    pub usage: Usage,
}

impl ScriptedResponse {
    /// A plain text answer that ends the turn, streamed in one chunk.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            deltas: vec![Delta::Text { text: text.into() }],
            stop: StopReason::EndTurn,
            usage: Usage::default(),
        }
    }

    /// The session events this response produces: one journalled delta per
    /// chunk, then the settled message.
    pub fn events(&self) -> Vec<Event> {
        let mut events: Vec<Event> = self
            .deltas
            .iter()
            .map(|delta| Event::AssistantDelta {
                delta: delta.clone(),
            })
            .collect();
        events.push(Event::AssistantMessage {
            content: assemble(&self.deltas),
            stop: self.stop,
            usage: self.usage,
        });
        events
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Recording {
    pub responses: Vec<ScriptedResponse>,
}

impl Recording {
    /// Extracts every settled response from a session log.
    ///
    /// Uses the journalled deltas when they reproduce the settled message
    /// exactly; otherwise (a log without a journal, or one edited by a
    /// layer) falls back to one delta per content block, so playing the
    /// recording always settles to the logged message.
    pub fn from_log(records: &[Record]) -> Self {
        let mut responses = Vec::new();
        let mut journal: Vec<Delta> = Vec::new();
        for record in records {
            match &record.event {
                Event::AssistantDelta { delta } => journal.push(delta.clone()),
                Event::AssistantMessage {
                    content,
                    stop,
                    usage,
                } => {
                    let mut deltas = std::mem::take(&mut journal);
                    if assemble(&deltas) != *content {
                        deltas = deltas_for(content);
                    }
                    responses.push(ScriptedResponse {
                        deltas,
                        stop: *stop,
                        usage: *usage,
                    });
                }
                Event::AssistantAttempt { .. } | Event::StepStart { .. } => journal.clear(),
                _ => {}
            }
        }
        Self { responses }
    }

    /// Reads a recording stored as JSONL, one response per line.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, EvalError> {
        let text = fs::read_to_string(path)?;
        let mut responses = Vec::new();
        for (index, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            responses.push(serde_json::from_str(line).map_err(|e| {
                EvalError::InvalidRecording {
                    line: index + 1,
                    reason: e.to_string(),
                }
            })?);
        }
        Ok(Self { responses })
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), EvalError> {
        let mut out = String::new();
        for response in &self.responses {
            out.push_str(&serde_json::to_string(response).map_err(io::Error::from)?);
            out.push('\n');
        }
        fs::write(path, out)?;
        Ok(())
    }
}

fn deltas_for(content: &[Block]) -> Vec<Delta> {
    let mut deltas = Vec::new();
    for block in content {
        match block {
            Block::Text { text } => deltas.push(Delta::Text { text: text.clone() }),
            Block::Thinking { text, signature } => {
                if !text.is_empty() || signature.is_none() {
                    deltas.push(Delta::Thinking { text: text.clone() });
                }
                if let Some(signature) = signature {
                    deltas.push(Delta::ThinkingSignature {
                        signature: signature.clone(),
                    });
                }
            }
            Block::ToolUse { id, name, input } => {
                deltas.push(Delta::ToolUseStart {
                    id: id.clone(),
                    name: name.clone(),
                });
                deltas.push(Delta::ToolInput {
                    json: input.to_string(),
                });
            }
            Block::Fallback { from, to } => deltas.push(Delta::Fallback {
                from: from.clone(),
                to: to.clone(),
            }),
            // Assistant messages do not carry tool results.
            Block::ToolResult { .. } => {}
        }
    }
    deltas
}

/// Plays a [`Recording`] back, one response per request. Implements
/// [`Llm`], so it stands in for a real provider anywhere in the harness.
#[derive(Debug)]
pub struct ScriptedLlm {
    state: Mutex<ScriptState>,
    delay: Duration,
}

#[derive(Debug)]
struct ScriptState {
    remaining: VecDeque<ScriptedResponse>,
    served: usize,
}

impl ScriptedLlm {
    pub fn new(recording: Recording) -> Self {
        Self {
            state: Mutex::new(ScriptState {
                remaining: recording.responses.into(),
                served: 0,
            }),
            delay: Duration::ZERO,
        }
    }

    /// Waits this long before each streamed delta, to imitate a slow model.
    pub fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    /// The response to the next request. Asking for more responses than
    /// were recorded is an error: it means the harness diverged from the
    /// run that was recorded.
    pub fn respond(&self) -> Result<ScriptedResponse, EvalError> {
        let mut state = self.state.lock().expect("script lock poisoned");
        state.served += 1;
        let request = state.served;
        state
            .remaining
            .pop_front()
            .ok_or(EvalError::ScriptExhausted { request })
    }

    /// Responses recorded but not yet requested.
    pub fn remaining(&self) -> usize {
        self.state
            .lock()
            .expect("script lock poisoned")
            .remaining
            .len()
    }
}

impl Llm for ScriptedLlm {
    fn stream<'a>(
        &'a self,
        _request: &'a Request,
        sink: &'a mut dyn DeltaSink,
    ) -> BoxFuture<'a, Result<Completion, LlmError>> {
        Box::pin(async move {
            let response = self
                .respond()
                .map_err(|e| LlmError::Protocol(e.to_string()))?;
            for delta in response.deltas {
                if !self.delay.is_zero() {
                    tokio::time::sleep(self.delay).await;
                }
                sink.delta(delta).map_err(LlmError::Sink)?;
            }
            Ok(Completion {
                stop: response.stop,
                usage: response.usage,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use zcode_session::{Clock, CreateOptions, SessionId, SessionStore, TurnEndReason};

    struct TickClock(AtomicU64);

    impl Clock for TickClock {
        fn now_ms(&self) -> u64 {
            self.0.fetch_add(1, Ordering::Relaxed)
        }
    }

    fn recording() -> Recording {
        Recording {
            responses: vec![
                ScriptedResponse {
                    deltas: vec![
                        Delta::Text {
                            text: "Check".into(),
                        },
                        Delta::Text {
                            text: "ing.".into(),
                        },
                        Delta::ToolUseStart {
                            id: "c1".into(),
                            name: "bash".into(),
                        },
                        Delta::ToolInput {
                            json: "{\"cmd\":".into(),
                        },
                        Delta::ToolInput {
                            json: "\"ls\"}".into(),
                        },
                    ],
                    stop: StopReason::ToolUse,
                    usage: Usage {
                        input_tokens: 10,
                        output_tokens: 5,
                        ..Usage::default()
                    },
                },
                ScriptedResponse::text("One file."),
            ],
        }
    }

    /// Stands in for the agent loop: one step per scripted response, with a
    /// canned result for every tool the model calls.
    fn drive(root: &Path, recording: &Recording) -> Vec<u8> {
        let store = SessionStore::with_clock(root, Arc::new(TickClock(AtomicU64::new(0))));
        let mut writer = store
            .create(CreateOptions {
                id: Some(SessionId::parse("replay").unwrap()),
                cwd: "/work".into(),
            })
            .unwrap();
        let llm = ScriptedLlm::new(recording.clone());
        writer.append(Event::TurnStart { turn: 1 }).unwrap();
        writer
            .append(Event::UserMessage {
                content: vec![Block::text("What is here?")],
            })
            .unwrap();
        for step in 1.. {
            writer.append(Event::StepStart { turn: 1, step }).unwrap();
            let response = llm.respond().unwrap();
            for event in response.events() {
                writer.append(event).unwrap();
            }
            for block in assemble(&response.deltas) {
                if let Block::ToolUse { id, name, input } = block {
                    writer
                        .append(Event::ToolCall {
                            call_id: id.clone(),
                            name,
                            input,
                        })
                        .unwrap();
                    writer
                        .append(Event::ToolResult {
                            call_id: id,
                            content: vec![Block::text("a.rs")],
                            is_error: false,
                        })
                        .unwrap();
                }
            }
            writer.append(Event::StepEnd { turn: 1, step }).unwrap();
            if response.stop != StopReason::ToolUse {
                break;
            }
        }
        writer
            .append(Event::TurnEnd {
                turn: 1,
                reason: TurnEndReason::Completed,
            })
            .unwrap();
        assert_eq!(llm.remaining(), 0);
        fs::read(root.join("replay/session.v1.jsonl")).unwrap()
    }

    #[test]
    fn replaying_a_recording_reproduces_the_log_byte_for_byte() {
        let dir = tempfile::tempdir().unwrap();
        let first = drive(&dir.path().join("first"), &recording());

        let store = SessionStore::new(dir.path().join("first"));
        let records = store.read(&SessionId::parse("replay").unwrap()).unwrap();
        let lifted = Recording::from_log(&records);
        assert_eq!(lifted, recording());

        let second = drive(&dir.path().join("second"), &lifted);
        assert_eq!(first, second);
    }

    #[test]
    fn a_log_without_a_journal_still_yields_a_playable_recording() {
        let content = vec![
            Block::text("Checking."),
            Block::ToolUse {
                id: "c1".into(),
                name: "bash".into(),
                input: json!({"cmd": "ls"}),
            },
        ];
        let records = vec![Record {
            seq: 1,
            ts: 0,
            event: Event::AssistantMessage {
                content: content.clone(),
                stop: StopReason::ToolUse,
                usage: Usage::default(),
            },
        }];
        let lifted = Recording::from_log(&records);
        assert_eq!(lifted.responses.len(), 1);
        assert_eq!(assemble(&lifted.responses[0].deltas), content);
    }

    #[test]
    fn recordings_round_trip_through_a_file_and_exhaustion_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recording.jsonl");
        recording().save(&path).unwrap();
        let loaded = Recording::load(&path).unwrap();
        assert_eq!(loaded, recording());

        let llm = ScriptedLlm::new(loaded);
        llm.respond().unwrap();
        llm.respond().unwrap();
        assert!(matches!(
            llm.respond(),
            Err(EvalError::ScriptExhausted { request: 3 })
        ));

        fs::write(&path, "{\"deltas\":[]}\n").unwrap();
        assert!(matches!(
            Recording::load(&path),
            Err(EvalError::InvalidRecording { line: 1, .. })
        ));
    }
}
