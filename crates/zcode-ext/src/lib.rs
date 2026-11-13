//! Ways to extend the agent without changing it.
//!
//! - [`mcp`]: tools served by Model Context Protocol servers.
//! - [`lsp`]: compiler diagnostics from language servers.
//! - [`skills`]: instruction packs the model loads on demand.
//! - [`hooks`]: user commands run before and after tool calls.
//! - [`wasm`]: plugins compiled to WebAssembly, with no access to anything
//!   the user did not grant.

pub mod claude_hooks;
pub mod hooks;
pub mod lsp;
pub mod mcp;
pub mod skills;
pub mod wasm;
