//! An independent review before a turn ends.
//!
//! The agent's own tests only cover what the agent understood. When it read
//! a requirement wrongly, or not at all, its tests pass and the gate stays
//! quiet. A fresh agent that has seen none of the work reads the request
//! and the documents it names, probes the code where mistakes are likely,
//! and reports what it can show is wrong. The findings go back to the agent
//! as a user message, once per turn.

use hame_agent::{Session, TurnHook};
use hame_kernel::BoxFuture;
use hame_session::{Block, CreateOptions, Event, Record, SessionId, SessionStore};
use hame_tools::Effect;

use crate::{ChildFactory, ChildSpec};

/// Starts the message carrying the reviewer's findings.
pub const REVIEW_PREFIX: &str = "[hame review]";
/// What the reviewer answers when it finds nothing.
pub const NO_FINDINGS: &str = "NO FINDINGS";
/// Bytes of the reviewer's report passed on.
const MAX_REPORT_BYTES: usize = 8000;
/// Model requests the reviewer may make. A review that keeps probing
/// costs as much as the work it checks; one that has found a violation has
/// done its job.
const REVIEW_STEPS: u64 = 30;

pub struct Reviewer {
    pub factory: ChildFactory,
    pub store: SessionStore,
    pub cwd: std::path::PathBuf,
    /// Depth of the agent being reviewed.
    pub depth: u32,
}

