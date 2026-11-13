//! `zcode eval`: run a suite of tasks and report how the agent did.
//!
//! A suite is a directory of tasks. Each task is a directory holding a
//! `task.toml` (the prompt and a shell command that decides pass or fail)
//! and an optional `files/` tree copied into a fresh workspace. An optional
//! `check/` tree is copied over the workspace before the check runs, so an
//! agent cannot pass by editing the tests. Every task runs through the real
//! `zcode run`, so what is measured is the product; `--command` runs another
//! agent on the same tasks for comparison.

use std::path::{Path, PathBuf};
use std::process::{ExitCode, Stdio};
use std::time::{Duration, Instant};

use clap::Args;
use serde::Deserialize;
use serde_json::json;
use zcode_session::{Event, SessionStore};

#[derive(Args)]
pub struct EvalArgs {
    /// Directory of tasks.
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
    /// Run this shell command in each workspace instead of zcode, to measure
    /// another agent on the same tasks. The prompt is in $ZCODE_EVAL_PROMPT.
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
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Task {
    prompt: String,
    /// Shell command run in the workspace afterwards; exit 0 means pass.
    check: String,
    /// Shell command run in the workspace before the agent starts.
    setup: Option<String>,
}

struct Outcome {
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

/// Runs a task's setup or check command. One that does not finish in five
/// minutes counts as failed: a check that runs the agent's code can hang on
/// it.
async fn shell(command: &str, cwd: &Path) -> bool {
    let child = tokio::process::Command::new(zcode_kernel::bash_program())
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn();
    let Ok(mut child) = child else {
        return false;
    };
    matches!(
        tokio::time::timeout(Duration::from_secs(300), child.wait()).await,
        Ok(Ok(status)) if status.success()
    )
}

async fn run_task(
    name: &str,
    dir: &Path,
    scratch: &Path,
    args: &EvalArgs,
    keys: &[(String, String)],
    config: &str,
) -> Result<Outcome, String> {
    let text = std::fs::read_to_string(dir.join("task.toml"))
        .map_err(|e| format!("{name}: cannot read task.toml: {e}"))?;
    let task: Task = toml::from_str(&text).map_err(|e| format!("{name}: {e}"))?;

    let (home, work) = (
        scratch.join(name).join("home"),
        scratch.join(name).join("work"),
    );
    std::fs::create_dir_all(&home).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    if !config.is_empty() {
        std::fs::write(home.join("config.toml"), config).map_err(|e| e.to_string())?;
    }
    if dir.join("files").is_dir() {
        copy_tree(&dir.join("files"), &work).map_err(|e| e.to_string())?;
    }
    if let Some(setup) = &task.setup
        && !shell(setup, &work).await
    {
        return Err(format!("{name}: setup failed"));
    }

    let mut command = match &args.command {
        Some(external) => {
            let mut command = tokio::process::Command::new(zcode_kernel::bash_program());
            command
                .arg("-c")
                .arg(external)
                .env("ZCODE_EVAL_PROMPT", &task.prompt);
            command
        }
        None => {
            let mut command =
                tokio::process::Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
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
            command.arg(&task.prompt);
            command
        }
    };
    command
        .current_dir(&work)
        .envs(keys.iter().cloned())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let started = Instant::now();
    let child = command.spawn().map_err(|e| e.to_string())?;
    let finished =
        tokio::time::timeout(Duration::from_secs(args.timeout), child.wait_with_output()).await;
    let seconds = started.elapsed().as_secs_f64();
    let mut note = match &finished {
        Err(_) => "timed out".to_string(),
        Ok(Err(e)) => format!("could not run: {e}"),
        Ok(Ok(output)) if !output.status.success() => String::from_utf8_lossy(&output.stderr)
            .lines()
            .rev()
            .find(|line| line.starts_with("zcode:"))
            .unwrap_or("the agent exited with an error")
            .to_string(),
        Ok(Ok(_)) => String::new(),
    };

    if dir.join("check").is_dir() {
        copy_tree(&dir.join("check"), &work).map_err(|e| e.to_string())?;
    }
    let passed = shell(&task.check, &work).await;
    if !passed && note.is_empty() {
        note = "check failed".into();
    }

    let mut outcome = Outcome {
        name: name.to_string(),
        passed,
        note,
        seconds,
        requests: 0,
        tool_calls: 0,
        input_tokens: 0,
        cached_tokens: 0,
        output_tokens: 0,
        escalated: false,
    };
    let store = SessionStore::new(home.join("sessions"));
    for header in store.list().unwrap_or_default() {
        for record in store.read(&header.id).unwrap_or_default() {
            match record.event {
                Event::AssistantMessage { usage, .. } => {
                    outcome.requests += 1;
                    outcome.input_tokens += usage.input_tokens + usage.cache_write_tokens;
                    outcome.cached_tokens += usage.cache_read_tokens;
                    outcome.output_tokens += usage.output_tokens;
                }
                Event::ToolCall { .. } => outcome.tool_calls += 1,
                Event::ModelSwitched { .. } => outcome.escalated = true,
                _ => {}
            }
        }
    }
    Ok(outcome)
}

/// The tasks of a suite, by name.
fn tasks_in(suite: &Path, filter: Option<&str>) -> Result<Vec<(String, PathBuf)>, String> {
    let mut tasks: Vec<(String, PathBuf)> = std::fs::read_dir(suite)
        .map_err(|e| format!("cannot read {}: {e}", suite.display()))?
        .flatten()
        .filter(|entry| entry.path().join("task.toml").is_file())
        .map(|entry| {
            (
                entry.file_name().to_string_lossy().into_owned(),
                entry.path(),
            )
        })
        .filter(|(name, _)| filter.is_none_or(|f| name.contains(f)))
        .collect();
    tasks.sort();
    if tasks.is_empty() {
        return Err(format!("no tasks found in {}", suite.display()));
    }
    Ok(tasks)
}

/// Runs every task once with `config` as the agent's configuration file.
async fn run_suite(
    home: &Path,
    args: &EvalArgs,
    tasks: &[(String, PathBuf)],
    config: &str,
    keep: Option<&Path>,
) -> Result<Vec<Outcome>, String> {
    // Each task gets an empty data directory, so the user's credentials are
    // handed over through the environment instead.
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let secrets = zcode_app::load_secrets(home, &cwd)?;
    let keys: Vec<(String, String)> = secrets
        .names()
        .filter_map(|name| Some((name.to_string(), secrets.get(name)?.to_string())))
        .collect();

    let scratch = std::env::temp_dir().join(format!("zcode-eval-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let mut outcomes = Vec::new();
    for (name, dir) in tasks {
        eprint!("{name} ... ");
        let outcome = run_task(name, dir, &scratch, args, &keys, config).await?;
        if let (false, Some(keep)) = (outcome.passed, keep) {
            let kept = keep.join(name);
            let _ = std::fs::remove_dir_all(&kept);
            if let Err(e) = copy_tree(&scratch.join(name), &kept) {
                eprintln!("zcode: could not keep {name}: {e}");
            }
        }
        eprintln!(
            "{} ({:.0}s){}",
            if outcome.passed { "pass" } else { "FAIL" },
            outcome.seconds,
            if outcome.note.is_empty() {
                String::new()
            } else {
                format!(" — {}", outcome.note)
            }
        );
        outcomes.push(outcome);
    }
    let _ = std::fs::remove_dir_all(&scratch);
    Ok(outcomes)
}

pub async fn run(home: &Path, args: EvalArgs) -> Result<ExitCode, String> {
    let tasks = tasks_in(&args.suite, args.filter.as_deref())?;
    let config = match &args.config {
        Some(path) => std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?,
        None => String::new(),
    };
    let runs = args.runs.max(1);
    let mut outcomes = Vec::new();
    for round in 1..=runs {
        // Each round keeps its failures apart, so none overwrites another.
        let keep = match (&args.keep, runs) {
            (Some(keep), 1) => Some(keep.clone()),
            (Some(keep), _) => Some(keep.join(format!("run-{round}"))),
            (None, _) => None,
        };
        if runs > 1 {
            eprintln!("== run {round} of {runs}");
        }
        outcomes.extend(run_suite(home, &args, &tasks, &config, keep.as_deref()).await?);
    }

    println!(
        "{:<24} {:>6} {:>7} {:>5} {:>6} {:>9} {:>9} {:>8}",
        "task", "result", "seconds", "reqs", "tools", "input", "cached", "output"
    );
    for o in &outcomes {
        println!(
            "{:<24} {:>6} {:>7.1} {:>5} {:>6} {:>9} {:>9} {:>8}",
            o.name,
            if o.passed { "pass" } else { "FAIL" },
            o.seconds,
            o.requests,
            o.tool_calls,
            o.input_tokens,
            o.cached_tokens,
            o.output_tokens
        );
    }
    if runs > 1 {
        println!("\n{:<24} {:>8}", "task", "passed");
        for (name, _) in &tasks {
            let of_task = || outcomes.iter().filter(|o| &o.name == name);
            println!(
                "{name:<24} {:>5}/{runs}",
                of_task().filter(|o| o.passed).count()
            );
        }
    }
    let passed = outcomes.iter().filter(|o| o.passed).count();
    let sum = |f: fn(&Outcome) -> u64| outcomes.iter().map(f).sum::<u64>();
    let (input, cached) = (sum(|o| o.input_tokens), sum(|o| o.cached_tokens));
    println!(
        "\n{passed}/{} passed · {:.0}s · {} input + {} cached ({:.0}% of prompt tokens from cache) · {} output",
        outcomes.len(),
        outcomes.iter().map(|o| o.seconds).sum::<f64>(),
        input,
        cached,
        if input + cached > 0 {
            100.0 * cached as f64 / (input + cached) as f64
        } else {
            0.0
        },
        sum(|o| o.output_tokens),
    );

    let escalated = outcomes.iter().filter(|o| o.escalated).count();
    if escalated > 0 {
        println!(
            "{escalated} of {} task runs were handed to the stronger model ({} of them passed)",
            outcomes.len(),
            outcomes.iter().filter(|o| o.escalated && o.passed).count()
        );
    }

    if let Some(path) = &args.json {
        let report = json!({
            "provider": args.provider,
            "model": args.model,
            "runs": runs,
            "passed": passed,
            "total": outcomes.len(),
            "tasks": outcomes.iter().map(|o| json!({
                "name": o.name, "passed": o.passed, "note": o.note, "seconds": o.seconds,
                "requests": o.requests, "tool_calls": o.tool_calls,
                "input_tokens": o.input_tokens, "cached_tokens": o.cached_tokens,
                "output_tokens": o.output_tokens, "escalated": o.escalated,
            })).collect::<Vec<_>>(),
        });
        std::fs::write(path, format!("{report:#}\n"))
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    }
    Ok(if passed == outcomes.len() {
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
}

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

/// `zcode probe`: find out how a model works best.
///
/// Which edit format a model handles, and whether it copes with the full
/// tool list, differs from model to model and cannot be told from its name.
/// This runs a suite once per setting and prints the configuration that
/// solved the most, at the lowest cost.
pub async fn probe(home: &Path, args: ProbeArgs) -> Result<ExitCode, String> {
    let tasks = tasks_in(&args.suite, args.filter.as_deref())?;
    let eval = EvalArgs {
        suite: args.suite.clone(),
        provider: args.provider.clone(),
        model: args.model.clone(),
        base_url: args.base_url.clone(),
        filter: args.filter.clone(),
        timeout: args.timeout,
        command: None,
        json: None,
        keep: None,
        config: None,
        runs: 1,
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
    // the short tool list: four runs instead of six.
    for edit_format in ["search-replace", "whole-file", "apply-patch"] {
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
        "\nBest on this suite. Put it in ~/.zcode/config.toml:\n\n[model]\nedit_format = \"{}\"\ntools = \"{}\"",
        winner.edit_format, winner.tools
    );
    if trials.iter().all(|trial| trial.passed == winner.passed) {
        println!(
            "\nEvery setting solved the same number of tasks; the choice above only saves \
             tokens. A harder suite would tell them apart."
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
        let tied = [
            trial("search-replace", 7, 3000),
            trial("apply-patch", 7, 3000),
        ];
        assert_eq!(best(&tied).edit_format, "search-replace");
    }
}
