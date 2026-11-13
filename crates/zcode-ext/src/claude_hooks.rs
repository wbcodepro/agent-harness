//! Runs hooks written for Claude Code.
//!
//! People already have hook scripts in `settings.json` files. This reads
//! the same files and speaks the same protocol to the commands in them, for
//! the part of it that maps onto this harness: command hooks on
//! `PreToolUse`, `PostToolUse`, `UserPromptSubmit` and `Stop`. Other events
//! and handler types are reported once at startup and skipped.
//!
//! Hooks run in the harness process's environment, outside the sandbox,
//! like the user's own `[[hook]]` commands. That is why reading them is
//! opt-in, and reading a project's file is a separate opt-in.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::time::Duration;

use regex::Regex;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;
use zcode_agent::{AgentError, Session, ToolRequest, TurnHook};
use zcode_kernel::{BoxFuture, Layer, Next};
use zcode_session::{Block, Event, Record};
use zcode_tools::ToolOutput;

/// Starts the message that keeps a turn going because a Stop hook objected.
pub const STOP_HOOK_PREFIX: &str = "[hook Stop]";
/// Starts a message carrying context a hook added to the prompt.
pub const HOOK_CONTEXT_PREFIX: &str = "[hook context]";

/// A Stop hook may keep one turn going this many times.
const MAX_STOP_CONTINUATIONS: usize = 8;

#[derive(Deserialize)]
struct SettingsFile {
    #[serde(default)]
    hooks: BTreeMap<String, Vec<Group>>,
}

#[derive(Deserialize)]
struct Group {
    matcher: Option<String>,
    #[serde(default)]
    hooks: Vec<Handler>,
}

#[derive(Deserialize)]
struct Handler {
    #[serde(rename = "type")]
    kind: String,
    command: Option<String>,
    args: Option<Vec<String>>,
    /// Seconds.
    timeout: Option<u64>,
    #[serde(rename = "if")]
    condition: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum On {
    PreToolUse,
    PostToolUse,
    UserPromptSubmit,
    Stop,
}

impl On {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "PreToolUse" => Self::PreToolUse,
            "PostToolUse" => Self::PostToolUse,
            "UserPromptSubmit" => Self::UserPromptSubmit,
            "Stop" => Self::Stop,
            _ => return None,
        })
    }

    fn name(self) -> &'static str {
        match self {
            Self::PreToolUse => "PreToolUse",
            Self::PostToolUse => "PostToolUse",
            Self::UserPromptSubmit => "UserPromptSubmit",
            Self::Stop => "Stop",
        }
    }

    fn for_tools(self) -> bool {
        matches!(self, Self::PreToolUse | Self::PostToolUse)
    }
}

enum Matcher {
    All,
    Names(Vec<String>),
    Pattern(Regex),
}

impl Matcher {
    /// Plain names, alone or separated by `|` or `,`, match exactly;
    /// anything else is an unanchored regular expression.
    fn parse(raw: Option<&str>) -> Result<Self, String> {
        let raw = raw.unwrap_or("").trim();
        if raw.is_empty() || raw == "*" {
            return Ok(Self::All);
        }
        let plain = raw
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | ' ' | ',' | '|'));
        if plain {
            Ok(Self::Names(
                raw.split(['|', ','])
                    .map(|name| name.trim().to_string())
                    .collect(),
            ))
        } else {
            Regex::new(raw)
                .map(Self::Pattern)
                .map_err(|e| format!("matcher {raw:?} is not a valid pattern: {e}"))
        }
    }

    fn matches(&self, tool: &str) -> bool {
        match self {
            Self::All => true,
            Self::Names(names) => names.iter().any(|name| name == tool),
            Self::Pattern(pattern) => pattern.is_match(tool),
        }
    }
}

/// An `if` rule such as `Bash(git *)` or `Edit(*.ts)`: a tool name and a
/// glob over its command or file path.
struct Condition {
    tool: String,
    subject: Regex,
}

