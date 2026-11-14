//! Commands that outlived their time limit, kept running in the background.
//!
//! A shell command still running at its time limit is handed to a
//! supervisor task instead of being killed. The supervisor writes what the
//! command has printed so far to a job directory every second and its final
//! output when it ends; [`JobOutput`] reads that directory, possibly from
//! another process. A worker process stays alive after answering until its
//! jobs are done, so a sandboxed command keeps its sandbox.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use hame_session::ToolSpec;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::process::Child;
use tokio::task::JoinHandle;

use crate::bash::{Capture, render};
use crate::{BoxFuture, Effect, GroupGuard, Tool, ToolContext, ToolOutput, parse_input};

pub const JOB_TOOL: &str = "job_output";
/// How often a running job's output so far is written out.
const SNAPSHOT_EVERY: Duration = Duration::from_secs(1);
/// A job still running after this long is killed: nobody is reading it.
const MAX_LIFETIME: Duration = Duration::from_secs(60 * 60);
const DEFAULT_WAIT_MS: u64 = 60_000;
const MAX_WAIT_MS: u64 = 600_000;
/// Bytes of a running job's output shown while it runs.
const SO_FAR_SHOWN: usize = 4_000;

const PARTIAL: &str = "partial.txt";
const RESULT: &str = "result.json";
const KILL: &str = "kill";

/// Supervisors running in this process.
static SUPERVISORS: Mutex<Vec<JoinHandle<()>>> = Mutex::new(Vec::new());

/// Where jobs keep their output: a directory a confined command may write.
fn jobs_dir() -> PathBuf {
    std::env::temp_dir().join("hame-jobs")
}

/// A command handed over at its time limit.
pub(crate) struct Running {
    pub command: String,
    pub child: Child,
    /// Armed when the command leads its own group; disarmed inside a worker,
    /// whose group belongs to the parent.
    pub group: GroupGuard,
    pub stdout: Capture,
    pub stderr: Capture,
    pub shared_group: bool,
}

#[derive(Serialize, Deserialize)]
struct Finished {
    text: String,
    is_error: bool,
    seconds: u64,
}

/// Words that introduce a job id in a shell result, and in nothing else.
const MOVED: &str = "moved to the background as job ";

/// The job a shell result says its command moved to, if any.
pub fn moved_to_background(text: &str) -> Option<&str> {
    let at = text.rfind(MOVED)? + MOVED.len();
    let id = &text[at..];
    let end = id
        .find(|c: char| !c.is_ascii_alphanumeric())
        .unwrap_or(id.len());
    (end > 0).then(|| &id[..end])
}

/// Whether a `job_output` result is the job's final output.
pub fn job_ended(text: &str) -> bool {
    text.rfind("[job ")
        .is_some_and(|at| text[at..].contains(" ended, "))
}

/// What a running command has printed, for the note that it moved to the
/// background: the end of it, where progress shows.
pub(crate) fn so_far(stdout: &Capture, stderr: &Capture) -> String {
    let mut text = stdout.snapshot();
    let err = stderr.snapshot();
    if !err.is_empty() {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str("[stderr]\n");
        text.push_str(&err);
    }
    let mut text = tail(&text, SO_FAR_SHOWN);
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

fn tail(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("[... {start} bytes before this ...]\n{}", &text[start..])
}

fn new_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |now| now.as_nanos());
    let mixed = (nanos as u64) ^ (u64::from(std::process::id()) << 40);
    format!("j{:08x}", (mixed ^ (mixed >> 29)) as u32)
}

/// A new job's id and directory, or `None` when the directory cannot be
/// made (a sandbox without a writable temp directory): the command is then
/// killed at its limit, as it used to be.
pub(crate) fn new_job() -> Option<(String, PathBuf)> {
    let id = new_id();
    let dir = jobs_dir().join(&id);
    std::fs::create_dir_all(&dir).ok()?;
    Some((id, dir))
}

/// Starts supervising `running` in the directory [`new_job`] made.
pub(crate) fn adopt(running: Running, dir: PathBuf) {
    let task = tokio::spawn(supervise(running, dir));
    let mut supervisors = SUPERVISORS.lock().expect("job list poisoned");
    supervisors.retain(|task| !task.is_finished());
    supervisors.push(task);
}

