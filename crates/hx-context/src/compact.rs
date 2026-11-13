use std::sync::Arc;

use hx_agent::{AgentError, Session, TurnHook};
use hx_kernel::BoxFuture;
use hx_llm::{Llm, Request, SinkError};
use hx_session::{Block, Delta, Event, Message, Record, Role, derive_history};

const SUMMARY_SYSTEM: &str = "You compress the working memory of a coding agent. You will be \
given the transcript of its session so far. Write the summary the agent will continue from; it \
will no longer see the transcript.";

const SUMMARY_REQUEST: &str = "Summarize the transcript above for the agent that will continue \
this work. Include, in this order:
1. Every request and instruction the user gave, verbatim where short, and whether each is done.
2. What has been done: files created or changed (with paths), commands run and their important \
results.
3. Facts learned about the codebase or environment that will be needed again.
4. What was in progress at the end and the exact next steps.
Be specific and complete rather than brief; do not mention this summarization.";

/// Bytes of one tool result kept in the transcript given to the summarizer.
const TRANSCRIPT_RESULT_BYTES: usize = 2000;

/// Replaces history with a summary when the context passes a threshold.
pub struct Compactor {
    /// Model that writes the summary.
    pub llm: Arc<dyn Llm>,
    /// Compact when the next request would carry more tokens than this.
    pub threshold_tokens: u64,
}

fn block_chars(blocks: &[Block]) -> usize {
    blocks
        .iter()
        .map(|block| match block {
            Block::Text { text } | Block::Thinking { text, .. } => text.len(),
            Block::ToolUse { name, input, .. } => name.len() + input.to_string().len(),
            Block::ToolResult { content, .. } => block_chars(content),
            Block::Fallback { .. } => 0,
        })
        .sum()
}

/// Estimates how many tokens the next request for this log will carry.
///
/// Uses the provider's own count from the most recent response when there is
/// one, and adds four bytes per token for everything logged since; without
/// a count, estimates the whole history that way.
pub fn context_tokens(records: &[Record]) -> u64 {
    let compacted_at = records
        .iter()
        .rposition(|record| matches!(record.event, Event::Compaction { .. }));
    let counted = records
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, record)| match &record.event {
            Event::AssistantMessage { usage, .. } if compacted_at.is_none_or(|cut| index > cut) => {
                let total = usage.input_tokens
                    + usage.cache_read_tokens
                    + usage.cache_write_tokens
                    + usage.output_tokens;
                (total > 0).then_some((index, total))
            }
            _ => None,
        });
    match counted {
        Some((index, total)) => {
            let since: usize = records[index + 1..]
                .iter()
                .map(|record| match &record.event {
                    Event::UserMessage { content } | Event::ToolResult { content, .. } => {
                        block_chars(content)
                    }
                    _ => 0,
                })
                .sum();
            total + since as u64 / 4
        }
        None => {
            let history = derive_history(records);
            let chars = history.system.map_or(0, |system| system.len())
                + history
                    .messages
                    .iter()
                    .map(|message| block_chars(&message.content))
                    .sum::<usize>();
            chars as u64 / 4
        }
    }
}