impl Condition {
    fn parse(raw: &str) -> Result<Self, String> {
        let (tool, pattern) = match raw.trim().split_once('(') {
            Some((tool, rest)) => (tool, rest.strip_suffix(')').unwrap_or(rest)),
            None => (raw.trim(), "*"),
        };
        let glob = pattern
            .split('*')
            .map(regex::escape)
            .collect::<Vec<_>>()
            .join(".*");
        Ok(Self {
            tool: tool.trim().to_string(),
            subject: Regex::new(&format!("^(?s){glob}$"))
                .map_err(|e| format!("if {raw:?} is not a valid rule: {e}"))?,
        })
    }

    fn matches(&self, tool: &str, input: &Value) -> bool {
        let subject = input["command"]
            .as_str()
            .or(input["file_path"].as_str())
            .unwrap_or("");
        self.tool == tool && self.subject.is_match(subject)
    }
}

struct Entry {
    on: On,
    matcher: Matcher,
    condition: Option<Condition>,
    command: String,
    args: Option<Vec<String>>,
    timeout: Duration,
    /// The directory of the plugin the hook came with, if any.
    plugin_root: Option<PathBuf>,
}

/// A file to read hooks from, and the plugin it belongs to when it is a
/// plugin's `hooks/hooks.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookFile {
    pub path: PathBuf,
    pub plugin_root: Option<PathBuf>,
}

impl From<PathBuf> for HookFile {
    fn from(path: PathBuf) -> Self {
        Self {
            path,
            plugin_root: None,
        }
    }
}

/// What one hook, or all hooks for one event together, decided.
#[derive(Debug, Default, PartialEq)]
struct Verdict {
    /// Set when the action must not go ahead, with the reason.
    block: Option<String>,
    /// Text to put in front of the model.
    context: Vec<String>,
    /// A replacement for the tool's input.
    updated_input: Option<Value>,
}

/// Reads what a finished hook said, by the Claude Code hook protocol:
/// exit status 2 blocks with stderr as the reason; exit status 0 may carry
/// a JSON object on stdout; anything else is a hook failure and changes
/// nothing.
fn interpret(on: On, code: Option<i32>, stdout: &str, stderr: &str) -> Verdict {
    let mut verdict = Verdict::default();
    let or_default = |reason: Option<&str>, fallback: &str| match reason.map(str::trim) {
        Some(reason) if !reason.is_empty() => reason.to_string(),
        _ => fallback.to_string(),
    };
    match code {
        Some(2) => verdict.block = Some(or_default(Some(stderr), "(the hook gave no reason)")),
        Some(0) => {
            let stdout = stdout.trim();
            let Some(said) = serde_json::from_str::<Value>(stdout)
                .ok()
                .filter(Value::is_object)
            else {
                // Plain output is context only where the protocol says so.
                if on == On::UserPromptSubmit && !stdout.is_empty() {
                    verdict.context.push(stdout.to_string());
                }
                return verdict;
            };
            let specific = &said["hookSpecificOutput"];
            if said["continue"] == json!(false) {
                verdict.block = Some(or_default(
                    said["stopReason"].as_str(),
                    "A hook stopped this.",
                ));
            } else if said["decision"] == "block" {
                verdict.block = Some(or_default(
                    said["reason"].as_str(),
                    "(the hook gave no reason)",
                ));
            } else if on == On::PreToolUse {
                let reason = specific["permissionDecisionReason"].as_str();
                match specific["permissionDecision"].as_str() {
                    Some("deny") => {
                        verdict.block = Some(or_default(reason, "(the hook gave no reason)"));
                    }
                    // There is no way from here to put the question to the
                    // user, and running the call unasked would be wrong.
                    Some("ask" | "defer") => {
                        verdict.block = Some(format!(
                            "a hook wants the user to confirm this call, which is not possible \
                             here: {}",
                            or_default(reason, "(no reason given)")
                        ));
                    }
                    _ => {}
                }
            }
            if on == On::PreToolUse && specific["updatedInput"].is_object() {
                verdict.updated_input = Some(specific["updatedInput"].clone());
            }
            if let Some(context) = specific["additionalContext"].as_str()
                && !context.trim().is_empty()
            {
                verdict.context.push(context.to_string());
            }
        }
        _ => {}
    }
    verdict
}

