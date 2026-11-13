use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;

use hx_kernel::BoxFuture;
use hx_tools::{GroupGuard, Tool, ToolContext, ToolOutput, ToolRunner, ToolSet};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::AsyncWriteExt;

use crate::policy::{Network, SandboxPolicy, wrap};

#[derive(Serialize, Deserialize)]
struct WorkerRequest {
    tool: String,
    input: Value,
}

#[derive(Serialize, Deserialize)]
struct WorkerResponse {
    text: String,
    is_error: bool,
    /// The tool gave up on something it started (a timeout); the parent
    /// should kill the worker's process group.
    kill_group: bool,
}

/// Environment variables a worker inherits. Everything else, including any
/// credential the harness itself holds, is dropped.
const INHERITED_ENV: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "LANG",
    "TERM",
    "TMPDIR",
    "TZ",
    // Windows programs do not start without these.
    "SYSTEMROOT",
    "SYSTEMDRIVE",
    "WINDIR",
    "COMSPEC",
    "PATHEXT",
    "USERPROFILE",
    "USERNAME",
    "APPDATA",
    "LOCALAPPDATA",
    "PROGRAMDATA",
    "PROGRAMFILES",
    "PROGRAMFILES(X86)",
    "HOMEDRIVE",
    "HOMEPATH",
    "NUMBER_OF_PROCESSORS",
    "PROCESSOR_ARCHITECTURE",
    "OS",
];

/// Body of the hidden `hx tool-worker` command: runs one tool call read from
/// stdin and writes its outcome to stdout. The process is already confined
/// by the time this runs.
pub async fn worker_main(tools: &ToolSet) -> u8 {
    let mut raw = String::new();
    let request: WorkerRequest = match std::io::stdin()
        .read_to_string(&mut raw)
        .map_err(|e| e.to_string())
        .and_then(|_| serde_json::from_str(&raw).map_err(|e| e.to_string()))
    {
        Ok(request) => request,
        Err(e) => {
            eprintln!("tool-worker: invalid request: {e}");
            return 2;
        }
    };
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(e) => {
            eprintln!("tool-worker: no working directory: {e}");
            return 2;
        }
    };
    let Some(tool) = tools.get(&request.tool) else {
        eprintln!("tool-worker: unknown tool {:?}", request.tool);
        return 2;
    };

    let mut ctx = ToolContext::new(cwd);
    ctx.shared_group = true;
    let output = tool.call(&request.input, &ctx).await;
    let response = WorkerResponse {
        text: output.text,
        is_error: output.is_error,
        kill_group: ctx.group_kill_requested(),
    };
    let mut stdout = std::io::stdout();
    match serde_json::to_writer(&mut stdout, &response).map(|_| stdout.flush()) {
        Ok(Ok(())) => 0,
        _ => 2,
    }
}

/// Runs each tool call in a fresh, confined `hx tool-worker` process.
pub struct SandboxedRunner {
    /// The `hx` executable.
    pub helper: PathBuf,
    pub policy: SandboxPolicy,
    /// Extra environment for the worker, e.g. secrets the user granted.
    pub env: Vec<(String, String)>,
}

impl SandboxedRunner {
    fn environment(&self) -> Vec<(OsString, OsString)> {
        let mut env: Vec<(OsString, OsString)> = std::env::vars_os()
            .filter(|(name, _)| {
                name.to_str().is_some_and(|name| {
                    // Windows spells variable names in any case.
                    let name = name.to_ascii_uppercase();
                    INHERITED_ENV.contains(&name.as_str()) || name.starts_with("LC_")
                })
            })
            .collect();
        if let Network::Proxy { port } = self.policy.network {
            let proxy = format!("http://127.0.0.1:{port}");
            for name in [
                "HTTP_PROXY",
                "HTTPS_PROXY",
                "ALL_PROXY",
                "http_proxy",
                "https_proxy",
            ] {
                env.push((name.into(), proxy.clone().into()));
            }
            for name in ["NO_PROXY", "no_proxy"] {
                env.push((name.into(), "localhost,127.0.0.1,::1".into()));
            }
        }
        // Confined tools on Windows cannot write to the shared temp
        // directory; point them at the one prepared for them.
        #[cfg(windows)]
        if self.policy.is_confined()
            && let Some(tmp) = crate::windows::low_temp_dir()
        {
            for name in ["TEMP", "TMP"] {
                env.push((name.into(), tmp.clone().into()));
            }
        }
        for (name, value) in &self.env {
            env.push((name.into(), value.into()));
        }
        env
    }
}

impl ToolRunner for SandboxedRunner {
    fn run<'a>(
        &'a self,
        tool: &'a Arc<dyn Tool>,
        input: &'a Value,
        ctx: &'a ToolContext,
    ) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            let wrapped = match wrap(
                &self.policy,
                &self.helper,
                &self.helper,
                &[OsString::from("tool-worker")],
            ) {
                Ok(wrapped) => wrapped,
                Err(e) => return ToolOutput::error(format!("Not run: {e}.")),
            };
            let request = WorkerRequest {
                tool: tool.spec().name,
                input: input.clone(),
            };
            let request = serde_json::to_vec(&request).expect("requests serialize");

            let mut command = tokio::process::Command::new(&wrapped.program);
            command
                .args(&wrapped.args)
                .current_dir(&ctx.cwd)
                .env_clear()
                .envs(self.environment())
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            #[cfg(unix)]
            command.process_group(0);

            let mut child = match command.spawn() {
                Ok(child) => child,
                Err(e) => {
                    return ToolOutput::error(format!(
                        "Not run: cannot start the sandboxed worker: {e}."
                    ));
                }
            };
            // Armed until the worker reports a clean finish: a cancelled or
            // failed call takes down everything the tool started.
            let mut group = GroupGuard::new(child.id());
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(&request).await;
            }
            let output = match child.wait_with_output().await {
                Ok(output) => output,
                Err(e) => {
                    return ToolOutput::error(format!("Lost track of the sandboxed worker: {e}."));
                }
            };

            match serde_json::from_slice::<WorkerResponse>(&output.stdout) {
                Ok(response) => {
                    if !response.kill_group {
                        group.disarm();
                    }
                    ToolOutput {
                        text: response.text,
                        is_error: response.is_error,
                    }
                }
                Err(_) => {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    ToolOutput::error(format!(
                        "The sandboxed worker failed before the tool finished ({}): {}",
                        output.status,
                        stderr.trim()
                    ))
                }
            }
        })
    }
}
