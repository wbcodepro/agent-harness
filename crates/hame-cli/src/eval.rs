//! `hame eval`: run a set of tasks and report how the agent did.
//!
//! A task set is a manifest (`manifest.toml`, see
//! [`hame_eval::manifest`]) or a directory of tasks in the older layout:
//! each a directory holding a `task.toml` (the prompt and a shell command
//! that decides pass or fail) and an optional `files/` tree copied into a
//! fresh workspace. An optional `check/` tree is copied over the workspace
//! before the check runs, so an agent cannot pass by editing the tests,
//! and the suite directory is hidden from the agent's tools by the
//! sandbox, so it cannot read them either. Every task runs through the
//! real `hame run`, so what is measured is the product; `--command` runs
//! another agent on the same tasks for comparison.
//!
//! Every attempt is recorded, failures and timeouts included, with the
//! agent's time, the hidden checker's time and their sum (verified
//! completion latency) kept apart from setup. Model use comes from the
//! trace `hame run` writes (`trace.v1.jsonl`): every attempt, retries
//! included, priced from a dated tariff table. `--arm` runs the same tasks
//! under several configurations, interleaved, and compares them task by
//! task with a bootstrap confidence interval.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{ExitCode, Stdio};
use std::time::{Duration, Instant};

use clap::Args;
use hame_eval::arms::{Arm, build_arms, interleave};
use hame_eval::manifest::{self, TaskSpec, file_hashes, sha256_hex, tree_hash};
use hame_eval::report::{
    Observed, Outcome, Paired, RunRecord, Summary, claims_success, classify, compare, summarize,
};
use hame_session::{Block, Event, Record, SessionStore, TurnEndReason};
use hame_trace::{TRACE_FILE, TariffTable, account, read_trace};
use serde_json::json;
use tokio::io::AsyncReadExt;

#[derive(Args)]
pub struct EvalArgs {
    /// A task manifest (manifest.toml, or a directory holding one), or a
    /// directory of tasks with a task.toml each.
    suite: PathBuf,
    #[arg(long)]
    provider: Option<String>,
    #[arg(long)]
    model: Option<String>,
    /// Root of the provider's API, for a local or compatible server.
    #[arg(long)]
    base_url: Option<String>,
    /// Only run tasks whose name contains this.
    #[arg(long)]
    filter: Option<String>,
    /// Seconds allowed per task. Defaults to the task's own limit, then
    /// 600.
    #[arg(long)]
    timeout: Option<u64>,
    /// Run this shell command in each workspace instead of hame, to measure
    /// another agent on the same tasks. The prompt is in $HAME_EVAL_PROMPT.
    #[arg(long)]
    command: Option<String>,
    /// Also write the results as JSON to this file.
    #[arg(long)]
    json: Option<PathBuf>,
    /// Keep the workspace and session log of every failed task under this
    /// directory, to see what went wrong.
    #[arg(long)]
    keep: Option<PathBuf>,
    /// Run every task with this file as the agent's config.toml, to
    /// measure a configuration.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Run the whole suite this many times and report how often each task
    /// passed. One run of a weak model says little: the same task can pass
    /// and fail on the same code.
    #[arg(long, default_value_t = 1)]
    runs: usize,
    /// Use the prompts in this language where a task has them (e.g. vi).
    #[arg(long)]
    lang: Option<String>,
    /// A configuration to compare: NAME, or NAME=CONFIG.toml. Give two or
    /// more; runs are interleaved and each arm is compared with the first.
    #[arg(long = "arm")]
    arms: Vec<String>,
    /// A configuration override, [ARM:]section.key=value, e.g.
    /// `lean:context.dedupe_results=false`. Without ARM it applies to all.
    #[arg(long = "set")]
    sets: Vec<String>,
    /// Keep every run's trace under this directory. Defaults to a
    /// directory beside --json.
    #[arg(long)]
    traces: Option<PathBuf>,
    /// Seed for the bootstrap confidence intervals.
    #[arg(long, default_value_t = 1)]
    seed: u64,
    /// Bootstrap resamples for the confidence intervals.
    #[arg(long, default_value_t = 2000)]
    bootstrap: usize,
    /// Name for this experiment in the records. Defaults to a fresh id.
    #[arg(long)]
    experiment_id: Option<String>,
}

const DEFAULT_TIMEOUT_SECS: u64 = 600;
/// A checker or setup command that runs longer than this has failed: a
/// check that runs the agent's code can hang on it.
const SHELL_LIMIT: Duration = Duration::from_secs(300);
/// How long an agent past its time limit gets to stop after an interrupt,
/// so it can close its session and trace, before it is killed.
const GRACE: Duration = Duration::from_secs(5);
/// At most this many changed files are hashed into a record.
const MAX_ARTIFACT_HASHES: usize = 200;

/// The figures of one attempt that the summary lines and `hame probe`
/// read, taken from the agent's session log.
struct Tally {
    name: String,
    passed: bool,
    note: String,
    seconds: f64,
    requests: u64,
    tool_calls: u64,
    input_tokens: u64,
    cached_tokens: u64,
    output_tokens: u64,
    /// Whether a stronger model took over at some point.
    escalated: bool,
    /// The independent reviewer found something and sent the agent back.
    reviewed: bool,
    /// What the agent's own session shows was run after its last change:
    /// `passed`, `failing`, `unchecked` or `no-change`.
    evidence: &'static str,
}

