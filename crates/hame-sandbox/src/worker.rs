use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;

use hame_kernel::BoxFuture;
use hame_tools::{GroupGuard, Tool, ToolContext, ToolOutput, ToolRunner, ToolSet};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};

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
    /// The worker stays alive after answering, supervising a command that
    /// moved to the background; the parent does not wait for it to exit.
    #[serde(default)]
    background: bool,
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
    // Which bash the shell tool runs on Windows.
    "HAME_BASH",
];

/// The part of the harness's environment a program started for the agent
/// inherits: what programs need in order to run at all, plus the variables
/// named in `more`. Everything else, credentials included, is left out.
///
/// Names are compared without regard to case: Windows spells them any way
/// it likes (`Path`), and a program there that gets no `PATH` or
/// `SystemRoot` does not start.
pub fn inherited_environment(more: &[&str]) -> Vec<(OsString, OsString)> {
    std::env::vars_os()
        .filter(|(name, _)| {
            name.to_str().is_some_and(|name| {
                let name = name.to_ascii_uppercase();
                INHERITED_ENV.contains(&name.as_str())
                    || more.contains(&name.as_str())
                    || name.starts_with("LC_")
            })
        })
        .collect()
}
/// Body of the hidden `hame tool-worker` command: runs one tool call read from
/// stdin and writes its outcome to stdout. The process is already confined
/// by the time this runs.
pub async fn worker_main(tools: &ToolSet) -> u8 {
    #[cfg(windows)]
    crate::windows::keep_stdio_private();
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
    let Some(tool) = tools.for_worker(&request.tool) else {
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
        background: hame_tools::jobs_running(),
    };
    // One line: the parent reads up to the newline, not to the end, since
    // a worker with a background job keeps running after this.
    let mut stdout = std::io::stdout();
    let written = serde_json::to_writer(&mut stdout, &response)
        .map_err(std::io::Error::other)
        .and_then(|()| stdout.write_all(b"\n"))
        .and_then(|()| stdout.flush());
    hame_tools::wait_for_jobs().await;
    if written.is_ok() { 0 } else { 2 }
}

/// Workers kept alive after answering because a command they started moved
/// to the background. Dropping this (when the harness exits) ends them and
/// everything they started.
#[derive(Default)]
pub struct BackgroundWorkers(std::sync::Mutex<Vec<KeptWorker>>);

struct KeptWorker {
    child: tokio::process::Child,
    _group: GroupGuard,
    #[cfg(windows)]
    _tree: Option<crate::windows::ProcessTree>,
}

impl BackgroundWorkers {
    fn keep(&self, worker: KeptWorker) {
        let mut kept = self.0.lock().expect("worker list poisoned");
        kept.retain_mut(|worker| matches!(worker.child.try_wait(), Ok(None)));
        kept.push(worker);
    }
}

/// Runs each tool call in a fresh, confined `hame tool-worker` process.
pub struct SandboxedRunner {
    /// The `hame` executable.
    pub helper: PathBuf,
    pub policy: SandboxPolicy,
    /// Extra environment for the worker, e.g. secrets the user granted.
    pub env: Vec<(String, String)>,
    pub background: BackgroundWorkers,
}

impl SandboxedRunner {
    fn environment(&self) -> Vec<(OsString, OsString)> {
        let mut env = inherited_environment(&[]);
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
            // Node's own fetch looks at the proxy variables only when told to.
            env.push(("NODE_USE_ENV_PROXY".into(), "1".into()));
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
            // npm and pip keep their caches under the profile, where a
            // confined tool cannot write, and pip stalls on its check for a
            // newer pip.
            env.push(("npm_config_cache".into(), tmp.join("npm-cache").into()));
            env.push(("PIP_CACHE_DIR".into(), tmp.join("pip-cache").into()));
            env.push(("PIP_DISABLE_PIP_VERSION_CHECK".into(), "1".into()));
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
                tool: tool.worker_name(),
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
            // Windows has no process groups; a job object does the same work.
            #[cfg(windows)]
            let tree = child
                .raw_handle()
                .and_then(|process| crate::windows::ProcessTree::around(process).ok());
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(&request).await;
            }
            let stdout = child.stdout.take().expect("stdout is piped");
            let mut stderr = child.stderr.take().expect("stderr is piped");
            // Drained alongside, so a chatty worker cannot block on a full
            // pipe; only the start is kept, for the error message.
            let errors = tokio::spawn(async move {
                let mut kept = Vec::new();
                let mut chunk = [0u8; 4096];
                while let Ok(n) = stderr.read(&mut chunk).await {
                    if n == 0 {
                        break;
                    }
                    if kept.len() < 16 * 1024 {
                        kept.extend_from_slice(&chunk[..n]);
                    }
                }
                kept
            });
            let mut line = Vec::new();
            let read = tokio::io::BufReader::new(stdout)
                .read_until(b'\n', &mut line)
                .await;
            let response = read
                .ok()
                .and_then(|_| serde_json::from_slice::<WorkerResponse>(&line).ok());

            match response {
                Some(response) if response.background => {
                    // Its command runs on in the worker's sandbox; the
                    // worker ends when the command does, or with hame.
                    self.background.keep(KeptWorker {
                        child,
                        _group: group,
                        #[cfg(windows)]
                        _tree: tree,
                    });
                    ToolOutput {
                        text: response.text,
                        is_error: response.is_error,
                    }
                }
                Some(response) => {
                    let _ = child.wait().await;
                    if !response.kill_group {
                        group.disarm();
                        #[cfg(windows)]
                        if let Some(tree) = tree {
                            tree.release();
                        }
                    }
                    ToolOutput {
                        text: response.text,
                        is_error: response.is_error,
                    }
                }
                None => {
                    let status = match child.wait().await {
                        Ok(status) => status.to_string(),
                        Err(e) => {
                            return ToolOutput::error(format!(
                                "Lost track of the sandboxed worker: {e}."
                            ));
                        }
                    };
                    let stderr = errors.await.unwrap_or_default();
                    let stderr = String::from_utf8_lossy(&stderr);
                    ToolOutput::error(format!(
                        "The sandboxed worker failed before the tool finished ({}): {}",
                        status,
                        stderr.trim()
                    ))
                }
            }
        })
    }
}
