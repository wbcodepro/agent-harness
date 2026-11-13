//! The interactive terminal interface.
//!
//! [`App`] is the pure part: it folds session records into a transcript and
//! draws it. [`drive`] is the event loop around it: keys, turns running in
//! the background, approval questions and slash commands. `drive` is generic
//! over the terminal backend so the whole loop can be tested without a tty.

mod app;
mod driver;

pub use app::App;
pub use driver::{Msg, drive, run};