/// The request that started the current turn: its first user message that
/// hame did not write itself.
fn request(records: &[Record]) -> Option<String> {
    let start = records
        .iter()
        .rposition(|record| matches!(record.event, Event::TurnStart { .. }))
        .unwrap_or(0);
    records[start..]
        .iter()
        .find_map(|record| match &record.event {
            Event::UserMessage { content } => {
                let text: String = content
                    .iter()
                    .filter_map(|block| match block {
                        Block::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                (!text.trim().is_empty() && !text.starts_with("[hame ")).then_some(text)
            }
            _ => None,
        })
}

fn reviewed_this_turn(records: &[Record]) -> bool {
    records
        .iter()
        .rev()
        .take_while(|record| !matches!(record.event, Event::TurnStart { .. }))
        .any(|record| {
            matches!(&record.event, Event::UserMessage { content }
                if matches!(content.first(), Some(Block::Text { text }) if text.starts_with(REVIEW_PREFIX)))
        })
}

/// Where the reviewer is told to put its probes: a place the sandbox lets
/// it write that is not the workspace.
#[cfg(windows)]
const PROBE_DIR: &str = "%TEMP%";
#[cfg(not(windows))]
const PROBE_DIR: &str = "/tmp";

/// The reviewer's instructions.
pub fn review_task(request: &str) -> String {
    format!(
        "Another agent was given the request below and says it has finished. Check its work \
         independently. Do not change any file in the working directory; write any probe \
         scripts under a new directory in {PROBE_DIR} and run them from there.\n\n\
         Read the request and every document it refers to. Then pick the requirements most \
         likely to have been missed or got wrong: rules that interact, cases a quick test \
         would not visit, exact output formats and orders, inputs that must be rejected, limits \
         on speed. For each one, work out the expected result from the documents yourself and \
         run a small probe against the current code to compare. Do not rely on the agent's own \
         tests.\n\n\
         Keep it short: about fifteen probes at most. Stop as soon as you have confirmed \
         violations of two or three requirements.\n\n\
         Report only requirements the code demonstrably violates: for each, the requirement, \
         the call, the expected result and the actual result. If you find no violation, reply \
         with exactly {NO_FINDINGS}.\n\nThe request:\n{request}"
    )
}

/// Whether a report says nothing is wrong.
fn clean(report: &str) -> bool {
    let report = report.trim();
    report.is_empty() || report.trim_end_matches('.').ends_with(NO_FINDINGS) && report.len() < 400
}

fn last_answer(records: &[Record]) -> String {
    records
        .iter()
        .rev()
        .find_map(|record| match &record.event {
            Event::AssistantMessage { content, .. } => {
                let text: String = content
                    .iter()
                    .filter_map(|block| match block {
                        Block::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect();
                (!text.trim().is_empty()).then_some(text)
            }
            _ => None,
        })
        .unwrap_or_default()
}

impl Reviewer {
    async fn review(&self, request: &str) -> Result<String, String> {
        let mut child = (self.factory)(ChildSpec {
            cwd: self.cwd.clone(),
            read_only: false,
            depth: self.depth + 1,
        })
        .await?;
        // It probes through the shell; it has no tool that edits files, and
        // does not hand the job on.
        child
            .agent
            .tools
            .retain(|tool| tool.effect() != Effect::Write && tool.spec().name != "subagent");
        child
            .agent
            .deferred
            .retain(|tool| tool.effect() != Effect::Write);
        child.agent.max_steps = child.agent.max_steps.min(REVIEW_STEPS);
        let id = SessionId::generate();
        let writer = self
            .store
            .create(CreateOptions {
                id: Some(id.clone()),
                cwd: self.cwd.to_string_lossy().into_owned(),
            })
            .map_err(|e| e.to_string())?;
        let records = self.store.read(&id).map_err(|e| e.to_string())?;
        let mut session = Session::new(records, writer);
        let outcome = child
            .agent
            .run_turn(&mut session, vec![Block::text(review_task(request))])
            .await;
        drop(child.keep_alive);
        let answer = last_answer(session.records());
        match outcome {
            Ok(()) => Ok(answer),
            // What it found before it ran out is still worth passing on.
            Err(_) if !answer.trim().is_empty() && !clean(&answer) => Ok(answer),
            Err(e) => Err(e.to_string()),
        }
    }
}

impl TurnHook for Reviewer {
    fn before_stop<'a>(
        &'a self,
        records: &'a [Record],
        changed: bool,
    ) -> BoxFuture<'a, Option<Vec<Block>>> {
        Box::pin(async move {
            if !changed || reviewed_this_turn(records) {
                return None;
            }
            let request = request(records)?;
            let report = match self.review(&request).await {
                Ok(report) => report,
                // A review that could not run says nothing about the work.
                Err(_) => return None,
            };
            if clean(&report) {
                return None;
            }
            let mut report = report.trim().to_string();
            if report.len() > MAX_REPORT_BYTES {
                let mut end = MAX_REPORT_BYTES;
                while !report.is_char_boundary(end) {
                    end -= 1;
                }
                report.truncate(end);
                report.push_str("\n[... report cut ...]");
            }
            Some(vec![Block::text(format!(
                "{REVIEW_PREFIX} A reviewer who saw only the request and the files probed your \
                 work and reports:\n\n{report}\n\nCheck each finding against the request. Fix \
                 the ones that are real; for any you are sure is wrong, say why. Then finish \
                 again."
            ))])
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_of_no_findings_is_clean() {
        assert!(clean("NO FINDINGS"));
        assert!(clean("I checked rules 3, 7 and 12.\n\nNO FINDINGS."));
        assert!(clean(""));
        assert!(!clean(
            "Rule 12: occurrences returns 2023-12-22 twice; expected once.\nNO FINDINGS for the rest"
        ));
    }

    #[test]
    fn the_request_is_the_first_message_the_user_wrote_this_turn() {
        let record = |event| Record {
            seq: 0,
            ts: 0,
            event,
        };
        let records = vec![
            record(Event::TurnStart { turn: 1 }),
            record(Event::UserMessage {
                content: vec![Block::text("old request")],
            }),
            record(Event::TurnStart { turn: 2 }),
            record(Event::UserMessage {
                content: vec![Block::text("implement SPEC.md")],
            }),
            record(Event::UserMessage {
                content: vec![Block::text("[hame verify] checks fail")],
            }),
        ];
        assert_eq!(request(&records).as_deref(), Some("implement SPEC.md"));
        assert!(!reviewed_this_turn(&records));
    }
}
