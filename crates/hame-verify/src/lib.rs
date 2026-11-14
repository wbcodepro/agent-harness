//! The completion gate.
//!
//! When a turn that changed the workspace is about to end, the project's
//! check commands run. If any fails, its output goes back to the model as a
//! user message and the turn continues, so "done" means the checks pass, or
//! the model has seen the failure and said why it stands.

mod escalate;
mod redcheck;

use std::sync::Arc;

use hame_agent::TurnHook;
use hame_kernel::BoxFuture;
use hame_session::{Block, Event, Record};
use hame_tools::{Tool, ToolContext, ToolRunner};
use serde_json::json;

/// Starts every message the gate sends, so they can be told apart from what
/// the user typed.
pub use escalate::{Escalating, Escalation, REPEAT_PREFIX, RepeatGuard, Switch, trouble_this_turn};

pub use redcheck::{CHECK_PREFIX, Evidence, RedCheck, evidence};

pub const VERIFY_PREFIX: &str = "[hame verify]";

/// Bytes of a failing check's output shown to the model.
const MAX_OUTPUT_BYTES: usize = 6000;

pub struct Verifier {
    /// Shell commands that must all succeed.
    pub commands: Vec<String>,
    /// How many times in one turn a failure is sent back before the turn is
    /// allowed to end anyway.
    pub max_rounds: usize,
    pub timeout_ms: u64,
    /// Runs the commands with the same confinement as the agent's tools.
    pub runner: Arc<dyn ToolRunner>,
    /// The shell tool the commands are run with.
    pub shell: Arc<dyn Tool>,
    pub context: ToolContext,
    /// Once per turn, before it ends, have the model check its work against
    /// the request.
    pub checklist: bool,
}

/// Starts the message asking the model to check its work.
pub const CHECKLIST_PREFIX: &str = "[hame checklist]";

fn sent_this_turn(records: &[Record], prefix: &str) -> usize {
    records
        .iter()
        .rev()
        .take_while(|record| !matches!(record.event, Event::TurnStart { .. }))
        .filter(|record| {
            matches!(&record.event, Event::UserMessage { content }
                if matches!(content.first(), Some(Block::Text { text }) if text.starts_with(prefix)))
        })
        .count()
}

fn rounds_this_turn(records: &[Record]) -> usize {
    sent_this_turn(records, VERIFY_PREFIX)
}

