use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use hame_agent::{AgentError, Session, TurnHook};
use hame_kernel::BoxFuture;
use hame_session::{Event, Record};

/// A token allowance shared by an agent and every subagent beneath it.
#[derive(Debug)]
pub struct TreeBudget {
    limit: u64,
    spent: AtomicU64,
}

impl TreeBudget {
    pub fn new(limit: u64) -> Arc<Self> {
        Arc::new(Self {
            limit,
            spent: AtomicU64::new(0),
        })
    }

    pub fn spent(&self) -> u64 {
        self.spent.load(Ordering::Relaxed)
    }
}

/// Charges one agent's usage to the shared budget as each response
/// arrives, and stops the agent before its next request once the tree has
/// spent it all. Install one per agent, all sharing the same [`TreeBudget`].
pub struct BudgetHook {
    budget: Arc<TreeBudget>,
}

impl BudgetHook {
    pub fn new(budget: Arc<TreeBudget>) -> Self {
        Self { budget }
    }
}

impl TurnHook for BudgetHook {
    fn after_response(&self, records: &[Record]) {
        if let Some(Event::AssistantMessage { usage, .. }) = records.last().map(|r| &r.event) {
            self.budget.spent.fetch_add(
                usage.input_tokens
                    + usage.output_tokens
                    + usage.cache_read_tokens
                    + usage.cache_write_tokens,
                Ordering::Relaxed,
            );
        }
    }

    fn before_step<'a>(
        &'a self,
        _session: &'a mut Session,
    ) -> BoxFuture<'a, Result<(), AgentError>> {
        Box::pin(async move {
            let spent = self.budget.spent();
            if spent >= self.budget.limit {
                return Err(AgentError::Hook(format!(
                    "the token budget for this task is used up ({spent} of {} tokens)",
                    self.budget.limit
                )));
            }
            Ok(())
        })
    }
}
