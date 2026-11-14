use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hame_session::ToolSpec;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

use crate::{
    BoxFuture, Effect, GroupGuard, Tool, ToolContext, ToolOutput, parse_input, truncate_middle,
};

const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MAX_TIMEOUT_MS: u64 = 600_000;
/// Bytes kept per stream while the command runs; the rest is discarded.
const CAPTURE_LIMIT: usize = 1024 * 1024;
/// Bytes of each stream shown to the model.
const SHOWN_LIMIT: usize = 200_000;

/// Runs a shell command and returns its output.
pub struct Bash;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BashInput {
    command: String,
    timeout_ms: Option<u64>,
}

impl Tool for Bash {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "bash".into(),
            description: "Run a bash command in the working directory and return its stdout, \
                          stderr and exit code. Each call starts a new shell, with pipefail \
                          set: a pipeline fails when any command in it fails. The command is \
                          killed after timeout_ms (default 120000, max 600000)."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "The command line to run."},
                    "timeout_ms": {"type": "integer", "minimum": 1, "description": "Time limit in milliseconds."}
                },
                "required": ["command"],
                "additionalProperties": false
            }),
        }
    }

    fn effect(&self) -> Effect {
        Effect::Execute
    }

    fn call<'a>(&'a self, input: &'a Value, ctx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            let input: BashInput = match parse_input(input) {
                Ok(input) => input,
                Err(output) => return output,
            };
            let timeout = input
                .timeout_ms
                .unwrap_or(DEFAULT_TIMEOUT_MS)
                .clamp(1, MAX_TIMEOUT_MS);

            // Without Git's bash the only `bash` on a Windows path is the WSL
            // launcher, which fails in ways that say nothing useful.
            #[cfg(windows)]
            if hame_kernel::windows_bash().is_none() {
                return ToolOutput::error(
                    "Cannot start bash: no bash that works on Windows paths was found. \
                     Install Git for Windows, or put its bin directory on PATH.",
                );
            }
            let mut command = Command::new(hame_kernel::bash_program());
            // With pipefail a failing test run piped through `tail` still
            // reports its failure, where bash alone reports `tail`'s success.
            command
                .args(["-o", "pipefail", "-c"])
                .arg(&input.command)
                .current_dir(&ctx.cwd)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            // Its own process group, so everything it starts can be killed
            // together. Inside a worker the group is the worker's.
            #[cfg(unix)]
            if !ctx.shared_group {
                command.process_group(0);
            }

            let mut child = match command.spawn() {
                Ok(child) => child,
                Err(e) => return ToolOutput::error(format!("Cannot start bash: {e}")),
            };
            let mut group = if ctx.shared_group {
                GroupGuard::disarmed()
            } else {
                GroupGuard::new(child.id())
            };
            let stdout = Capture::start(child.stdout.take().expect("stdout is piped"));
            let stderr = Capture::start(child.stderr.take().expect("stderr is piped"));

            let status =
                match tokio::time::timeout(Duration::from_millis(timeout), child.wait()).await {
                    Ok(Ok(status)) => status,
                    Ok(Err(e)) => {
                        return ToolOutput::error(format!("Lost track of the command: {e}"));
                    }
                    Err(_) => {
                        // Our own group dies when `group` drops; a shared one is
                        // killed by its owner.
                        ctx.request_group_kill();
                        return ToolOutput::error(format!(
                            "The command did not finish within {timeout} ms and was killed."
                        ));
                    }
                };
            // The shell exited by itself. Leave anything it deliberately put
            // in the background running, and do not wait for it to close the
            // output pipes.
            group.disarm();
            let (stdout, stderr) = tokio::join!(stdout.finish(), stderr.finish());

            let mut text = String::new();
            if !stdout.is_empty() {
                text.push_str(&truncate_middle(&stdout, SHOWN_LIMIT));
            }
            if !stderr.is_empty() {
                if !text.is_empty() && !text.ends_with('\n') {
                    text.push('\n');
                }
                text.push_str("[stderr]\n");
                text.push_str(&truncate_middle(&stderr, SHOWN_LIMIT));
            }
            // A reader that stopped early (`... | head`) kills the writer
            // with SIGPIPE; that is how such a pipeline normally ends.
            let closed_pipe = status.code() == Some(141) && input.command.contains('|');
            if status.success() || closed_pipe {
                if text.is_empty() {
                    text.push_str("(no output)");
                }
                ToolOutput::ok(text)
            } else {
                if !text.is_empty() && !text.ends_with('\n') {
                    text.push('\n');
                }
                match status.code() {
                    Some(code) if input.command.contains('|') => text.push_str(&format!(
                        "[exit code {code}: a command in the pipeline failed; the shell runs \
                         with pipefail]"
                    )),
                    Some(code) => text.push_str(&format!("[exit code {code}]")),
                    None => text.push_str("[killed by a signal]"),
                }
                // Git's bash keeps shared state per installation. Once a
                // bash outside the sandbox has created it, one inside is
                // refused access and dies before running anything.
                #[cfg(windows)]
                if stderr.contains("NtCreateDirectoryObject") {
                    text.push_str(
                        "\n[hame] bash could not start in the sandbox because another bash \
                         from the same Git installation is running outside it (a Git Bash \
                         window, an editor's terminal). The command did not run. Tell the user: \
                         close those, or set HAME_BASH to the bash.exe of a second copy of \
                         Git for Windows that only hame uses.",
                    );
                }
                ToolOutput::error(text)
            }
        })
    }
}