/// The name Claude Code gives the tool that does what `tool` does here.
fn claude_tool_name(tool: &str) -> &str {
    match tool {
        "bash" => "Bash",
        "read" => "Read",
        "write" => "Write",
        "edit" => "Edit",
        "glob" => "Glob",
        "grep" => "Grep",
        "subagent" => "Agent",
        other => other,
    }
}

fn takes_a_file(tool: &str) -> bool {
    matches!(tool, "read" | "write" | "edit")
}

/// Hooks loaded from Claude Code settings files.
pub struct ClaudeHooks {
    entries: Vec<Entry>,
    cwd: PathBuf,
    /// Where session logs live, for `transcript_path`.
    sessions: PathBuf,
    plan: bool,
    /// The session the agent is working in, once a turn has started.
    session: Mutex<Option<String>>,
}

impl ClaudeHooks {
    /// Reads the hooks of every file in `files` that exists. Returns the
    /// hooks and a line for each thing that was skipped or unreadable.
    pub fn load(
        files: &[HookFile],
        cwd: &Path,
        sessions: &Path,
        plan: bool,
    ) -> (Self, Vec<String>) {
        let mut entries: Vec<Entry> = Vec::new();
        let mut notes = Vec::new();
        let mut seen: Vec<(On, String, String)> = Vec::new();
        for HookFile {
            path: file,
            plugin_root,
        } in files
        {
            // A plugin refers to its own files through this placeholder.
            let in_plugin = |text: String| match plugin_root {
                Some(root) => text.replace("${CLAUDE_PLUGIN_ROOT}", &root.to_string_lossy()),
                None => text,
            };
            let Ok(text) = std::fs::read_to_string(file) else {
                continue;
            };
            let settings: SettingsFile = match serde_json::from_str(&text) {
                Ok(settings) => settings,
                Err(e) => {
                    notes.push(format!("{}: hooks not loaded: {e}", file.display()));
                    continue;
                }
            };
            for (event, groups) in settings.hooks {
                let Some(on) = On::parse(&event) else {
                    notes.push(format!(
                        "{}: {event} hooks are not supported and will not run",
                        file.display()
                    ));
                    continue;
                };
                for group in groups {
                    // Events without tools ignore the matcher.
                    let matcher = on.for_tools().then_some(group.matcher.as_deref()).flatten();
                    for handler in group.hooks {
                        let skip =
                            |why: String| format!("{}: a {event} hook {why}", file.display());
                        let Some(command) = handler.command.filter(|_| handler.kind == "command")
                        else {
                            notes.push(skip(format!(
                                "of type {:?} is not supported and will not run",
                                handler.kind
                            )));
                            continue;
                        };
                        // The same handler in several files runs once.
                        let key = (
                            on,
                            group.matcher.clone().unwrap_or_default(),
                            command.clone(),
                        );
                        if seen.contains(&key) {
                            continue;
                        }
                        let parsed = Matcher::parse(matcher).and_then(|matcher| {
                            let condition = match (&handler.condition, on.for_tools()) {
                                (Some(rule), true) => Some(Condition::parse(rule)?),
                                // By the protocol, `if` on a non-tool event
                                // means the hook never runs.
                                (Some(_), false) => return Err("has `if`, so it never runs".into()),
                                (None, _) => None,
                            };
                            Ok((matcher, condition))
                        });
                        match parsed {
                            Ok((matcher, condition)) => {
                                seen.push(key);
                                let default = if on == On::UserPromptSubmit { 30 } else { 600 };
                                entries.push(Entry {
                                    on,
                                    matcher,
                                    condition,
                                    command: in_plugin(command),
                                    args: handler
                                        .args
                                        .map(|args| args.into_iter().map(&in_plugin).collect()),
                                    timeout: Duration::from_secs(
                                        handler.timeout.unwrap_or(default),
                                    ),
                                    plugin_root: plugin_root.clone(),
                                });
                            }
                            Err(why) => notes.push(skip(format!("was skipped: {why}"))),
                        }
                    }
                }
            }
        }
        let mut said = Vec::new();
        notes.retain(|note: &String| {
            let new = !said.contains(note);
            said.push(note.clone());
            new
        });
        let hooks = Self {
            entries,
            cwd: cwd.to_path_buf(),
            sessions: sessions.to_path_buf(),
            plan,
            session: Mutex::new(None),
        };
        (hooks, notes)
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Fields every hook receives.
    fn payload(&self, on: On) -> Value {
        let session = self.session.lock().expect("hook lock poisoned").clone();
        let mut payload = json!({
            "cwd": self.cwd,
            "hook_event_name": on.name(),
            "permission_mode": if self.plan { "plan" } else { "default" },
        });
        if let Some(id) = session {
            payload["transcript_path"] = json!(self.sessions.join(&id).join("session.v1.jsonl"));
            payload["session_id"] = json!(id);
        }
        payload
    }

    async fn run(&self, entry: &Entry, payload: &Value) -> Verdict {
        let mut command = match &entry.args {
            Some(args) => {
                let mut command = tokio::process::Command::new(&entry.command);
                command.args(args);
                command
            }
            None => {
                let mut command = tokio::process::Command::new("sh");
                command.arg("-c").arg(&entry.command);
                command
            }
        };
        if let Some(root) = &entry.plugin_root {
            command.env("CLAUDE_PLUGIN_ROOT", root);
        }
        let spawned = command
            .current_dir(&self.cwd)
            .env("CLAUDE_PROJECT_DIR", &self.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn();
        let Ok(mut child) = spawned else {
            return Verdict::default();
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(payload.to_string().as_bytes()).await;
        }
        // A hook that cannot be run or does not finish changes nothing.
        match tokio::time::timeout(entry.timeout, child.wait_with_output()).await {
            Ok(Ok(output)) => interpret(
                entry.on,
                output.status.code(),
                &String::from_utf8_lossy(&output.stdout),
                &String::from_utf8_lossy(&output.stderr),
            ),
            _ => Verdict::default(),
        }
    }

    /// Runs every hook for `on` that applies, in the order they were
    /// loaded, stopping at the first that blocks.
    async fn run_all(&self, on: On, tool: Option<(&str, &Value)>, payload: &Value) -> Verdict {
        let mut combined = Verdict::default();
        for entry in self.entries.iter().filter(|entry| entry.on == on) {
            if let Some((name, input)) = tool {
                let wanted = entry.matcher.matches(name)
                    && entry
                        .condition
                        .as_ref()
                        .is_none_or(|condition| condition.matches(name, input));
                if !wanted {
                    continue;
                }
            }
            let verdict = self.run(entry, payload).await;
            combined.context.extend(verdict.context);
            if verdict.updated_input.is_some() {
                combined.updated_input = verdict.updated_input;
            }
            if verdict.block.is_some() {
                combined.block = verdict.block;
                break;
            }
        }
        combined
    }

    /// The call's input under the field names Claude Code uses.
    fn outward_input(&self, tool: &str, input: &Value) -> Value {
        let mut input = input.clone();
        if takes_a_file(tool)
            && let Some(object) = input.as_object_mut()
            && let Some(Value::String(path)) = object.remove("path")
        {
            object.insert("file_path".into(), json!(self.cwd.join(path)));
        }
        input
    }
}

/// An input a hook rewrote, back under this harness's field names.
fn inward_input(tool: &str, mut input: Value) -> Value {
    if takes_a_file(tool)
        && let Some(object) = input.as_object_mut()
        && let Some(path) = object.remove("file_path")
    {
        object.insert("path".into(), path);
    }
    input
}

/// The tool-call side of the hooks, for the tool pipeline. The same hooks
/// are also installed as a turn hook, hence the shared pointer.
pub struct ToolHooks(pub std::sync::Arc<ClaudeHooks>);

impl Layer<ToolRequest, ToolOutput> for ToolHooks {
    fn call<'a>(
        &'a self,
        request: ToolRequest,
        next: Next<'a, ToolRequest, ToolOutput>,
    ) -> BoxFuture<'a, ToolOutput> {
        self.0.around_tool(request, next)
    }
}

