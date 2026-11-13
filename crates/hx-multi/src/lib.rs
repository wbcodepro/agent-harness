//! Several agents working on one job.
//!
//! - [`SubagentTool`] hands a self-contained task to a fresh agent with its
//!   own context, optionally in its own git worktree so parallel workers do
//!   not collide.
//! - [`TaskBoard`] is a small shared list the agents coordinate through.
//! - [`TreeBudget`] caps the tokens a whole tree of agents may spend.

mod board;
mod budget;
mod subagent;

pub use board::{TaskBoard, board_tools};
pub use budget::{BudgetHook, TreeBudget};
pub use subagent::{Child, ChildFactory, ChildSpec, SubagentTool};
