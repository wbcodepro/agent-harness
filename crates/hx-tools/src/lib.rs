//! Built-in tools and the trait they implement.
//!
//! A tool body has whatever privileges the process running it has. The
//! harness runs them through a [`ToolRunner`], normally one that ships each
//! call to a confined worker process.

mod bash;
mod files;
mod process;
mod search;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub use bash::Bash;
pub use files::{Edit, Read, Write};
pub use hx_kernel::BoxFuture;
use hx_session::ToolSpec;
pub use process::GroupGuard;
pub use search::{Glob, Grep, RepoMap};
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
    /// Set when the tool runs inside a worker process whose process group
    /// the caller manages. Commands then stay in that group instead of
    /// getting their own.
    pub shared_group: bool,
    kill_group: Arc<AtomicBool>,
}

impl ToolContext {
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: cwd.into(),
            shared_group: false,
            kill_group: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn resolve(&self, path: &str) -> PathBuf {
        let path = Path::new(path);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.cwd.join(path)
        }
    }

    /// Asks whoever owns the shared process group to kill it once this
    /// call's output has been delivered.
    pub fn request_group_kill(&self) {
        self.kill_group.store(true, Ordering::Relaxed);
    }

    pub fn group_kill_requested(&self) -> bool {
        self.kill_group.load(Ordering::Relaxed)
    }
}

pub trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;
    fn effect(&self) -> Effect;

    /// Whether the tool must run in the harness process instead of a
    /// confined worker, because it holds something a worker cannot have: a
    /// live connection, or its own stronger sandbox.
    fn host_side(&self) -> bool {
        false
    }

    fn call<'a>(&'a self, input: &'a Value, ctx: &'a ToolContext) -> BoxFuture<'a, ToolOutput>;
}

/// Where and how a tool body actually executes.
///
/// The default runs it in this process. A sandboxed runner ships the call to
/// a confined worker process instead.
pub trait ToolRunner: Send + Sync {
    fn run<'a>(
        &'a self,
        tool: &'a Arc<dyn Tool>,
        input: &'a Value,
        ctx: &'a ToolContext,
    ) -> BoxFuture<'a, ToolOutput>;
}

/// Runs tools in the harness process, with its full privileges.
pub struct InProcess;

impl ToolRunner for InProcess {
    fn run<'a>(
        &'a self,
        tool: &'a Arc<dyn Tool>,
        input: &'a Value,
        ctx: &'a ToolContext,
    ) -> BoxFuture<'a, ToolOutput> {
        tool.call(input, ctx)
    }
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

    /// `read`, `write`, `edit`, `bash`, `glob`, `grep` and `repo_map`.
    pub fn builtin() -> Self {
        let mut set = Self::new();
        set.insert(Arc::new(Read));
        set.insert(Arc::new(Write));
        set.insert(Arc::new(Edit));
        set.insert(Arc::new(Bash));
        set.insert(Arc::new(Glob));
        set.insert(Arc::new(Grep));
        set.insert(Arc::new(RepoMap));
        set
    }

    /// Adds a tool, replacing any tool with the same name.
    pub fn insert(&mut self, tool: Arc<dyn Tool>) {
        let name = tool.spec().name;
        self.tools.retain(|existing| existing.spec().name != name);
        self.tools.push(tool);
    }

    /// Keeps only the tools for which `keep` returns true.
    pub fn retain(&mut self, keep: impl Fn(&Arc<dyn Tool>) -> bool) {
        self.tools.retain(keep);
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
        assert_eq!(
            names,
            ["read", "write", "edit", "bash", "glob", "grep", "repo_map"]
        );
        assert!(
            specs
                .iter()
                .all(|spec| spec.input_schema["type"] == "object")
        );
    }
}