impl ClaudeHooks {
    fn around_tool<'a>(
        &'a self,
        mut request: ToolRequest,
        next: Next<'a, ToolRequest, ToolOutput>,
    ) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            let tool = request.name.clone();
            let name = claude_tool_name(&tool);
            let mut input = self.outward_input(&tool, &request.input);

            let mut payload = self.payload(On::PreToolUse);
            payload["tool_name"] = json!(name);
            payload["tool_input"] = input.clone();
            payload["tool_use_id"] = json!(request.call_id);
            let before = self
                .run_all(On::PreToolUse, Some((name, &input)), &payload)
                .await;
            if let Some(reason) = before.block {
                return ToolOutput::error(format!(
                    "PreToolUse:{name} hook blocked this call: {reason}"
                ));
            }
            if let Some(updated) = before.updated_input {
                request.input = inward_input(&tool, updated.clone());
                input = updated;
            }

            let mut output = next.run(request).await;
            let mut notes = before.context;

            // The protocol runs these only after a tool succeeded.
            if !output.is_error {
                payload["hook_event_name"] = json!(On::PostToolUse.name());
                payload["tool_input"] = input.clone();
                payload["tool_response"] = json!({ "output": output.text });
                let after = self
                    .run_all(On::PostToolUse, Some((name, &input)), &payload)
                    .await;
                notes.extend(after.context);
                if let Some(reason) = after.block {
                    notes.push(format!("PostToolUse:{name} hook: {reason}"));
                }
            }
            for note in notes {
                output.text.push_str(&format!("\n[hook] {note}"));
            }
            output
        })
    }
}

