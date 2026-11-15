//! Execution budgets: stops a task that has used its tokens or time.
//!
//! The limits come from the task contract. They are checked before every
//! model request, so a turn stops at a decision boundary rather than in the
//! middle of an action. The stop is an explicit failure with the reason,
//! never a quiet end that could be mistaken for completion.

use std::time::Instant;

use hame_agent::{AgentError, Session, TurnHook};
use hame_kernel::BoxFuture;
use hame_session::contract::Budget;
use hame_session::{Event, Record};

/// Prefix of the error a budget stop produces.
pub const BUDGET_PREFIX: &str = "[hame budget]";

/// All tokens of every request in `records`, cached included: what the
/// budget's `max_tokens` counts.
pub fn tokens_used(records: &[Record]) -> u64 {
    records
        .iter()
        .map(|record| match &record.event {
            Event::AssistantMessage { usage, .. } | Event::Compaction { usage, .. } => {
                usage.input_tokens
                    + usage.cache_read_tokens
                    + usage.cache_write_tokens
                    + usage.output_tokens
            }
            _ => 0,
        })
        .sum()
}

pub struct BudgetGuard {
    pub budget: Budget,
    pub started: Instant,
    /// Records already in the session when the task began; only what comes
    /// after them counts.
    pub baseline: usize,
}

impl BudgetGuard {
    pub fn new(budget: Budget, session: &Session) -> Self {
        Self {
            budget,
            started: Instant::now(),
            baseline: session.records().len(),
        }
    }

    /// Why the task must stop, if it must.
    pub fn exceeded(&self, records: &[Record]) -> Option<String> {
        let tail = records.get(self.baseline..).unwrap_or(&[]);
        if let Some(max) = self.budget.max_tokens {
            let used = tokens_used(tail);
            if used >= max {
                return Some(format!(
                    "{BUDGET_PREFIX} token budget exhausted: {used} of {max} tokens used"
                ));
            }
        }
        if let Some(deadline) = self.budget.deadline_ms {
            let elapsed = self.started.elapsed().as_millis() as u64;
            if elapsed >= deadline {
                return Some(format!(
                    "{BUDGET_PREFIX} time budget exhausted: {elapsed} ms of {deadline} ms"
                ));
            }
        }
        None
    }
}

impl TurnHook for BudgetGuard {
    fn before_step<'a>(
        &'a self,
        session: &'a mut Session,
    ) -> BoxFuture<'a, Result<(), AgentError>> {
        Box::pin(async move {
            match self.exceeded(session.records()) {
                Some(reason) => Err(AgentError::Hook(reason)),
                None => Ok(()),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hame_session::{StopReason, Usage};

    fn response(input: u64, output: u64) -> Record {
        Record {
            seq: 0,
            ts: 0,
            event: Event::AssistantMessage {
                content: vec![],
                stop: StopReason::EndTurn,
                usage: Usage {
                    input_tokens: input,
                    output_tokens: output,
                    cache_read_tokens: 100,
                    cache_write_tokens: 0,
                },
            },
        }
    }

    #[test]
    fn tokens_and_time_are_counted_from_the_task_start() {
        let guard = BudgetGuard {
            budget: Budget {
                max_tokens: Some(1000),
                ..Budget::default()
            },
            started: Instant::now(),
            baseline: 1,
        };
        // The first record predates the task and does not count.
        let records = vec![response(5000, 0), response(400, 100)];
        assert_eq!(tokens_used(&records[1..]), 600);
        assert!(guard.exceeded(&records).is_none());
        let records = vec![response(5000, 0), response(400, 100), response(300, 0)];
        assert!(guard.exceeded(&records).unwrap().contains("1000 of 1000"));

        let late = BudgetGuard {
            budget: Budget {
                deadline_ms: Some(0),
                ..Budget::default()
            },
            started: Instant::now(),
            baseline: 0,
        };
        assert!(late.exceeded(&[]).unwrap().contains("time budget"));
    }
}