/// One attempt: the session figures and the full record.
struct Attempt {
    tally: Tally,
    record: RunRecord,
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// Runs a shell command in `cwd` with `env` added. Returns whether it
/// exited 0 within [`SHELL_LIMIT`] and the last lines it printed.
async fn shell_with(command: &str, cwd: &Path, env: &[(&str, OsString)]) -> (bool, String) {
    // Windows installs Python as `python`; `python3` there is a stub that
    // only offers to open the Store.
    #[cfg(windows)]
    let command =
        &format!("python3 -c 0 >/dev/null 2>&1 || python3() {{ python \"$@\"; }}\n{command}");
    let child = tokio::process::Command::new(hame_kernel::bash_program())
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .envs(env.iter().map(|(name, value)| (*name, value)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn();
    let Ok(child) = child else {
        return (false, "could not start the command".into());
    };
    match tokio::time::timeout(SHELL_LIMIT, child.wait_with_output()).await {
        Ok(Ok(output)) => {
            let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
            text.push_str(&String::from_utf8_lossy(&output.stderr));
            let tail: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
            let tail = tail[tail.len().saturating_sub(3)..].join(" | ");
            (output.status.success(), tail)
        }
        Ok(Err(e)) => (false, e.to_string()),
        Err(_) => (false, "timed out".into()),
    }
}

/// Runs a task's setup command. One that does not finish in five minutes
/// counts as failed.
async fn shell(command: &str, cwd: &Path) -> bool {
    shell_with(command, cwd, &[]).await.0
}

/// Stops an agent past its limit: an interrupt first, so it can close its
/// session and trace, then a kill.
async fn interrupt(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        let _ = std::process::Command::new("kill")
            .args(["-INT", &pid.to_string()])
            .status();
        if tokio::time::timeout(GRACE, child.wait()).await.is_ok() {
            return;
        }
    }
    let _ = child.kill().await;
}

/// What the agent's own session logs show.
#[derive(Default)]
struct SessionFacts {
    found: bool,
    requests: u64,
    tool_calls: u64,
    input_tokens: u64,
    cached_tokens: u64,
    output_tokens: u64,
    escalated: bool,
    reviewed: bool,
    evidence: &'static str,
    last_turn_completed: bool,
    final_text: String,
    system_prompt: Option<String>,
    receipts: Vec<String>,
}

fn session_facts(home: &Path) -> SessionFacts {
    let mut facts = SessionFacts {
        evidence: "no-change",
        ..SessionFacts::default()
    };
    let store = SessionStore::new(home.join("sessions"));
    let mut longest: Vec<Record> = Vec::new();
    for header in store.list().unwrap_or_default() {
        let records = store.read(&header.id).unwrap_or_default();
        facts.found = true;
        for record in &records {
            match &record.event {
                // A summary is a model request too, and is paid for.
                Event::AssistantMessage { usage, .. } | Event::Compaction { usage, .. } => {
                    facts.requests += 1;
                    facts.input_tokens += usage.input_tokens + usage.cache_write_tokens;
                    facts.cached_tokens += usage.cache_read_tokens;
                    facts.output_tokens += usage.output_tokens;
                }
                Event::ToolCall { .. } => facts.tool_calls += 1,
                Event::ModelSwitched { .. } => facts.escalated = true,
                Event::UserMessage { content }
                    if matches!(content.first(), Some(Block::Text { text })
                        if text.starts_with(hame_multi::REVIEW_PREFIX)) =>
                {
                    facts.reviewed = true;
                }
                Event::ActionReceipt { receipt } => facts.receipts.push(receipt.op_id.clone()),
                _ => {}
            }
        }
        // The agent's own session is the long one; a subagent's is short.
        if records.len() > longest.len() {
            longest = records;
        }
    }
    facts.evidence = if longest.is_empty() {
        "no-change"
    } else {
        hame_verify::evidence(&longest).status()
    };
    facts.last_turn_completed = longest
        .iter()
        .rev()
        .find_map(|record| match &record.event {
            Event::TurnEnd { reason, .. } => Some(*reason == TurnEndReason::Completed),
            _ => None,
        })
        .unwrap_or(false);
    facts.final_text = longest
        .iter()
        .rev()
        .find_map(|record| match &record.event {
            Event::AssistantMessage { content, .. } => Some(
                content
                    .iter()
                    .filter_map(|block| match block {
                        Block::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<String>(),
            ),
            _ => None,
        })
        .unwrap_or_default();
    facts.system_prompt = longest.iter().find_map(|record| match &record.event {
        Event::SystemMessage { text } => Some(text.clone()),
        _ => None,
    });
    facts
}

/// What changed in the workspace: created or changed files with their new
/// hash, deleted ones as `deleted`.
fn changes(
    before: &BTreeMap<String, String>,
    after: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (path, hash) in after {
        if before.get(path) != Some(hash) {
            out.insert(path.clone(), hash.clone());
        }
    }
    for path in before.keys() {
        if !after.contains_key(path) {
            out.insert(path.clone(), "deleted".into());
        }
    }
    out
}

/// Protected paths (a file, or a directory ending in `/`) that changed.
fn forbidden_changes(
    protected: &[String],
    before: &BTreeMap<String, String>,
    after: &BTreeMap<String, String>,
) -> Vec<String> {
    let covers = |pattern: &str, path: &str| {
        path == pattern || (pattern.ends_with('/') && path.starts_with(pattern))
    };
    let mut found = Vec::new();
    for pattern in protected {
        let paths: std::collections::BTreeSet<&String> = before
            .keys()
            .chain(after.keys())
            .filter(|path| covers(pattern, path))
            .collect();
        for path in paths {
            match (before.get(path), after.get(path)) {
                (Some(a), Some(b)) if a != b => found.push(format!("changed {path}")),
                (Some(_), None) => found.push(format!("deleted {path}")),
                (None, Some(_)) => found.push(format!("created {path}")),
                _ => {}
            }
        }
    }
    found
}

/// Everything shared by the attempts of one eval invocation.
struct Context<'a> {
    args: &'a EvalArgs,
    keys: Vec<(String, String)>,
    scratch: PathBuf,
    experiment_id: String,
    task_set_version: String,
    hame_commit: Option<String>,
    hardware: Option<String>,
    tariffs: TariffTable,
    traces: Option<PathBuf>,
}

impl<'a> Context<'a> {
    fn new(
        home: &Path,
        args: &'a EvalArgs,
        task_set_version: String,
        traces: Option<PathBuf>,
    ) -> Result<Self, String> {
        // Each task gets an empty data directory, so the user's credentials
        // are handed over through the environment instead.
        let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
        let secrets = hame_app::load_secrets(home, &cwd)?;
        let keys = secrets
            .names()
            .filter_map(|name| Some((name.to_string(), secrets.get(name)?.to_string())))
            .collect();
        let scratch = std::env::temp_dir().join(format!("hame-eval-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&scratch);
        let hame_commit = std::env::var("HAME_COMMIT").ok().or_else(|| {
            std::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .output()
                .ok()
                .filter(|output| output.status.success())
                .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        });
        Ok(Self {
            args,
            keys,
            scratch,
            experiment_id: args
                .experiment_id
                .clone()
                .unwrap_or_else(|| uuid::Uuid::now_v7().simple().to_string()),
            task_set_version,
            hame_commit,
            hardware: std::thread::available_parallelism()
                .ok()
                .map(|cpus| format!("{} {} cpus", std::env::consts::ARCH, cpus)),
            tariffs: TariffTable::anthropic(),
            traces,
        })
    }

    fn base_record(&self, task: &TaskSpec, arm: &Arm, run_index: u32) -> RunRecord {
        RunRecord {
            experiment_id: self.experiment_id.clone(),
            task_set_version: self.task_set_version.clone(),
            task_id: task.id.clone(),
            run_index,
            arm: arm.name.clone(),
            hame_commit: self.hame_commit.clone(),
            os: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
            hardware: self.hardware.clone(),
            cold_or_warm: "cold".into(),
            enabled_features: arm.overrides.clone(),
            channels: task.channels.clone(),
            lang: self
                .args
                .lang
                .clone()
                .filter(|lang| task.prompts.contains_key(lang))
                .unwrap_or_else(|| "en".into()),
            model_version: arm
                .overrides
                .get("model.name")
                .and_then(|v| v.as_str().map(String::from))
                .or_else(|| self.args.model.clone()),
            // Approval questions are answered by --yes; nobody intervenes.
            human_interventions: Some(0),
            ..RunRecord::default()
        }
    }
}

async fn run_task(
    ctx: &Context<'_>,
    task: &TaskSpec,
    arm: &Arm,
    run_index: u32,
    keep: Option<&Path>,
) -> Result<Attempt, String> {
    let args = ctx.args;
    let name = &task.id;
    let mut record = ctx.base_record(task, arm, run_index);
    let mut tally = Tally {
        name: name.clone(),
        passed: false,
        note: String::new(),
        seconds: 0.0,
        requests: 0,
        tool_calls: 0,
        input_tokens: 0,
        cached_tokens: 0,
        output_tokens: 0,
        escalated: false,
        reviewed: false,
        evidence: "no-change",
    };
    let missing = task.missing_capabilities();
    if !missing.is_empty() {
        record.outcome = Outcome::Unsupported;
        record.failure_category = Some("unsupported".into());
        record.note = format!("needs {}", missing.join(", "));
        tally.note = record.note.clone();
        return Ok(Attempt { tally, record });
    }

    let dir = ctx
        .scratch
        .join(&arm.name)
        .join(name)
        .join(format!("run-{run_index}"));
    let (home, work) = (dir.join("home"), dir.join("work"));
    let trace_path = dir.join(TRACE_FILE);

    // Setup: a fresh workspace and data directory, the fixture, the
    // task's setup command.
    let setup_started = Instant::now();
    std::fs::create_dir_all(&home).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    // The agent must not be able to read how it will be graded: the task
    // set, with every checker and solution, is put out of its tools'
    // sight. (Where the sandbox confines only writes, this does nothing;
    // an agent run by `--command` is outside the sandbox altogether.)
    let config = with_unreadable(&arm.config, &task.hidden_root);
    if !config.is_empty() {
        std::fs::write(home.join("config.toml"), &config).map_err(|e| e.to_string())?;
    }
    if let Some(fixture) = &task.fixture {
        copy_tree(fixture, &work).map_err(|e| e.to_string())?;
    }
    let setup_ok = match &task.setup {
        Some(setup) => shell(setup, &work).await,
        None => true,
    };
    let before = file_hashes(&work).unwrap_or_default();
    record.initial_state_id = tree_hash(&work).ok();
    record.setup_ms = ms(setup_started.elapsed());

    let timeout = args
        .timeout
        .or(task.timeout_secs)
        .unwrap_or(DEFAULT_TIMEOUT_SECS);
    let prompt = task.prompt_for(args.lang.as_deref()).to_string();
    let mut agent_started = false;
    let mut exited_ok = false;
    let mut timed_out = false;
    if setup_ok {
        let mut command = match &args.command {
            Some(external) => {
                let mut command = tokio::process::Command::new(hame_kernel::bash_program());
                command
                    .arg("-c")
                    .arg(external)
                    .env("HAME_EVAL_PROMPT", &prompt);
                command
            }
            None => {
                let mut command = tokio::process::Command::new(
                    std::env::current_exe().map_err(|e| e.to_string())?,
                );
                command.arg("--home").arg(&home).args(["run", "--yes"]);
                if let Some(provider) = &args.provider {
                    command.args(["--provider", provider]);
                }
                if let Some(model) = &args.model {
                    command.args(["--model", model]);
                }
                if let Some(base_url) = &args.base_url {
                    command.args(["--base-url", base_url]);
                }
                command.arg("--trace").arg(&trace_path);
                command.arg(&prompt);
                command
            }
        };
        command
            .current_dir(&work)
            .envs(ctx.keys.iter().cloned())
            .env_remove("HAME_TRACE")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let started = Instant::now();
        match command.spawn() {
            Err(e) => tally.note = format!("could not run: {e}"),
            Ok(mut child) => {
                agent_started = true;
                let stderr = child.stderr.take();
                let reader = tokio::spawn(async move {
                    let mut text = String::new();
                    if let Some(mut stderr) = stderr {
                        let _ = stderr.read_to_string(&mut text).await;
                    }
                    text
                });
                let status =
                    match tokio::time::timeout(Duration::from_secs(timeout), child.wait()).await {
                        Ok(status) => status.ok(),
                        Err(_) => {
                            timed_out = true;
                            interrupt(&mut child).await;
                            None
                        }
                    };
                record.agent_ms = ms(started.elapsed());
                // Something the agent started may still hold its stderr.
                let stderr = tokio::time::timeout(GRACE, reader)
                    .await
                    .ok()
                    .and_then(Result::ok)
                    .unwrap_or_default();
                exited_ok = status.is_some_and(|s| s.success());
                tally.note = if timed_out {
                    "timed out".to_string()
                } else if !exited_ok {
                    stderr
                        .lines()
                        .rev()
                        .find(|line| line.starts_with("hame:"))
                        .unwrap_or("the agent exited with an error")
                        .to_string()
                } else {
                    String::new()
                };
            }
        }
    } else {
        tally.note = "setup failed".into();
    }

    // Verification: what changed, the protected files, the hidden check.
    let verify_started = Instant::now();
    let after = file_hashes(&work).unwrap_or_default();
    let mut changed = changes(&before, &after);
    if changed.len() > MAX_ARTIFACT_HASHES {
        changed = changed.into_iter().take(MAX_ARTIFACT_HASHES).collect();
    }
    record.artifact_hashes = changed;
    record.forbidden_effects = forbidden_changes(&task.unchanged, &before, &after);
    let mut check_passed = false;
    if setup_ok {
        if let Some(overlay) = &task.overlay {
            copy_tree(overlay, &work).map_err(|e| e.to_string())?;
        }
        let mut env: Vec<(&str, OsString)> = vec![("HAME_TASK_ID", task.id.clone().into())];
        if let Some(check_dir) = &task.check_dir {
            let absolute = check_dir
                .canonicalize()
                .unwrap_or_else(|_| check_dir.clone());
            env.push(("HAME_CHECK_DIR", absolute.into_os_string()));
        }
        let (passed, tail) = shell_with(&task.check, &work, &env).await;
        check_passed = passed;
        if !passed && tally.note.is_empty() {
            tally.note = if tail.is_empty() {
                "check failed".into()
            } else {
                format!("check failed: {tail}")
            };
        }
    }
    record.verification_ms = ms(verify_started.elapsed());
    record.total_ms = record.agent_ms + record.verification_ms;
    record.check_passed = check_passed;

    // What the agent's logs say.
    let facts = session_facts(&home);
    tally.requests = facts.requests;
    tally.tool_calls = facts.tool_calls;
    tally.input_tokens = facts.input_tokens;
    tally.cached_tokens = facts.cached_tokens;
    tally.output_tokens = facts.output_tokens;
    tally.escalated = facts.escalated;
    tally.reviewed = facts.reviewed;
    tally.evidence = facts.evidence;
    record.receipt_ids = facts.receipts.clone();
    record.prompt_version = facts
        .system_prompt
        .as_ref()
        .map(|text| format!("sha256:{}", &sha256_hex(text.as_bytes())[..12]));
    record.claimed_success = if args.command.is_some() {
        // Another agent's only claim is its exit status.
        exited_ok && !timed_out
    } else {
        !timed_out && claims_success(exited_ok, facts.last_turn_completed, &facts.final_text)
    };
    if trace_path.is_file() {
        let records = read_trace(&trace_path).map_err(|e| e.to_string())?;
        record.apply_account(&account(&records, &ctx.tariffs));
        if let Some(traces) = &ctx.traces {
            let kept = traces
                .join(&arm.name)
                .join(name)
                .join(format!("run-{run_index}.{TRACE_FILE}"));
            if let Some(parent) = kept.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if std::fs::copy(&trace_path, &kept).is_ok() {
                record.trace_reference = Some(kept.to_string_lossy().into_owned());
            }
        }
    } else {
        record.usage_complete = false;
    }

    let (outcome, category) = classify(Observed {
        setup_ok,
        agent_started,
        agent_exited_ok: exited_ok,
        timed_out,
        check_passed,
        forbidden_effects: record.forbidden_effects.len(),
        claimed_success: record.claimed_success,
    });
    record.outcome = outcome;
    record.failure_category = category.map(String::from);
    if !record.forbidden_effects.is_empty() && tally.note.is_empty() {
        tally.note = format!("forbidden: {}", record.forbidden_effects.join(", "));
    }
    record.note = tally.note.clone();
    tally.passed = outcome == Outcome::Pass;
    tally.seconds = record.total_ms / 1000.0;

    if let (false, Some(keep)) = (tally.passed, keep) {
        let kept = keep.join(name);
        let _ = std::fs::remove_dir_all(&kept);
        if let Err(e) = copy_tree(&dir, &kept) {
            eprintln!("hame: could not keep {name}: {e}");
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    Ok(Attempt { tally, record })
}

fn ms(duration: Duration) -> f64 {
    (duration.as_secs_f64() * 1000.0 * 1000.0).round() / 1000.0
}

/// What in an agent's error says the provider turned the account away:
/// no credit, no valid key, no permission. `None` for anything that could
/// be the task's or the model's doing.
fn account_problem(note: &str) -> Option<&'static str> {
    [
        ("insufficient_quota", "the account is out of quota"),
        ("credit_balance", "the account is out of credit"),
        ("billing", "a billing problem"),
        ("invalid_api_key", "the API key is not valid"),
        ("HTTP 401", "the API key was not accepted"),
        ("HTTP 402", "payment is required"),
        ("HTTP 403", "the account may not use this model"),
    ]
    .into_iter()
    .find(|(marker, _)| note.contains(marker))
    .map(|(_, problem)| problem)
}

/// The tasks' configuration, with the user's acceptance of a sandbox that
/// confines only writes carried over from their own config.toml. Each task
/// runs with a data directory of its own, and on Windows the agent refuses
/// to start confined until `allow_partial` is set there.
fn with_accepted_partial_sandbox(home: &Path, config: &str) -> String {
    let accepted = std::fs::read_to_string(home.join("config.toml"))
        .ok()
        .and_then(|text| text.parse::<toml::Table>().ok())
        .and_then(|user| user.get("sandbox")?.get("allow_partial")?.as_bool())
        .unwrap_or(false);
    let Ok(mut table) = config.parse::<toml::Table>() else {
        return config.to_string();
    };
    if !accepted {
        return config.to_string();
    }
    let sandbox = table
        .entry("sandbox")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    match sandbox.as_table_mut() {
        Some(sandbox) if !sandbox.contains_key("allow_partial") => {
            sandbox.insert("allow_partial".into(), toml::Value::Boolean(true));
            toml::to_string(&table).unwrap_or_else(|_| config.to_string())
        }
        _ => config.to_string(),
    }
}

/// `config` with the suite that holds the task at `dir` added to what the
/// sandbox hides from tools.
#[cfg(test)]
fn with_suite_unreadable(config: &str, dir: &Path) -> String {
    with_unreadable(config, dir.parent().unwrap_or(dir))
}

/// `config` with `root` added to what the sandbox hides from tools.
fn with_unreadable(config: &str, root: &Path) -> String {
    let Ok(mut table) = config.parse::<toml::Table>() else {
        return config.to_string();
    };
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let sandbox = table
        .entry("sandbox")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    let Some(sandbox) = sandbox.as_table_mut() else {
        return config.to_string();
    };
    let hidden = sandbox
        .entry("unreadable")
        .or_insert_with(|| toml::Value::Array(Vec::new()));
    if let Some(hidden) = hidden.as_array_mut() {
        hidden.push(toml::Value::String(root.to_string_lossy().into_owned()));
    }
    toml::to_string(&table).unwrap_or_else(|_| config.to_string())
}

fn print_progress(attempt: &Attempt, arms: usize) {
    let record = &attempt.record;
    let arm = if arms > 1 {
        format!("[{}] ", record.arm)
    } else {
        String::new()
    };
    eprintln!(
        "{arm}{} ... {} (agent {:.0}s, check {:.1}s){}",
        record.task_id,
        record.outcome.as_str(),
        record.agent_ms / 1000.0,
        record.verification_ms / 1000.0,
        if attempt.tally.note.is_empty() {
            String::new()
        } else {
            format!(" — {}", attempt.tally.note)
        }
    );
}

/// Runs every task once with `config` as the agent's configuration file.
/// For `hame probe`.
async fn run_suite(
    home: &Path,
    args: &EvalArgs,
    tasks: &[TaskSpec],
    config: &str,
    keep: Option<&Path>,
) -> Result<Vec<Tally>, String> {
    let ctx = Context::new(home, args, "probe".into(), None)?;
    let arm = Arm {
        name: "probe".into(),
        config: with_accepted_partial_sandbox(home, config),
        overrides: BTreeMap::new(),
        source: None,
    };
    let mut tallies = Vec::new();
    for task in tasks {
        let attempt = run_task(&ctx, task, &arm, 1, keep).await?;
        print_progress(&attempt, 1);
        if let Some(problem) = account_problem(&attempt.tally.note) {
            let _ = std::fs::remove_dir_all(&ctx.scratch);
            return Err(format!(
                "stopped at {}: the provider refused the request ({problem}). The results \
                 so far are incomplete; nothing after this point was measured.",
                task.id
            ));
        }
        tallies.push(attempt.tally);
    }
    let _ = std::fs::remove_dir_all(&ctx.scratch);
    Ok(tallies)
}

fn opt<T: std::fmt::Display>(value: Option<T>) -> String {
    value.map_or_else(|| "-".into(), |v| v.to_string())
}

fn print_records(records: &[RunRecord]) {
    println!(
        "{:<22} {:<10} {:>3} {:<13} {:>8} {:>8} {:>5} {:>5} {:>9} {:>9} {:>8} {:>7} {:>9} {:>9}",
        "task",
        "arm",
        "run",
        "outcome",
        "agent_s",
        "check_s",
        "reqs",
        "retry",
        "uncached",
        "cache_rd",
        "cache_wr",
        "output",
        "req_bytes",
        "cost_usd"
    );
    for r in records {
        println!(
            "{:<22} {:<10} {:>3} {:<13} {:>8.1} {:>8.2} {:>5} {:>5} {:>9} {:>9} {:>8} {:>7} {:>9} {:>9}",
            r.task_id,
            r.arm,
            r.run_index,
            r.outcome.as_str(),
            r.agent_ms / 1000.0,
            r.verification_ms / 1000.0,
            opt(r.model_calls),
            opt(r.retries),
            opt(r.uncached_input),
            opt(r.cached_input),
            opt(r.cache_write),
            opt(r.output),
            opt(r.request_bytes),
            opt(r.billed_cost.map(|c| format!("{c:.4}")))
        );
    }
}

fn print_summary(s: &Summary) {
    let rate = s
        .strict_completion_rate
        .map_or_else(|| "-".into(), |r| format!("{:.0}%", r * 100.0));
    let secs = |ms: Option<f64>| ms.map_or_else(|| "-".into(), |ms| format!("{:.1}s", ms / 1000.0));
    println!(
        "[{}] strict completion {}/{} ({rate}) · false success {} · fail {} · timeout {} · error {}{}",
        s.arm,
        s.passes,
        s.attempts,
        s.false_successes,
        s.fails,
        s.timeouts,
        s.errors,
        if s.unsupported > 0 {
            format!(" · unsupported (not counted) {}", s.unsupported)
        } else {
            String::new()
        }
    );
    println!(
        "[{}] verified latency p50 {} p95 {} · tokens: {} uncached, {} cache read, {} cache write, {} output{} · {} model calls, {} retries",
        s.arm,
        secs(s.verified_latency_ms_p50),
        secs(s.verified_latency_ms_p95),
        s.uncached_input,
        s.cached_input,
        s.cache_write,
        s.output,
        if s.usage_complete {
            ""
        } else {
            " (incomplete: lower bounds)"
        },
        s.model_calls,
        s.retries,
    );
    println!(
        "[{}] cost {} · per verified success {} · request bytes {}",
        s.arm,
        s.cost_usd.map_or_else(
            || format!("unknown (known part ${:.4})", s.known_cost_usd),
            |c| format!("${c:.4}")
        ),
        s.cost_per_success
            .map_or_else(|| "undefined".into(), |c| format!("${c:.4}")),
        opt(s.request_bytes),
    );
}

fn print_comparison(paired: &[Paired]) {
    println!(
        "\n{:<16} {:>10} {:>6} {:>12} {:>24}",
        "metric", "arm-base", "tasks", "mean diff", "95% CI (by task)"
    );
    for p in paired {
        let fmt = |v: Option<f64>| v.map_or_else(|| "-".into(), |v| format!("{v:.4}"));
        println!(
            "{:<16} {:>10} {:>6} {:>12} {:>24}",
            p.metric,
            format!("{}-{}", p.arm, p.baseline),
            p.tasks,
            fmt(p.mean_diff),
            match (p.ci95_low, p.ci95_high) {
                (Some(lo), Some(hi)) => format!("[{lo:.4}, {hi:.4}]"),
                _ => "-".into(),
            }
        );
    }
}

pub async fn run(home: &Path, args: EvalArgs) -> Result<ExitCode, String> {
    let set = manifest::load(&args.suite, args.filter.as_deref())?;
    let base = match &args.config {
        Some(path) => std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?,
        None => String::new(),
    };
    let arms: Vec<Arm> = build_arms(&base, &args.arms, &args.sets, |file| {
        std::fs::read_to_string(file).map_err(|e| format!("cannot read {file}: {e}"))
    })?
    .into_iter()
    .map(|mut arm| {
        arm.config = with_accepted_partial_sandbox(home, &arm.config);
        arm
    })
    .collect();
    let traces = args
        .traces
        .clone()
        .or_else(|| args.json.as_ref().map(|json| json.with_extension("traces")));
    let ctx = Context::new(home, &args, set.version.clone(), traces)?;
    let runs = args.runs.max(1) as u32;

    let mut attempts: Vec<Attempt> = Vec::new();
    let mut last_run = 0;
    for (run_index, task_index, arm_index) in interleave(runs, set.tasks.len(), arms.len()) {
        if runs > 1 && run_index != last_run {
            eprintln!("== run {run_index} of {runs}");
        }
        last_run = run_index;
        let (task, arm) = (&set.tasks[task_index], &arms[arm_index]);
        // Each round and arm keeps its failures apart, so none overwrites
        // another.
        let keep = args.keep.as_ref().map(|keep| {
            let mut keep = keep.clone();
            if arms.len() > 1 {
                keep = keep.join(&arm.name);
            }
            if runs > 1 {
                keep = keep.join(format!("run-{run_index}"));
            }
            keep
        });
        let attempt = run_task(&ctx, task, arm, run_index, keep.as_deref()).await?;
        print_progress(&attempt, arms.len());
        // A refusal about the account says nothing about the task. Going on
        // would count every remaining task as failed.
        if let Some(problem) = account_problem(&attempt.tally.note) {
            let _ = std::fs::remove_dir_all(&ctx.scratch);
            return Err(format!(
                "stopped at {}: the provider refused the request ({problem}). The results \
                 so far are incomplete; nothing after this point was measured.",
                task.id
            ));
        }
        attempts.push(attempt);
    }
    let _ = std::fs::remove_dir_all(&ctx.scratch);

    let records: Vec<RunRecord> = attempts.iter().map(|a| a.record.clone()).collect();
    print_records(&records);
    if runs > 1 {
        println!("\n{:<24} {:<10} {:>8}", "task", "arm", "passed");
        for arm in &arms {
            for task in &set.tasks {
                let passed = records
                    .iter()
                    .filter(|r| r.arm == arm.name && r.task_id == task.id)
                    .filter(|r| r.outcome == Outcome::Pass)
                    .count();
                println!("{:<24} {:<10} {passed:>5}/{runs}", task.id, arm.name);
            }
        }
    }
    println!();
    let summaries: Vec<Summary> = arms
        .iter()
        .map(|arm| {
            let mine: Vec<&RunRecord> = records.iter().filter(|r| r.arm == arm.name).collect();
            summarize(&arm.name, &mine)
        })
        .collect();
    for summary in &summaries {
        print_summary(summary);
    }
    let mut comparisons = Vec::new();
    for arm in arms.iter().skip(1) {
        let paired = compare(
            &records,
            &arms[0].name,
            &arm.name,
            args.bootstrap,
            args.seed,
        );
        print_comparison(&paired);
        comparisons.extend(paired);
    }

    let tallies: Vec<&Tally> = attempts.iter().map(|a| &a.tally).collect();
    // The agent stopping is one thing and the work being right another:
    // how often it stopped with a passing check behind it, and how often
    // that agreed with the task's own check.
    let stopped_unproven = tallies
        .iter()
        .filter(|o| matches!(o.evidence, "unchecked" | "failing"))
        .count();
    let false_done = tallies
        .iter()
        .filter(|o| o.evidence == "passed" && !o.passed)
        .count();
    if stopped_unproven + false_done > 0 {
        println!(
            "{stopped_unproven} task runs ended with changes and no passing check of the \
             agent's own; {false_done} ended with a passing check of its own and still failed \
             the task's check"
        );
    }
    let escalated = tallies.iter().filter(|o| o.escalated).count();
    if escalated > 0 {
        println!(
            "{escalated} of {} task runs were handed to the stronger model ({} of them passed)",
            tallies.len(),
            tallies.iter().filter(|o| o.escalated && o.passed).count()
        );
    }
    let reviewed = tallies.iter().filter(|o| o.reviewed).count();
    if reviewed > 0 {
        println!(
            "{reviewed} of {} task runs were sent back by the reviewer ({} of them passed)",
            tallies.len(),
            tallies.iter().filter(|o| o.reviewed && o.passed).count()
        );
    }

    let counted: Vec<&RunRecord> = records
        .iter()
        .filter(|r| r.outcome != Outcome::Unsupported)
        .collect();
    let passed = counted
        .iter()
        .filter(|r| r.outcome == Outcome::Pass)
        .count();
    if let Some(path) = &args.json {
        let report = json!({
            "schema": "hame.eval.v2",
            "experiment_id": ctx.experiment_id,
            "task_set": set.name,
            "task_set_version": set.version,
            "tariff_date": hame_trace::TARIFF_DATE,
            "tariff_source": hame_trace::TARIFF_SOURCE,
            "seed": args.seed,
            "bootstrap_resamples": args.bootstrap,
            "arms": arms.iter().map(|arm| json!({
                "name": arm.name, "source": arm.source, "overrides": arm.overrides,
            })).collect::<Vec<_>>(),
            "records": records,
            "summaries": summaries,
            "comparisons": comparisons,
            // The fields of the first report format, kept for readers of it.
            "provider": args.provider,
            "model": args.model,
            "runs": runs,
            "passed": passed,
            "total": counted.len(),
            "tasks": attempts.iter().map(|a| { let o = &a.tally; json!({
                "name": o.name, "passed": o.passed, "note": o.note, "seconds": o.seconds,
                "requests": o.requests, "tool_calls": o.tool_calls,
                "input_tokens": o.input_tokens, "cached_tokens": o.cached_tokens,
                "output_tokens": o.output_tokens, "escalated": o.escalated,
                "reviewed": o.reviewed,
                "evidence": o.evidence,
                "arm": a.record.arm, "outcome": a.record.outcome,
            })}).collect::<Vec<_>>(),
        });
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(path, format!("{report:#}\n"))
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    }
    Ok(if passed == counted.len() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

#[derive(Args)]
pub struct ProbeArgs {
    /// Directory of tasks to try each setting on.
    suite: PathBuf,
    #[arg(long)]
    provider: Option<String>,
    #[arg(long)]
    model: Option<String>,
    /// Root of the provider's API, for a local or compatible server.
    #[arg(long)]
    base_url: Option<String>,
    /// Only run tasks whose name contains this.
    #[arg(long)]
    filter: Option<String>,
    /// Seconds allowed per task.
    #[arg(long, default_value_t = 600)]
    timeout: u64,
    /// Keep the workspace and session log of every failed task under this
    /// directory, one subdirectory per setting.
    #[arg(long)]
    keep: Option<PathBuf>,
    /// Choose the setting on every task. Without this, every third task
    /// is held back: settings are compared on the rest, and the winner is
    /// then run once on the held-back tasks, which it was not chosen on.
    #[arg(long)]
    no_holdout: bool,
}

/// Fewer tasks than this are not split: too few would be left to choose on.
const MIN_TASKS_TO_HOLD_OUT: usize = 6;

/// One way of setting a model up, and how the suite went with it.
struct Trial {
    edit_format: &'static str,
    tools: &'static str,
    passed: usize,
    tokens: u64,
    seconds: f64,
}

impl Trial {
    /// More tasks solved wins; among equals, fewer tokens.
    fn beats(&self, other: &Trial) -> bool {
        (self.passed, std::cmp::Reverse(self.tokens))
            > (other.passed, std::cmp::Reverse(other.tokens))
    }
}

fn best(trials: &[Trial]) -> &Trial {
    trials
        .iter()
        .reduce(|best, trial| if trial.beats(best) { trial } else { best })
        .expect("at least one trial")
}

/// `hame probe`: find out how a model works best.
///
/// Which edit format a model handles, and whether it copes with the full
/// tool list, differs from model to model and cannot be told from its name.
/// This runs a suite once per setting and prints the configuration that
/// solved the most, at the lowest cost.
pub async fn probe(home: &Path, args: ProbeArgs) -> Result<ExitCode, String> {
    let all = manifest::load(&args.suite, args.filter.as_deref())?.tasks;
    // A setting chosen on a set of tasks looks better on that set than it
    // is. Every third task (by name order) is kept out of the choosing.
    let hold_out = !args.no_holdout && all.len() >= MIN_TASKS_TO_HOLD_OUT;
    let (held_back, tasks): (Vec<_>, Vec<_>) = all
        .into_iter()
        .enumerate()
        .partition(|(index, _)| hold_out && index % 3 == 2);
    let held_back: Vec<TaskSpec> = held_back.into_iter().map(|(_, task)| task).collect();
    let tasks: Vec<TaskSpec> = tasks.into_iter().map(|(_, task)| task).collect();
    let eval = EvalArgs {
        suite: args.suite.clone(),
        provider: args.provider.clone(),
        model: args.model.clone(),
        base_url: args.base_url.clone(),
        filter: args.filter.clone(),
        timeout: Some(args.timeout),
        command: None,
        json: None,
        keep: None,
        config: None,
        runs: 1,
        lang: None,
        arms: Vec::new(),
        sets: Vec::new(),
        traces: None,
        seed: 1,
        bootstrap: 0,
        experiment_id: None,
    };
    let mut trials: Vec<Trial> = Vec::new();
    let try_setting = async |edit_format: &'static str, tools: &'static str| {
        eprintln!("== edit_format = {edit_format}, tools = {tools}");
        let config = format!("[model]\nedit_format = \"{edit_format}\"\ntools = \"{tools}\"\n");
        let kept = args
            .keep
            .as_ref()
            .map(|keep| keep.join(format!("{edit_format}-{tools}")));
        let outcomes = run_suite(home, &eval, &tasks, &config, kept.as_deref()).await?;
        Ok::<Trial, String>(Trial {
            edit_format,
            tools,
            passed: outcomes.iter().filter(|o| o.passed).count(),
            tokens: outcomes
                .iter()
                .map(|o| o.input_tokens + o.cached_tokens + o.output_tokens)
                .sum(),
            seconds: outcomes.iter().map(|o| o.seconds).sum(),
        })
    };
    // Every edit format with all tools, then the best format again with
    // the short tool list: five runs instead of eight.
    for edit_format in ["search-replace", "whole-file", "apply-patch", "hashline"] {
        trials.push(try_setting(edit_format, "full").await?);
    }
    let format = best(&trials).edit_format;
    trials.push(try_setting(format, "minimal").await?);

    println!(
        "{:<16} {:<8} {:>7} {:>10} {:>8}",
        "edit_format", "tools", "passed", "tokens", "seconds"
    );
    for trial in &trials {
        println!(
            "{:<16} {:<8} {:>4}/{:<2} {:>10} {:>8.0}",
            trial.edit_format,
            trial.tools,
            trial.passed,
            tasks.len(),
            trial.tokens,
            trial.seconds
        );
    }
    let winner = best(&trials);
    println!(
        "\nBest on this suite. Put it in ~/.hame/config.toml:\n\n[model]\nedit_format = \"{}\"\ntools = \"{}\"",
        winner.edit_format, winner.tools
    );
    if trials.iter().all(|trial| trial.passed == winner.passed) {
        println!(
            "\nEvery setting solved the same number of tasks; the choice above only saves \
             tokens. A harder suite would tell them apart."
        );
    }
    if !held_back.is_empty() {
        eprintln!(
            "== the chosen setting on the {} tasks held back",
            held_back.len()
        );
        let config = format!(
            "[model]\nedit_format = \"{}\"\ntools = \"{}\"\n",
            winner.edit_format, winner.tools
        );
        let kept = args.keep.as_ref().map(|keep| keep.join("held-back"));
        let outcomes = run_suite(home, &eval, &held_back, &config, kept.as_deref()).await?;
        println!(
            "\nOn the {} tasks it was not chosen on, this setting solved {}. That is the \
             number to expect; the table above is from the tasks it was chosen on.",
            held_back.len(),
            outcomes.iter().filter(|o| o.passed).count()
        );
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_setting_that_solves_more_wins_and_tokens_break_ties() {
        let trial = |edit_format, passed, tokens| Trial {
            edit_format,
            tools: "full",
            passed,
            tokens,
            seconds: 0.0,
        };
        let trials = [
            trial("search-replace", 5, 900),
            trial("whole-file", 7, 5000),
            trial("apply-patch", 7, 3000),
        ];
        assert_eq!(best(&trials).edit_format, "apply-patch");
        // A tie on both keeps the earlier, more ordinary setting.
        // An account refused is not a task failed.
        assert_eq!(
            account_problem(
                "hame: unexpected provider response: {\"code\":\"credit_balance_exhausted\"}"
            ),
            Some("the account is out of credit")
        );
        assert_eq!(
            account_problem("hame: the provider answered HTTP 401: nope"),
            Some("the API key was not accepted")
        );
        assert_eq!(account_problem("check failed"), None);
        assert_eq!(
            account_problem("hame: the turn was stopped after 50 steps without finishing"),
            None
        );
        let tied = [
            trial("search-replace", 7, 3000),
            trial("apply-patch", 7, 3000),
        ];
        assert_eq!(best(&tied).edit_format, "search-replace");
    }

    #[test]
    fn tasks_inherit_the_users_acceptance_of_a_partial_sandbox() {
        let dir = tempfile::tempdir().unwrap();
        let task = "[model]\ntools = \"minimal\"\n";
        // Not accepted: the configuration is passed on untouched.
        assert_eq!(with_accepted_partial_sandbox(dir.path(), task), task);
        let user = dir.path().join("config.toml");
        std::fs::write(&user, "[sandbox]\nallow_partial = false\n").unwrap();
        assert_eq!(with_accepted_partial_sandbox(dir.path(), task), task);

        std::fs::write(
            &user,
            "[sandbox]\nallow_partial = true\nwritable = [\"/elsewhere\"]\n",
        )
        .unwrap();
        // Only the acceptance is carried over, nothing else of the user's.
        let merged: toml::Table = with_accepted_partial_sandbox(dir.path(), task)
            .parse()
            .unwrap();
        assert_eq!(merged["model"]["tools"].as_str(), Some("minimal"));
        assert_eq!(merged["sandbox"].as_table().unwrap().len(), 1);
        assert_eq!(merged["sandbox"]["allow_partial"].as_bool(), Some(true));
        let empty: toml::Table = with_accepted_partial_sandbox(dir.path(), "")
            .parse()
            .unwrap();
        assert_eq!(empty["sandbox"]["allow_partial"].as_bool(), Some(true));
        // A configuration that decides for itself keeps its decision.
        let own = "[sandbox]\nallow_partial = false\n";
        assert_eq!(with_accepted_partial_sandbox(dir.path(), own), own);
    }

    #[test]
    fn the_suite_is_hidden_from_the_agent_whatever_the_config_says() {
        let dir = tempfile::tempdir().unwrap();
        let task = dir.path().join("suite").join("one");
        std::fs::create_dir_all(&task).unwrap();
        let suite = task.parent().unwrap().canonicalize().unwrap();
        let hidden = |config: &str| -> Vec<String> {
            let table: toml::Table = with_suite_unreadable(config, &task).parse().unwrap();
            table["sandbox"]["unreadable"]
                .as_array()
                .unwrap()
                .iter()
                .map(|path| path.as_str().unwrap().to_string())
                .collect()
        };
        assert_eq!(hidden(""), vec![suite.to_string_lossy().to_string()]);
        let kept = hidden("[model]\nname = \"m\"\n[sandbox]\nunreadable = [\"/x\"]\n");
        assert_eq!(
            kept,
            vec!["/x".to_string(), suite.to_string_lossy().to_string()]
        );
    }

    #[test]
    fn changes_and_protected_paths_are_found_by_hash() {
        let map = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        let before = map(&[("a.py", "1"), ("tests/t.py", "2"), ("data/x.csv", "3")]);
        let after = map(&[("a.py", "9"), ("tests/t.py", "2"), ("new.txt", "4")]);
        assert_eq!(
            changes(&before, &after),
            map(&[("a.py", "9"), ("new.txt", "4"), ("data/x.csv", "deleted")])
        );
        assert_eq!(
            forbidden_changes(&["tests/".into(), "data/".into()], &before, &after),
            vec!["deleted data/x.csv".to_string()]
        );
        assert_eq!(
            forbidden_changes(&["a.py".into(), "b.py".into()], &before, &after),
            vec!["changed a.py".to_string()]
        );
    }
}