fn text_of(content: &[Block]) -> String {
    content
        .iter()
        .filter_map(|block| match block {
            Block::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

impl TurnHook for ClaudeHooks {
    fn before_step<'a>(
        &'a self,
        session: &'a mut Session,
    ) -> BoxFuture<'a, Result<(), AgentError>> {
        Box::pin(async move {
            *self.session.lock().expect("hook lock poisoned") = Some(session.id().to_string());
            // Only the first step of a turn has a prompt just submitted.
            let records = session.records();
            let first_step = matches!(
                records.last().map(|record| &record.event),
                Some(Event::StepStart { step: 1, .. })
            );
            let prompt = records.iter().rev().find_map(|record| match &record.event {
                Event::UserMessage { content } => Some(text_of(content)),
                _ => None,
            });
            let (true, Some(prompt)) = (first_step, prompt) else {
                return Ok(());
            };
            let mut payload = self.payload(On::UserPromptSubmit);
            payload["prompt"] = json!(prompt);
            let verdict = self.run_all(On::UserPromptSubmit, None, &payload).await;
            if let Some(reason) = verdict.block {
                return Err(AgentError::Hook(format!(
                    "a UserPromptSubmit hook blocked the prompt: {reason}"
                )));
            }
            if !verdict.context.is_empty() {
                session.append(Event::UserMessage {
                    content: vec![Block::text(format!(
                        "{HOOK_CONTEXT_PREFIX}\n{}",
                        verdict.context.join("\n")
                    ))],
                })?;
            }
            Ok(())
        })
    }

    fn before_stop<'a>(
        &'a self,
        records: &'a [Record],
        _changed: bool,
    ) -> BoxFuture<'a, Option<Vec<Block>>> {
        Box::pin(async move {
            let this_turn = || {
                records
                    .iter()
                    .rev()
                    .take_while(|record| !matches!(record.event, Event::TurnStart { .. }))
            };
            let continued = this_turn()
                .filter(|record| {
                    matches!(&record.event, Event::UserMessage { content }
                        if text_of(content).starts_with(STOP_HOOK_PREFIX))
                })
                .count();
            if continued >= MAX_STOP_CONTINUATIONS {
                return None;
            }
            let mut payload = self.payload(On::Stop);
            payload["stop_hook_active"] = json!(continued > 0);
            payload["last_assistant_message"] = json!(
                this_turn()
                    .find_map(|record| match &record.event {
                        Event::AssistantMessage { content, .. } => Some(text_of(content)),
                        _ => None,
                    })
                    .unwrap_or_default()
            );
            let verdict = self.run_all(On::Stop, None, &payload).await;
            verdict
                .block
                .map(|reason| vec![Block::text(format!("{STOP_HOOK_PREFIX} {reason}"))])
        })
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use zcode_kernel::Chain;
    use zcode_tools::Effect;

    fn hooks(dir: &Path, settings: Value) -> (ClaudeHooks, Vec<String>) {
        let file = dir.join("settings.json");
        std::fs::write(&file, settings.to_string()).unwrap();
        ClaudeHooks::load(
            &[file.clone().into(), file.into()],
            dir,
            &dir.join("sessions"),
            false,
        )
    }

    fn command(matcher: &str, command: &str) -> Value {
        json!({"matcher": matcher, "hooks": [{"type": "command", "command": command}]})
    }

    async fn call(hooks: ClaudeHooks, name: &str, input: Value) -> (ToolOutput, Value) {
        let chain: Chain<ToolRequest, ToolOutput> = Chain::new();
        chain.push(ToolHooks(std::sync::Arc::new(hooks)));
        let request = ToolRequest {
            call_id: "c1".into(),
            name: name.into(),
            input,
            effect: Effect::Execute,
        };
        let ran_with = std::sync::Arc::new(Mutex::new(Value::Null));
        let seen = ran_with.clone();
        let output = chain
            .run(request, move |request: ToolRequest| {
                *seen.lock().unwrap() = request.input.clone();
                async { ToolOutput::ok("ran") }
            })
            .await;
        let input = ran_with.lock().unwrap().clone();
        (output, input)
    }

    #[test]
    fn the_protocols_exit_codes_and_json_are_read_as_documented() {
        let blocked = |reason: &str| Verdict {
            block: Some(reason.into()),
            ..Verdict::default()
        };
        assert_eq!(
            interpret(On::PreToolUse, Some(2), "", "no rm\n"),
            blocked("no rm")
        );
        // Any other failure never blocks, and stderr of a success is ignored.
        assert_eq!(
            interpret(On::PreToolUse, Some(1), "", "boom"),
            Verdict::default()
        );
        assert_eq!(interpret(On::PreToolUse, None, "", ""), Verdict::default());
        assert_eq!(
            interpret(On::PreToolUse, Some(0), "", "noise"),
            Verdict::default()
        );

        let deny = r#"{"hookSpecificOutput": {"hookEventName": "PreToolUse",
            "permissionDecision": "deny", "permissionDecisionReason": "protected file"}}"#;
        assert_eq!(
            interpret(On::PreToolUse, Some(0), deny, ""),
            blocked("protected file")
        );
        let ask = r#"{"hookSpecificOutput": {"permissionDecision": "ask"}}"#;
        assert!(interpret(On::PreToolUse, Some(0), ask, "").block.is_some());
        let allow = r#"{"hookSpecificOutput": {"permissionDecision": "allow",
            "updatedInput": {"command": "ls -la"}, "additionalContext": "note"}}"#;
        assert_eq!(
            interpret(On::PreToolUse, Some(0), allow, ""),
            Verdict {
                block: None,
                context: vec!["note".into()],
                updated_input: Some(json!({"command": "ls -la"})),
            }
        );
        assert_eq!(
            interpret(
                On::Stop,
                Some(0),
                r#"{"decision": "block", "reason": "run the tests"}"#,
                ""
            ),
            blocked("run the tests")
        );
        assert_eq!(
            interpret(
                On::PostToolUse,
                Some(0),
                r#"{"continue": false, "stopReason": "halt"}"#,
                ""
            ),
            blocked("halt")
        );
        // Plain output is context for a prompt, and nothing for a tool.
        assert_eq!(
            interpret(On::UserPromptSubmit, Some(0), "Today is Monday.\n", "").context,
            ["Today is Monday."]
        );
        assert_eq!(
            interpret(On::PostToolUse, Some(0), "fine", ""),
            Verdict::default()
        );
    }

    #[tokio::test]
    async fn tool_hooks_see_claude_style_calls_and_can_block_rewrite_and_comment() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path().canonicalize().unwrap();
        let (loaded, notes) = hooks(
            &dir,
            json!({"hooks": {
                "PreToolUse": [
                    command("Bash", "cat > seen.json; grep -q 'rm -rf' seen.json && { echo 'no rm' >&2; exit 2; }; exit 0"),
                    command("Edit|Write", "cat > edit.json; echo '{\"hookSpecificOutput\": {\"updatedInput\": {\"file_path\": \"other.txt\", \"content\": \"x\"}}}'"),
                    {"hooks": [{"type": "command", "command": "exit 2", "if": "Bash(git push*)"}]},
                ],
                "PostToolUse": [command("^Ba", "echo '{\"decision\": \"block\", \"reason\": \"format the file\"}'")],
                "Notification": [command("", "true")],
                "Stop": [{"hooks": [{"type": "prompt", "prompt": "done?"}]}],
            }}),
        );
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert!(
            notes[0].contains("Notification") && notes[1].contains("\"prompt\""),
            "{notes:?}"
        );
        // The same file listed twice loads each hook once.
        assert_eq!(loaded.entries.len(), 4);
        *loaded.session.lock().unwrap() = Some("s1".into());

        let (output, _) = call(loaded, "bash", json!({"command": "ls"})).await;
        assert_eq!(
            output.text,
            "ran\n[hook] PostToolUse:Bash hook: format the file"
        );
        assert!(!output.is_error);
        let seen: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("seen.json")).unwrap()).unwrap();
        assert_eq!(seen["hook_event_name"], "PreToolUse");
        assert_eq!(seen["tool_name"], "Bash");
        assert_eq!(seen["tool_input"]["command"], "ls");
        assert_eq!(seen["tool_use_id"], "c1");
        assert_eq!(seen["session_id"], "s1");
        assert_eq!(seen["cwd"], json!(dir));
        assert!(
            seen["transcript_path"]
                .as_str()
                .unwrap()
                .ends_with("sessions/s1/session.v1.jsonl")
        );

        let reload = || {
            hooks(
                &dir,
                serde_json::from_str(&std::fs::read_to_string(dir.join("settings.json")).unwrap())
                    .unwrap(),
            )
            .0
        };
        let (output, input) = call(reload(), "bash", json!({"command": "rm -rf /"})).await;
        assert_eq!(output.text, "PreToolUse:Bash hook blocked this call: no rm");
        assert!(output.is_error && input.is_null(), "the tool did not run");

        let (output, _) = call(reload(), "bash", json!({"command": "git push -f"})).await;
        assert!(output.is_error, "the `if` rule applied");

        // File tools are shown with an absolute file_path, and a rewritten
        // input comes back under this harness's own field name.
        let (output, input) =
            call(reload(), "write", json!({"path": "a.txt", "content": "y"})).await;
        assert_eq!(output.text, "ran");
        let seen: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("edit.json")).unwrap()).unwrap();
        assert_eq!(seen["tool_name"], "Write");
        assert_eq!(seen["tool_input"]["file_path"], json!(dir.join("a.txt")));
        assert_eq!(input, json!({"path": "other.txt", "content": "x"}));
    }
}
