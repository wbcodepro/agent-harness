//! Several agents working on one job.
//!
//! - [`SubagentTool`] hands a self-contained task to a fresh agent with its
//!   own context, optionally in its own git worktree so parallel workers do
//!   not collide.
//! - [`TaskBoard`] is a small shared list the agents coordinate through.
//! - [`TreeBudget`] caps the tokens a whole tree of agents may spend.
//! - [`Reviewer`] has a fresh agent probe the work before a turn ends.
//! - [`Mailboxes`] and [`MessageTool`] let running agents of one tree write
//!   to each other.

mod board;
mod budget;
mod mail;
mod review;
mod subagent;

pub use board::{TaskBoard, board_tools};
pub use budget::{BudgetHook, TreeBudget};
pub use mail::{LEAD, Mailboxes, MessageTool};
pub use review::{NO_FINDINGS, REVIEW_PREFIX, Reviewer, review_task};
pub use subagent::{Child, ChildFactory, ChildSpec, SubagentKind, SubagentTool};
