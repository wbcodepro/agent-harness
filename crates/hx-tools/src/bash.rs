use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hx_session::ToolSpec;
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
                          stderr and exit code. Each call starts a new shell. The command is \
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

            let mut command = Command::new(shell_program());
            command
                .arg("-c")
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
            if status.success() {
                if text.is_empty() {
                    text.push_str("(no output)");
                }
                ToolOutput::ok(text)
            } else {
                if !text.is_empty() && !text.ends_with('\n') {
                    text.push('\n');
                }
                match status.code() {
                    Some(code) => text.push_str(&format!("[exit code {code}]")),
                    None => text.push_str("[killed by a signal]"),
                }
                ToolOutput::error(text)
            }
        })
    }
}

/// The bash to run commands with.
///
/// On Windows the `bash` found first on the system path is usually the
/// launcher for the Windows Subsystem for Linux, which runs commands in a
/// different machine with a different filesystem. Git for Windows ships the
/// bash that works on Windows paths, so that one is looked for by location.
#[cfg(windows)]
fn shell_program() -> std::path::PathBuf {
    let roots = [
        "ProgramFiles",
        "ProgramW6432",
        "ProgramFiles(x86)",
        "LOCALAPPDATA",
    ];
    let layouts = [r"Git\bin\bash.exe", r"Programs\Git\bin\bash.exe"];
    for root in roots.iter().filter_map(std::env::var_os) {
        for layout in layouts {
            let candidate = std::path::Path::new(&root).join(layout);
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    "bash".into()
}

#[cfg(not(windows))]
fn shell_program() -> &'static str {
    "bash"
}

/// Collects a stream in the background, keeping at most [`CAPTURE_LIMIT`]
/// bytes so a runaway command cannot exhaust memory.
struct Capture {
    state: Arc<Mutex<(Vec<u8>, usize)>>,
    task: tokio::task::JoinHandle<()>,
}

impl Capture {
    fn start(mut stream: impl AsyncRead + Unpin + Send + 'static) -> Self {
        let state = Arc::new(Mutex::new((Vec::new(), 0usize)));
        let shared = Arc::clone(&state);
        let task = tokio::spawn(async move {
            let mut chunk = [0u8; 8192];
            loop {
                match stream.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let mut state = shared.lock().expect("capture lock poisoned");
                        let room = CAPTURE_LIMIT.saturating_sub(state.0.len()).min(n);
                        state.0.extend_from_slice(&chunk[..room]);
                        state.1 += n - room;
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
        let mut text = String::from_utf8_lossy(&state.0).into_owned();
        if state.1 > 0 {
            text.push_str(&format!(
                "\n[... {} more bytes not captured ...]\n",
                state.1
            ));
        }
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