/// Collects a stream in the background, keeping at most [`CAPTURE_LIMIT`]
/// bytes so a runaway command cannot exhaust memory.
struct Capture {
    state: Arc<Mutex<Captured>>,
    task: tokio::task::JoinHandle<()>,
}

/// The start and the end of a stream: the end is where a long run says
/// what went wrong.
#[derive(Default)]
struct Captured {
    head: Vec<u8>,
    tail: std::collections::VecDeque<u8>,
    dropped: usize,
}

impl Capture {
    fn start(mut stream: impl AsyncRead + Unpin + Send + 'static) -> Self {
        let state = Arc::new(Mutex::new(Captured::default()));
        let shared = Arc::clone(&state);
        let task = tokio::spawn(async move {
            let mut chunk = [0u8; 8192];
            loop {
                match stream.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let mut state = shared.lock().expect("capture lock poisoned");
                        let half = CAPTURE_LIMIT / 2;
                        let room = half.saturating_sub(state.head.len()).min(n);
                        state.head.extend_from_slice(&chunk[..room]);
                        state.tail.extend(&chunk[room..n]);
                        let over = state.tail.len().saturating_sub(half);
                        state.tail.drain(..over);
                        state.dropped += over;
                    }
                }
            }
        });
        Self { state, task }
    }

    /// What was captured. Called after the command exited: waits briefly for
    /// the pipe to drain, but not for a background process that kept it open.
    async fn finish(mut self) -> String {
        if tokio::time::timeout(Duration::from_millis(300), &mut self.task)
            .await
            .is_err()
        {
            self.task.abort();
        }
        let state = self.state.lock().expect("capture lock poisoned");
        let mut text = String::from_utf8_lossy(&state.head).into_owned();
        if state.dropped > 0 {
            text.push_str(&format!(
                "\n[... {} bytes not captured ...]\n",
                state.dropped
            ));
        }
        let (front, back) = state.tail.as_slices();
        text.push_str(&String::from_utf8_lossy(&[front, back].concat()));
        text
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::Instant;

    fn ctx(dir: &std::path::Path) -> ToolContext {
        ToolContext::new(dir)
    }

    #[tokio::test]
    async fn reports_output_streams_and_exit_code() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx(dir.path());

        let ok = Bash
            .call(&json!({"command": "echo out; echo err >&2; pwd"}), &ctx)
            .await;
        assert!(!ok.is_error);
        assert!(ok.text.starts_with("out\n"));
        assert!(ok.text.contains("[stderr]\nerr\n"));
        let cwd = dir.path().canonicalize().unwrap();
        assert!(ok.text.contains(cwd.file_name().unwrap().to_str().unwrap()));

        let quiet = Bash.call(&json!({"command": "true"}), &ctx).await;
        assert_eq!(quiet, ToolOutput::ok("(no output)"));

        let failed = Bash
            .call(&json!({"command": "echo nope; exit 3"}), &ctx)
            .await;
        assert_eq!(failed, ToolOutput::error("nope\n[exit code 3]"));
    }

    #[tokio::test]
    async fn a_timeout_kills_the_whole_process_group() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx(dir.path());
        let started = Instant::now();

        // The grandchild would create the marker if it outlived the timeout.
        let out = Bash
            .call(
                &json!({
                    "command": "(sleep 1; touch survived) & sleep 30",
                    "timeout_ms": 200
                }),
                &ctx,
            )
            .await;

        assert!(
            out.is_error && out.text.contains("was killed"),
            "{}",
            out.text
        );
        assert!(started.elapsed() < Duration::from_secs(5));
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(!dir.path().join("survived").exists());
    }

    #[tokio::test]
    async fn a_background_process_does_not_hold_the_call_open() {
        let dir = tempfile::tempdir().unwrap();
        let started = Instant::now();
        let out = Bash
            .call(
                &json!({"command": "sleep 20 & echo started"}),
                &ctx(dir.path()),
            )
            .await;
        assert_eq!(out, ToolOutput::ok("started\n"));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn a_failure_hidden_behind_a_pipe_is_still_a_failure() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path());
        let out = Bash
            .call(&json!({"command": "(echo boom; exit 3) | tail -n 5"}), &ctx)
            .await;
        assert!(out.is_error, "{}", out.text);
        assert!(out.text.contains("boom") && out.text.contains("pipefail"));

        // A reader that stops early is not a failure.
        let out = Bash
            .call(&json!({"command": "seq 1 200000 | head -n 2"}), &ctx)
            .await;
        assert!(!out.is_error, "{}", out.text);
    }

    #[tokio::test]
    async fn the_end_of_very_long_output_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path());
        let out = Bash
            .call(
                &json!({"command": "head -c 3000000 /dev/zero | tr '\\0' 'x'; echo; echo THE-END"}),
                &ctx,
            )
            .await;
        assert!(out.text.contains("THE-END"), "the tail is lost");
        assert!(out.text.contains("omitted"));
    }

    #[tokio::test]
    async fn huge_output_is_capped_not_buffered() {
        let dir = tempfile::tempdir().unwrap();
        let out = Bash
            .call(
                &json!({"command": "head -c 3000000 /dev/zero | tr '\\0' x"}),
                &ctx(dir.path()),
            )
            .await;
        assert!(!out.is_error);
        assert!(out.text.len() < 2 * SHOWN_LIMIT);
        assert!(out.text.len() < 3_000_000 / 2);
        assert!(out.text.contains("bytes omitted"));
    }
}