/// Whether this process supervises a job that has not finished.
pub fn jobs_running() -> bool {
    SUPERVISORS
        .lock()
        .expect("job list poisoned")
        .iter()
        .any(|task| !task.is_finished())
}

/// Waits until every job this process supervises has finished. A worker
/// calls this after answering, so its commands keep their sandbox.
pub async fn wait_for_jobs() {
    let tasks: Vec<JoinHandle<()>> =
        std::mem::take(&mut *SUPERVISORS.lock().expect("job list poisoned"));
    for task in tasks {
        let _ = task.await;
    }
}

/// Writes `contents` so a reader sees all of it or none.
fn write_whole(path: &Path, contents: &[u8]) {
    let staged = path.with_extension("tmp");
    if std::fs::write(&staged, contents).is_ok() {
        let _ = std::fs::rename(&staged, path);
    }
}

async fn supervise(mut running: Running, dir: PathBuf) {
    let started = Instant::now();
    let mut tick = tokio::time::interval(SNAPSHOT_EVERY);
    let ended = loop {
        tokio::select! {
            status = running.child.wait() => break status.ok(),
            _ = tick.tick() => {
                write_whole(
                    &dir.join(PARTIAL),
                    so_far(&running.stdout, &running.stderr).as_bytes(),
                );
                let asked = dir.join(KILL).exists();
                let expired = started.elapsed() >= MAX_LIFETIME;
                if asked || expired {
                    let why = if asked {
                        "[stopped on request]"
                    } else {
                        "[killed: still running an hour after it was moved to the background]"
                    };
                    let mut text = so_far(&running.stdout, &running.stderr);
                    text.push_str(why);
                    finish(&dir, text, true, started);
                    kill(&mut running).await;
                    return;
                }
            }
        }
    };
    // The command ended by itself; whatever it deliberately left running
    // in the background is let be, as for a command that ends in time.
    running.group.disarm();
    let Running {
        command,
        stdout,
        stderr,
        ..
    } = running;
    let (stdout, stderr) = tokio::join!(stdout.finish(), stderr.finish());
    let output = match ended {
        Some(status) => render(&command, status, &stdout, &stderr),
        None => ToolOutput::error("Lost track of the command."),
    };
    finish(&dir, output.text, output.is_error, started);
}

fn finish(dir: &Path, text: String, is_error: bool, started: Instant) {
    let finished = Finished {
        text,
        is_error,
        seconds: started.elapsed().as_secs(),
    };
    if let Ok(json) = serde_json::to_vec(&finished) {
        write_whole(&dir.join(RESULT), &json);
    }
}