fn clip(text: &str, max: usize) -> &str {
    let mut end = max.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Renders model history as plain text, so any model can summarize it
/// without having to accept another model's tool-call structure.
fn transcript(messages: &[Message]) -> String {
    fn render(blocks: &[Block], out: &mut String) {
        for block in blocks {
            match block {
                Block::Text { text } => {
                    out.push_str(text);
                    out.push('\n');
                }
                Block::Thinking { .. } | Block::Fallback { .. } => {}
                Block::ToolUse { name, input, .. } => {
                    out.push_str(&format!("[called {name} with {input}]\n"));
                }
                Block::ToolResult {
                    content, is_error, ..
                } => {
                    let mut inner = String::new();
                    render(content, &mut inner);
                    let shown = clip(&inner, TRANSCRIPT_RESULT_BYTES);
                    out.push_str(if *is_error {
                        "[tool error] "
                    } else {
                        "[tool result] "
                    });
                    out.push_str(shown);
                    if shown.len() < inner.len() {
                        out.push_str("\n[... truncated ...]\n");
                    }
                }
            }
        }
    }
    let mut out = String::new();
    for message in messages {
        out.push_str(match message.role {
            Role::User => "\n## User\n",
            Role::Assistant => "\n## Agent\n",
            Role::Tool => "",
        });
        render(&message.content, &mut out);
    }
    out
}

/// Rough token count of what one record adds to the model's context.
fn record_tokens(record: &Record) -> u64 {
    let chars = match &record.event {
        Event::UserMessage { content }
        | Event::AssistantMessage { content, .. }
        | Event::ToolResult { content, .. } => block_chars(content),
        _ => 0,
    };
    chars as u64 / 4
}

impl Compactor {
    /// Where to cut: the index of the step start that the kept tail begins
    /// at. Everything before it gets summarized.
    ///
    /// Cutting only at step starts keeps every tool call together with its
    /// results. The most recent steps, about a third of the threshold, stay
    /// verbatim so the model does not lose the thread of what it is doing.
    /// Returns `None` when there is too little before the tail to be worth
    /// a summary: without that guard, a threshold below the fixed cost of
    /// the system prompt and tool list would compact on every step.
    fn cut_point(&self, records: &[Record]) -> Option<usize> {
        let first = records
            .iter()
            .rposition(|record| matches!(record.event, Event::Compaction { .. }))
            .map_or(0, |index| index + 1);
        let keep = self.threshold_tokens / 3;
        let worthwhile = self.threshold_tokens / 4;

        let mut tail = 0u64;
        let mut cut = None;
        for index in (first..records.len()).rev() {
            tail += record_tokens(&records[index]);
            if matches!(records[index].event, Event::StepStart { .. }) && tail >= keep {
                cut = Some(index);
                break;
            }
        }
        let cut = cut?;
        let covered: u64 = records[first..cut].iter().map(record_tokens).sum();
        (covered >= worthwhile).then_some(cut)
    }

    /// Summarizes the older part of what the model currently sees and logs
    /// a compaction covering it. Returns whether it compacted.
    pub async fn compact(&self, session: &mut Session) -> Result<bool, AgentError> {
        let Some(cut) = self.cut_point(session.records()) else {
            return Ok(false);
        };
        let covered = derive_history(&session.records()[..cut]);
        if covered.messages.is_empty() {
            return Ok(false);
        }
        let request = Request {
            system: Some(SUMMARY_SYSTEM.into()),
            tools: Vec::new(),
            messages: vec![Message {
                role: Role::User,
                content: vec![Block::text(format!(
                    "<transcript>\n{}\n</transcript>\n\n{SUMMARY_REQUEST}",
                    transcript(&covered.messages)
                ))],
            }],
            max_tokens: None,
        };
        // A side request: its stream is not part of the session's own
        // assistant output, so it is collected here rather than journalled.
        let mut summary = String::new();
        let mut collect = |delta: Delta| -> Result<(), SinkError> {
            if let Delta::Text { text } = delta {
                summary.push_str(&text);
            }
            Ok(())
        };
        self.llm.stream(&request, &mut collect).await?;
        if summary.trim().is_empty() {
            return Err(AgentError::Hook(
                "compaction failed: the model returned an empty summary".into(),
            ));
        }
        let up_to_seq = session.records()[cut].seq - 1;
        session.append(Event::Compaction {
            summary: format!(
                "[The earlier part of this session was summarized to save context. The most \
                 recent steps follow this summary unchanged. Summary:]\n\n{}",
                summary.trim()
            ),
            up_to_seq,
        })?;
        Ok(true)
    }
}

impl TurnHook for Compactor {
    fn before_step<'a>(
        &'a self,
        session: &'a mut Session,
    ) -> BoxFuture<'a, Result<(), AgentError>> {
        Box::pin(async move {
            if context_tokens(session.records()) > self.threshold_tokens {
                self.compact(session).await?;
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hx_eval::{Recording, ScriptedLlm, ScriptedResponse};
    use hx_session::{CreateOptions, SessionStore, StopReason, Usage};

    fn session(dir: &std::path::Path) -> Session {
        let store = SessionStore::new(dir);
        let writer = store.create(CreateOptions::default()).unwrap();
        let records = store.read(writer.id()).unwrap();
        Session::new(records, writer)
    }

    fn assistant(text: &str, usage: Usage) -> Event {
        Event::AssistantMessage {
            content: vec![Block::text(text)],
            stop: StopReason::EndTurn,
            usage,
        }
    }

    #[test]
    fn token_estimates_prefer_the_provider_count() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = session(dir.path());
        session
            .append(Event::UserMessage {
                content: vec![Block::text("x".repeat(400))],
            })
            .unwrap();
        assert_eq!(context_tokens(session.records()), 100);

        session
            .append(assistant(
                "ok",
                Usage {
                    input_tokens: 500,
                    output_tokens: 20,
                    cache_read_tokens: 1000,
                    cache_write_tokens: 0,
                },
            ))
            .unwrap();
        session
            .append(Event::UserMessage {
                content: vec![Block::text("y".repeat(40))],
            })
            .unwrap();
        assert_eq!(context_tokens(session.records()), 1530);
    }

    /// Appends one step in which the model calls a tool and gets `chars`
    /// characters back.
    fn step(session: &mut Session, step: u64, chars: usize) {
        let id = format!("c{step}");
        session.append(Event::StepStart { turn: 1, step }).unwrap();
        session
            .append(Event::AssistantMessage {
                content: vec![Block::ToolUse {
                    id: id.clone(),
                    name: "read".into(),
                    input: serde_json::json!({}),
                }],
                stop: StopReason::ToolUse,
                usage: Usage::default(),
            })
            .unwrap();
        session
            .append(Event::ToolResult {
                call_id: id,
                content: vec![Block::text(format!("step {step} ") + &"x".repeat(chars))],
                is_error: false,
            })
            .unwrap();
        session.append(Event::StepEnd { turn: 1, step }).unwrap();
    }

    fn compactor(responses: Vec<ScriptedResponse>, threshold_tokens: u64) -> Compactor {
        Compactor {
            llm: Arc::new(ScriptedLlm::new(Recording { responses })),
            threshold_tokens,
        }
    }

    #[tokio::test]
    async fn compaction_summarizes_old_steps_and_keeps_recent_ones_verbatim() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = session(dir.path());
        session.append(Event::TurnStart { turn: 1 }).unwrap();
        session
            .append(Event::UserMessage {
                content: vec![Block::text("Read everything.")],
            })
            .unwrap();
        for n in 1..=6 {
            step(&mut session, n, 400); // about 100 tokens each
        }
        session
            .append(Event::StepStart { turn: 1, step: 7 })
            .unwrap();
        let before = session.records().to_vec();

        // Threshold 300: keep about 100 tokens (the last step), summarize
        // the rest.
        let compactor = compactor(vec![ScriptedResponse::text("Read steps 1 to 5.")], 300);
        compactor.before_step(&mut session).await.unwrap();

        assert_eq!(
            session.records()[..before.len()],
            before[..],
            "the log is intact"
        );
        let history = session.history();
        let Block::Text { text } = &history.messages[0].content[0] else {
            panic!("summary is text");
        };
        assert!(text.ends_with("Read steps 1 to 5."));
        // Summary, then step 6's call and result, whole and paired.
        assert_eq!(history.messages.len(), 3);
        assert!(history.dangling_tool_calls().is_empty());
        let Block::ToolResult { content, .. } = &history.messages[2].content[0] else {
            panic!("the kept step's result follows its call");
        };
        assert!(matches!(&content[0], Block::Text { text } if text.starts_with("step 6 ")));

        // Nothing new since: no second summary (the script is exhausted, so
        // a request would fail).
        compactor.before_step(&mut session).await.unwrap();
        assert_eq!(session.records().len(), before.len() + 1);
    }

    #[tokio::test]
    async fn a_threshold_below_the_fixed_prompt_cost_does_not_compact_every_step() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = session(dir.path());
        session.append(Event::TurnStart { turn: 1 }).unwrap();
        // The provider reports a context far over the threshold, but almost
        // all of it is system prompt and tool definitions.
        session
            .append(Event::StepStart { turn: 1, step: 1 })
            .unwrap();
        session
            .append(assistant(
                "ok",
                Usage {
                    input_tokens: 5000,
                    ..Usage::default()
                },
            ))
            .unwrap();
        session.append(Event::StepEnd { turn: 1, step: 1 }).unwrap();
        session
            .append(Event::StepStart { turn: 1, step: 2 })
            .unwrap();
        let before = session.records().len();

        let compactor = compactor(Vec::new(), 1000);
        assert!(context_tokens(session.records()) > 1000);
        compactor.before_step(&mut session).await.unwrap();
        assert_eq!(session.records().len(), before, "nothing worth summarizing");
    }

    #[test]
    fn transcripts_are_plain_text_with_clipped_results() {
        let messages = vec![
            Message {
                role: Role::User,
                content: vec![Block::text("List files")],
            },
            Message {
                role: Role::Assistant,
                content: vec![Block::ToolUse {
                    id: "c1".into(),
                    name: "bash".into(),
                    input: serde_json::json!({"command": "ls"}),
                }],
            },
            Message {
                role: Role::Tool,
                content: vec![Block::ToolResult {
                    call_id: "c1".into(),
                    content: vec![Block::text("é".repeat(3000))],
                    is_error: false,
                }],
            },
        ];
        let text = transcript(&messages);
        assert!(text.contains("## User\nList files"));
        assert!(text.contains("[called bash with {\"command\":\"ls\"}]"));
        assert!(text.contains("[... truncated ...]"));
        assert!(text.len() < 2400);
    }
}
