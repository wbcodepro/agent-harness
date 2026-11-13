//! Built-in tools and the trait they implement.
//!
//! These run in the harness process with its full privileges. Confinement
//! belongs to the sandbox layer; until that exists, callers must gate
//! anything whose [`Effect`] is not `ReadOnly` behind approval.

mod bash;
mod files;

use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use bash::Bash;
pub use files::{Edit, Read, Write};
pub use hx_kernel::BoxFuture;
use hx_session::ToolSpec;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// What a tool can do to the world, for policy decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    ReadOnly,
    /// Creates or changes files.
    Write,
    /// Runs arbitrary programs.
    Execute,
}

/// The model-facing outcome of a tool call. A tool reports its own failures
/// here rather than as a Rust error, so the model can read and react.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutput {
    pub text: String,
    pub is_error: bool,
}

impl ToolOutput {
    pub fn ok(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: false,
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ToolContext {
    /// Directory relative paths resolve against and commands start in.
    pub cwd: PathBuf,
}

impl ToolContext {
    pub fn resolve(&self, path: &str) -> PathBuf {
        let path = Path::new(path);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.cwd.join(path)
        }
    }
}

pub trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;
    fn effect(&self) -> Effect;
    fn call<'a>(&'a self, input: &'a Value, ctx: &'a ToolContext) -> BoxFuture<'a, ToolOutput>;
}

/// An ordered collection of tools with unique names.
#[derive(Clone, Default)]
pub struct ToolSet {
    tools: Vec<Arc<dyn Tool>>,
}

impl ToolSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// `read`, `write`, `edit` and `bash`.
    pub fn builtin() -> Self {
        let mut set = Self::new();
        set.insert(Arc::new(Read));
        set.insert(Arc::new(Write));
        set.insert(Arc::new(Edit));
        set.insert(Arc::new(Bash));
        set
    }

    /// Adds a tool, replacing any tool with the same name.
    pub fn insert(&mut self, tool: Arc<dyn Tool>) {
        let name = tool.spec().name;
        self.tools.retain(|existing| existing.spec().name != name);
        self.tools.push(tool);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools
            .iter()
            .find(|tool| tool.spec().name == name)
            .cloned()
    }

    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools.iter().map(|tool| tool.spec()).collect()
    }
}

/// Parses a tool's input, turning a mismatch into an error the model can fix.
fn parse_input<T: DeserializeOwned>(input: &Value) -> Result<T, ToolOutput> {
    T::deserialize(input).map_err(|e| ToolOutput::error(format!("Invalid input: {e}")))
}

/// Shortens `text` to about `max` bytes by cutting out the middle, on
/// character boundaries, and says how much was removed.
fn truncate_middle(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut head = max / 2;
    while !text.is_char_boundary(head) {
        head -= 1;
    }
    let mut tail = text.len() - max / 2;
    while !text.is_char_boundary(tail) {
        tail += 1;
    }
    format!(
        "{}\n[... {} bytes omitted ...]\n{}",
        &text[..head],
        tail - head,
        &text[tail..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_keeps_both_ends_and_respects_char_boundaries() {
        assert_eq!(truncate_middle("short", 100), "short");
        let text = "é".repeat(100);
        let cut = truncate_middle(&text, 21);
        assert!(cut.starts_with("ééééé\n[... "));
        assert!(cut.ends_with(" bytes omitted ...]\nééééé"));
    }

    #[test]
    fn builtin_tools_have_unique_names_and_object_schemas() {
        let specs = ToolSet::builtin().specs();
        let names: Vec<&str> = specs.iter().map(|spec| spec.name.as_str()).collect();
        assert_eq!(names, ["read", "write", "edit", "bash"]);
        assert!(
            specs
                .iter()
                .all(|spec| spec.input_schema["type"] == "object")
        );
    }
}