/// Ends the command and everything it started.
async fn kill(running: &mut Running) {
    // Windows has no process groups: the tree is ended through the command.
    #[cfg(not(unix))]
    let _ = running.shared_group;
    #[cfg(unix)]
    if running.shared_group {
        // The group is this worker's own; it goes down with the command.
        drop(GroupGuard::new(Some(std::process::id())));
    }
    #[cfg(windows)]
    if let Some(pid) = running.child.id() {
        let _ = std::process::Command::new("taskkill")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    let _ = running.child.kill().await;
    // Outside a worker the armed guard kills the command's own group when
    // `running` drops.
}

/// Waits for a background job and reads its output, or stops it.
pub struct JobOutput;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JobInput {
    id: String,
    wait_ms: Option<u64>,
    #[serde(default)]
    kill: bool,
}

impl Tool for JobOutput {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: JOB_TOOL.into(),
            description: "Wait for a background job (a bash command that outlived its \
                          timeout) and return its output once it ends, or what it has printed \
                          so far if it is still running after wait_ms (default 60000, max \
                          600000). kill: true stops it."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "The job id bash reported."},
                    "wait_ms": {"type": "integer", "minimum": 0, "description": "How long to wait for it to end."},
                    "kill": {"type": "boolean", "description": "Stop the job."}
                },
                "required": ["id"],
                "additionalProperties": false
            }),
        }
    }

    fn effect(&self) -> Effect {
        Effect::ReadOnly
    }

    fn call<'a>(&'a self, input: &'a Value, _ctx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            let input: JobInput = match parse_input(input) {
                Ok(input) => input,
                Err(output) => return output,
            };
            let valid = input.id.len() <= 32 && input.id.chars().all(|c| c.is_ascii_alphanumeric());
            let dir = jobs_dir().join(&input.id);
            if !valid || !dir.is_dir() {
                return ToolOutput::error(format!(
                    "There is no job {:?}{}.",
                    input.id,
                    known_jobs()
                ));
            }
            if input.kill && !dir.join(RESULT).exists() {
                let _ = std::fs::write(dir.join(KILL), b"");
            }
            let wait = if input.kill {
                // The supervisor looks once a second.
                3 * SNAPSHOT_EVERY
            } else {
                Duration::from_millis(input.wait_ms.unwrap_or(DEFAULT_WAIT_MS).min(MAX_WAIT_MS))
            };
            let deadline = Instant::now() + wait;
            loop {
                if let Some(finished) = std::fs::read(dir.join(RESULT))
                    .ok()
                    .and_then(|raw| serde_json::from_slice::<Finished>(&raw).ok())
                {
                    let _ = std::fs::remove_dir_all(&dir);
                    let mut text = finished.text;
                    if !text.ends_with('\n') {
                        text.push('\n');
                    }
                    text.push_str(&format!(
                        "[job {} ended, {} s after it moved to the background]",
                        input.id, finished.seconds
                    ));
                    debug_assert!(job_ended(&text));
                    return ToolOutput {
                        text,
                        is_error: finished.is_error,
                    };
                }
                if Instant::now() >= deadline {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            let so_far = std::fs::read_to_string(dir.join(PARTIAL)).unwrap_or_default();
            ToolOutput::ok(format!(
                "{so_far}[job {} is still running after waiting {} ms; the output so far is \
                 above. Call {JOB_TOOL} again to keep waiting, or with kill: true to stop it.]",
                input.id,
                wait.as_millis()
            ))
        })
    }
}

fn known_jobs() -> String {
    let ids: Vec<String> = std::fs::read_dir(jobs_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    if ids.is_empty() {
        String::new()
    } else {
        format!("; jobs there are: {}", ids.join(", "))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::Bash;

    fn job_id(text: &str) -> String {
        let at = text.find("as job ").expect("a job id") + "as job ".len();
        text[at..]
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .collect()
    }

    #[tokio::test]
    async fn a_command_past_its_limit_finishes_in_the_background() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path());
        let started = Bash
            .call(
                &json!({"command": "echo begun; sleep 1; echo done > marker; echo finished", "timeout_ms": 200}),
                &ctx,
            )
            .await;
        assert!(!started.is_error, "{}", started.text);
        assert!(started.text.starts_with("begun\n"), "{}", started.text);
        let id = job_id(&started.text);

        let ended = JobOutput
            .call(&json!({"id": id, "wait_ms": 10_000}), &ctx)
            .await;
        assert!(!ended.is_error, "{}", ended.text);
        assert!(ended.text.contains("begun\nfinished\n"), "{}", ended.text);
        assert!(dir.path().join("marker").exists());
        // Read once: the job is gone afterwards.
        let again = JobOutput.call(&json!({"id": id}), &ctx).await;
        assert!(again.is_error);
    }

    #[tokio::test]
    async fn a_background_job_can_be_waited_on_in_pieces_and_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path());
        let started = Bash
            .call(
                &json!({"command": "echo tick; sleep 30; touch survived", "timeout_ms": 100}),
                &ctx,
            )
            .await;
        let id = job_id(&started.text);

        let waiting = JobOutput
            .call(&json!({"id": id, "wait_ms": 1_500}), &ctx)
            .await;
        assert!(!waiting.is_error);
        assert!(waiting.text.contains("tick"), "{}", waiting.text);
        assert!(waiting.text.contains("still running"), "{}", waiting.text);

        let stopped = JobOutput.call(&json!({"id": id, "kill": true}), &ctx).await;
        assert!(stopped.is_error);
        assert!(
            stopped.text.contains("[stopped on request]"),
            "{}",
            stopped.text
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(!dir.path().join("survived").exists());
    }

    #[tokio::test]
    async fn job_ids_cannot_reach_outside_the_job_directory() {
        let ctx = ToolContext::new(std::env::temp_dir());
        let out = JobOutput.call(&json!({"id": "../../etc"}), &ctx).await;
        assert!(out.is_error);
    }
}
