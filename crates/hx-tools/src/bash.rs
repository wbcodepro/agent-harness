use std::process::Stdio;
use std::time::Duration;

use hx_session::ToolSpec;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

use crate::{BoxFuture, Effect, Tool, ToolContext, ToolOutput, parse_input, truncate_middle};

const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MAX_TIMEOUT_MS: u64 = 600_000;
/// Bytes kept per stream while the command runs; the rest is discarded.
const CAPTURE_LIMIT: usize = 1024 * 1024;
/// Bytes of each stream shown to the model.
const SHOWN_LIMIT: usize = 30_000;

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

            let mut command = Command::new("bash");
            command
                .arg("-c")
                .arg(&input.command)
                .current_dir(&ctx.cwd)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            // Its own process group, so everything it starts can be killed
            // together.
            #[cfg(unix)]
            command.process_group(0);

            let mut child = match command.spawn() {
                Ok(child) => child,
                Err(e) => return ToolOutput::error(format!("Cannot start bash: {e}")),
            };
            let mut group = GroupGuard(child.id());
            let stdout = child.stdout.take().expect("stdout is piped");
            let stderr = child.stderr.take().expect("stderr is piped");

            let run = async { tokio::join!(child.wait(), capture(stdout), capture(stderr)) };
            let (status, stdout, stderr) =
                match tokio::time::timeout(Duration::from_millis(timeout), run).await {
                    Ok((Ok(status), stdout, stderr)) => (status, stdout, stderr),
                    Ok((Err(e), _, _)) => {
                        return ToolOutput::error(format!("Lost track of the command: {e}"));
                    }
                    // `group` is dropped on return and kills the process group.
                    Err(_) => {
                        return ToolOutput::error(format!(
                            "The command did not finish within {timeout} ms and was killed."
                        ));
                    }
                };
            // The shell exited by itself. Leave anything it deliberately put
            // in the background running.
            group.disarm();

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

/// Reads a stream to its end, keeping at most [`CAPTURE_LIMIT`] bytes so a
/// runaway command cannot exhaust memory.
async fn capture(mut stream: impl AsyncRead + Unpin) -> String {
    let mut kept = Vec::new();
    let mut dropped = 0usize;
    let mut chunk = [0u8; 8192];
    loop {
        match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let room = CAPTURE_LIMIT.saturating_sub(kept.len()).min(n);
                kept.extend_from_slice(&chunk[..room]);
                dropped += n - room;
            }
        }
    }
    let mut text = String::from_utf8_lossy(&kept).into_owned();
    if dropped > 0 {
        text.push_str(&format!("\n[... {dropped} more bytes not captured ...]\n"));
    }
    text
}

/// Kills the command's whole process group when dropped, unless disarmed.
/// Dropping happens on timeout and when the tool call itself is cancelled.
struct GroupGuard(Option<u32>);

impl GroupGuard {
    fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for GroupGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.0 {
            let _ = std::process::Command::new("kill")
                .args(["-KILL", "--", &format!("-{pid}")])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::Instant;

    fn ctx(dir: &std::path::Path) -> ToolContext {
        ToolContext {
            cwd: dir.to_path_buf(),
        }
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
        assert!(out.text.contains("bytes omitted"));
    }
}