impl TurnHook for Verifier {
    fn before_stop<'a>(
        &'a self,
        records: &'a [Record],
        changed: bool,
    ) -> BoxFuture<'a, Option<Vec<Block>>> {
        Box::pin(async move {
            if !changed {
                return None;
            }
            let mut failures = String::new();
            let commands = if rounds_this_turn(records) < self.max_rounds {
                self.commands.as_slice()
            } else {
                &[]
            };
            for command in commands {
                let input = json!({ "command": command, "timeout_ms": self.timeout_ms });
                let output = self.runner.run(&self.shell, &input, &self.context).await;
                if output.is_error {
                    // Both ends of a long run: what failed is usually said
                    // last, and what was run first.
                    let text = &output.text;
                    if text.len() <= MAX_OUTPUT_BYTES {
                        failures.push_str(&format!("\n$ {command}\n{text}\n"));
                    } else {
                        let mut head = MAX_OUTPUT_BYTES / 3;
                        while !text.is_char_boundary(head) {
                            head -= 1;
                        }
                        let mut tail = text.len() - MAX_OUTPUT_BYTES * 2 / 3;
                        while !text.is_char_boundary(tail) {
                            tail += 1;
                        }
                        failures.push_str(&format!(
                            "\n$ {command}\n{}\n[... {} bytes omitted ...]\n{}\n",
                            &text[..head],
                            tail - head,
                            &text[tail..]
                        ));
                    }
                }
            }
            if failures.is_empty() {
                // Weaker models stop with part of the request undone more
                // often than they get any one part wrong.
                let ask = self.checklist && sent_this_turn(records, CHECKLIST_PREFIX) == 0;
                return ask.then(|| {
                    vec![Block::text(format!(
                        "{CHECKLIST_PREFIX} Before you finish: read the request again and go \
                         through it point by point. For each thing asked, name the file or \
                         command output that shows it is done. Do now whatever is missing or \
                         only partly done. If everything is done, give your final answer."
                    ))]
                });
            }
            Some(vec![Block::text(format!(
                "{VERIFY_PREFIX} The project's checks fail after your changes:\n{failures}\n\
                 Fix the cause and finish again. If a failure is unrelated to your changes or \
                 cannot be fixed here, say so explicitly instead of ending as if all were well."
            ))])
        })
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use hame_tools::{Bash, InProcess};

    fn verifier(dir: &std::path::Path, commands: &[&str]) -> Verifier {
        Verifier {
            commands: commands.iter().map(|c| c.to_string()).collect(),
            max_rounds: 2,
            timeout_ms: 10_000,
            runner: Arc::new(InProcess),
            shell: Arc::new(Bash),
            context: ToolContext::new(dir),
            checklist: false,
        }
    }

    fn record(seq: u64, event: Event) -> Record {
        Record { seq, ts: 0, event }
    }

    #[tokio::test]
    async fn failing_checks_are_reported_until_the_round_limit() {
        let dir = tempfile::tempdir().unwrap();
        let verifier = verifier(dir.path(), &["true", "echo broken >&2; exit 3"]);
        let mut records = vec![record(0, Event::TurnStart { turn: 1 })];

        assert!(verifier.before_stop(&records, false).await.is_none());

        let first = verifier.before_stop(&records, true).await.unwrap();
        let Block::Text { text } = &first[0] else {
            panic!("the report is text");
        };
        assert!(text.starts_with(VERIFY_PREFIX));
        assert!(text.contains("$ echo broken >&2; exit 3"));
        assert!(text.contains("broken") && text.contains("[exit code 3]"));
        assert!(!text.contains("$ true"));

        records.push(record(
            1,
            Event::UserMessage {
                content: first.clone(),
            },
        ));
        assert!(verifier.before_stop(&records, true).await.is_some());
        records.push(record(2, Event::UserMessage { content: first }));
        assert!(
            verifier.before_stop(&records, true).await.is_none(),
            "after max_rounds the turn may end"
        );

        // A new turn starts counting again.
        records.push(record(3, Event::TurnStart { turn: 2 }));
        assert!(verifier.before_stop(&records, true).await.is_some());
    }

    #[tokio::test]
    async fn passing_checks_let_the_turn_end() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("ok"), "").unwrap();
        let verifier = verifier(dir.path(), &["test -f ok"]);
        let records = vec![record(0, Event::TurnStart { turn: 1 })];
        assert!(verifier.before_stop(&records, true).await.is_none());
        assert!(
            super::Verifier {
                commands: vec![],
                ..verifier
            }
            .before_stop(&records, true)
            .await
            .is_none()
        );
    }

    #[tokio::test]
    async fn the_checklist_is_asked_once_and_only_after_the_checks_pass() {
        let dir = tempfile::tempdir().unwrap();
        let verifier = Verifier {
            checklist: true,
            ..verifier(dir.path(), &["test -f ok"])
        };
        let mut records = vec![record(0, Event::TurnStart { turn: 1 })];
        let text = |blocks: Vec<Block>| match &blocks[0] {
            Block::Text { text } => text.clone(),
            _ => String::new(),
        };

        assert!(verifier.before_stop(&records, false).await.is_none());
        let failing = text(verifier.before_stop(&records, true).await.unwrap());
        assert!(failing.starts_with(VERIFY_PREFIX));

        std::fs::write(dir.path().join("ok"), "").unwrap();
        let asked = verifier.before_stop(&records, true).await.unwrap();
        assert!(text(asked.clone()).starts_with(CHECKLIST_PREFIX));
        records.push(record(1, Event::UserMessage { content: asked }));
        assert!(verifier.before_stop(&records, true).await.is_none());

        // A new turn is asked again.
        records.push(record(2, Event::TurnStart { turn: 2 }));
        assert!(verifier.before_stop(&records, true).await.is_some());
    }
}
