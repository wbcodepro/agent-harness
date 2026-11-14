use std::process::Stdio;
use std::time::Duration;

use hame_agent::ToolRequest;
use hame_kernel::{BoxFuture, Layer, Next};
use hame_tools::ToolOutput;
use serde::Deserialize;
use serde_json::json;
use tokio::io::AsyncWriteExt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookEvent {
    /// Before a tool runs. Exit status 2 stops the call.
    PreTool,
    /// After a tool ran. Exit status 2 marks the result as failed.
    PostTool,
}

/// A command the user wants run around tool calls. It receives a JSON
/// description of the call on stdin. Exit status 2 is an objection and what
/// it wrote to stderr is shown to the model; any other status lets the call
/// stand.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hook {
    pub event: HookEvent,
    /// Tool name the hook applies to. Absent or `*` means every tool.
    pub tool: Option<String>,
    pub command: String,
    /// Seconds before the hook is abandoned. Default 60.
    pub timeout_secs: Option<u64>,
}

impl Hook {
    fn matches(&self, event: HookEvent, tool: &str) -> bool {
        self.event == event && self.tool.as_deref().is_none_or(|t| t == "*" || t == tool)
    }

    /// Runs the hook; returns its objection, if it made one.
    async fn run(&self, payload: &serde_json::Value, cwd: &std::path::Path) -> Option<String> {
        let mut child = tokio::process::Command::new(hame_kernel::sh_program())
            .arg("-c")
            .arg(&self.command)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .ok()?;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(payload.to_string().as_bytes()).await;
        }
        let limit = Duration::from_secs(self.timeout_secs.unwrap_or(60));
        let output = tokio::time::timeout(limit, child.wait_with_output())
            .await
            .ok()?
            .ok()?;
        (output.status.code() == Some(2)).then(|| {
            let reason = String::from_utf8_lossy(&output.stderr).trim().to_string();
            if reason.is_empty() {
                "(the hook gave no reason)".to_string()
            } else {
                reason
            }
        })
    }
}

/// Runs the user's hooks around every tool call. A hook that fails to
/// start, times out or exits with any status other than 2 never blocks.
pub struct HookLayer {
    pub hooks: Vec<Hook>,
    pub cwd: std::path::PathBuf,
}

impl Layer<ToolRequest, ToolOutput> for HookLayer {
    fn call<'a>(
        &'a self,
        request: ToolRequest,
        next: Next<'a, ToolRequest, ToolOutput>,
    ) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            let (name, input) = (request.name.clone(), request.input.clone());
            for hook in self
                .hooks
                .iter()
                .filter(|h| h.matches(HookEvent::PreTool, &name))
            {
                let payload =
                    json!({"event": "pre_tool", "tool": name, "input": input, "cwd": self.cwd});
                if let Some(reason) = hook.run(&payload, &self.cwd).await {
                    return ToolOutput::error(format!(
                        "Blocked by the user's pre-tool hook: {reason}"
                    ));
                }
            }

            let mut output = next.run(request).await;

            for hook in self
                .hooks
                .iter()
                .filter(|h| h.matches(HookEvent::PostTool, &name))
            {
                let payload = json!({
                    "event": "post_tool", "tool": name, "input": input, "cwd": self.cwd,
                    "output": output.text, "is_error": output.is_error,
                });
                if let Some(reason) = hook.run(&payload, &self.cwd).await {
                    output
                        .text
                        .push_str(&format!("\n[The user's post-tool hook objected: {reason}]"));
                    output.is_error = true;
                }
            }
            output
        })
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use hame_kernel::Chain;
    use hame_tools::Effect;

    fn hook(event: HookEvent, tool: Option<&str>, command: &str) -> Hook {
        Hook {
            event,
            tool: tool.map(String::from),
            command: command.into(),
            timeout_secs: Some(5),
        }
    }

    fn request(name: &str, input: serde_json::Value) -> ToolRequest {
        ToolRequest {
            call_id: "c".into(),
            name: name.into(),
            input,
            effect: Effect::Execute,
        }
    }

    #[tokio::test]
    async fn hooks_see_the_call_and_can_object_before_or_after() {
        let dir = tempfile::tempdir().unwrap();
        let chain = Chain::new();
        chain.push(HookLayer {
            cwd: dir.path().to_path_buf(),
            hooks: vec![
                // Blocks any bash command that mentions "prod".
                hook(
                    HookEvent::PreTool,
                    Some("bash"),
                    "payload=$(cat); case \"$payload\" in *prod*) echo 'no prod commands' >&2; exit 2;; esac",
                ),
                // Records every call it sees.
                hook(HookEvent::PostTool, None, "cat >> seen.jsonl; echo >> seen.jsonl"),
                // Objects to outputs containing TODO.
                hook(
                    HookEvent::PostTool,
                    Some("write"),
                    "grep -q TODO && { echo 'finish the TODO' >&2; exit 2; }; exit 0",
                ),
                // Broken hooks never block.
                hook(HookEvent::PreTool, None, "exit 1"),
                hook(HookEvent::PreTool, None, "/nonexistent/hook"),
            ],
        });
        let run = |request: ToolRequest, text: &'static str| {
            let chain = &chain;
            async move {
                chain
                    .run(request, move |_| async move { ToolOutput::ok(text) })
                    .await
            }
        };

        let blocked = run(request("bash", json!({"command": "deploy prod"})), "ran").await;
        assert_eq!(
            blocked,
            ToolOutput::error("Blocked by the user's pre-tool hook: no prod commands")
        );

        let allowed = run(request("bash", json!({"command": "ls"})), "ran").await;
        assert_eq!(allowed, ToolOutput::ok("ran"));

        let objected = run(request("write", json!({"path": "a"})), "wrote TODO").await;
        assert!(objected.is_error);
        assert!(
            objected
                .text
                .ends_with("[The user's post-tool hook objected: finish the TODO]")
        );

        // The blocked call never reached the post hook.
        let seen = std::fs::read_to_string(dir.path().join("seen.jsonl")).unwrap();
        let events: Vec<serde_json::Value> = seen
            .lines()
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["input"]["command"], "ls");
        assert_eq!(events[0]["output"], "ran");
    }
}
