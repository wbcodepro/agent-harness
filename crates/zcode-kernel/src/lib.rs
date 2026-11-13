//! Kernel primitives shared by every zcode crate.
//!
//! - [`Services`]: a typed registry for swappable capabilities ("seams").
//! - [`Bus`]: fire-and-forget notifications.
//! - [`Chain`]: around-middleware for interception points such as tool
//!   execution, where a layer may rewrite the request, short-circuit, or
//!   post-process the result.
//!
//! Every registration returns a [`Registration`] id so it can be undone.

mod bus;
mod chain;
mod paths;
mod services;
mod shell;

pub use bus::Bus;
pub use chain::{BoxFuture, Chain, Layer, Next};
pub use paths::{real_path, simplify_path};
pub use services::{MissingService, Services};
#[cfg(windows)]
pub use shell::windows_bash;
pub use shell::{bash_program, sh_program};

/// Handle for removing a listener or layer that was registered earlier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Registration(pub(crate) u64);
