use crate::model::{AttemptReason, Block, Delta, Event, Record, TurnEndReason, assemble};

/// Text of the result synthesized for a tool call that had started when the
/// process stopped.
pub const INTERRUPTED_RUNNING: &str = "Interrupted: the harness stopped while this tool call was running. \
     It may have partly or fully taken effect; check the current state before retrying.";

/// Text of the result synthesized for a tool call that was requested but
/// never started.
pub const INTERRUPTED_NOT_STARTED: &str =
    "Interrupted: the harness stopped before this tool call started. It did not run.";

/// The events that close whatever a log left open.
///
/// A log ends open when the process died mid-turn. Appending the returned
/// events makes it well-formed again: a streamed response that never settled
/// becomes an `assistant/attempt`, every tool call the model is owed an
/// answer for gets an error result saying whether it had started, and the
/// open step and turn are ended as interrupted. Returns nothing for a log
/// that is already closed, so applying it twice is harmless.
pub fn plan_recovery(records: &[Record]) -> Vec<Event> {
    plan_close(records, TurnEndReason::Interrupted)
}

/// Like [`plan_recovery`], for a turn the running harness is abandoning on
/// purpose: `reason` says why (cancelled by the user, or failed).
pub fn plan_close(records: &[Record], reason: TurnEndReason) -> Vec<Event> {
    let mut open_turn: Option<u64> = None;
    let mut open_step: Option<(u64, u64)> = None;
    let mut unsettled: Vec<&Delta> = Vec::new();
    // (call id, whether a `tool/call` was logged for it)
    let mut owed: Vec<(String, bool)> = Vec::new();

    for record in records {
        match &record.event {
            Event::TurnStart { turn } => open_turn = Some(*turn),
            Event::TurnEnd { .. } => {
                open_turn = None;
                open_step = None;
                unsettled.clear();
                owed.clear();
            }
            Event::StepStart { turn, step } => {
                open_step = Some((*turn, *step));
                unsettled.clear();
            }
            Event::StepEnd { .. } => open_step = None,
            Event::AssistantDelta { delta } => unsettled.push(delta),
            Event::AssistantMessage { content, .. } => {
                unsettled.clear();
                owed.extend(content.iter().filter_map(|block| match block {
                    Block::ToolUse { id, .. } => Some((id.clone(), false)),
                    _ => None,
                }));
            }
            Event::AssistantAttempt { .. } => unsettled.clear(),
            Event::ToolCall { call_id, .. } => {
                match owed.iter_mut().find(|(id, _)| id == call_id) {
                    Some(entry) => entry.1 = true,
                    None => owed.push((call_id.clone(), true)),
                }
            }
            Event::ToolResult { call_id, .. } => owed.retain(|(id, _)| id != call_id),
            Event::Session(_)
            | Event::SystemMessage { .. }
            | Event::ToolsDeclared { .. }
            | Event::UserMessage { .. }
            | Event::Compaction { .. } => {}
        }
    }

    let Some(turn) = open_turn else {
        return Vec::new();
    };

    let mut plan = Vec::new();
    if !unsettled.is_empty() {
        plan.push(Event::AssistantAttempt {
            content: assemble(unsettled),
            reason: match reason {
                TurnEndReason::Cancelled => AttemptReason::Cancelled,
                TurnEndReason::Failed => AttemptReason::Failed,
                TurnEndReason::Completed | TurnEndReason::Interrupted => AttemptReason::Interrupted,
            },
        });
    }
    for (call_id, started) in owed {
        let text = if started {
            INTERRUPTED_RUNNING
        } else {
            INTERRUPTED_NOT_STARTED
        };
        plan.push(Event::ToolResult {
            call_id,
            content: vec![Block::text(text)],
            is_error: true,
        });
    }
    if let Some((turn, step)) = open_step {
        plan.push(Event::StepEnd { turn, step });
    }
    plan.push(Event::TurnEnd { turn, reason });
    plan
}
