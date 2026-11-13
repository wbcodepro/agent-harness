//! Keeping the model's context small and useful.
//!
//! - [`SpillLayer`] moves large tool outputs to files and leaves a preview.
//! - [`Compactor`] replaces old history with a summary once the context
//!   grows past a threshold. The log keeps everything; only the projection
//!   the model sees changes.
//! - [`load_instructions`] reads the project's and user's `AGENTS.md`.

mod compact;
mod instructions;
mod spill;

pub use compact::{Compactor, context_tokens};
pub use instructions::load_instructions;
pub use spill::SpillLayer;
