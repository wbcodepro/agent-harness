//! Ways to extend the agent without changing it.
//!
//! - [`mcp`]: tools served by Model Context Protocol servers.
//! - [`skills`]: instruction packs the model loads on demand.
//! - [`hooks`]: user commands run before and after tool calls.
//! - [`wasm`]: plugins compiled to WebAssembly, with no access to anything
//!   the user did not grant.

pub mod hooks;
pub mod mcp;
pub mod skills;
pub mod wasm;
