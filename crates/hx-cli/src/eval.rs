//! `hx eval`: run a suite of tasks and report how the agent did.
//!
//! A suite is a directory of tasks. Each task is a directory holding a
//! `task.toml` (the prompt and a shell command that decides pass or fail)
//! and an optional `files/` tree copied into a fresh workspace. Every task
//! runs through the real `hx run`, so what is measured is the product.

use std::path::{Path, PathBuf};
use std::process::{ExitCode, Stdio};
use std::time::{Duration, Instant};

use clap::Args;
use hx_session::{Event, SessionStore};
use serde::Deserialize;
use serde_json::json;

#[derive(Args)]
pub struct EvalArgs {
    /// Directory of tasks.
    suite: PathBuf,
    #[arg(long)]
    provider: Option<String>,
    #[arg(long)]
    model: Option<String>,
    /// Only run tasks whose name contains this.
    #[arg(long)]
    filter: Option<String>,
    /// Seconds allowed per task.
    #[arg(long, default_value_t = 600)]
    timeout: u64,
    /// Also write the results as JSON to this file.
    #[arg(long)]
    json: Option<PathBuf>,
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

async fn shell(command: &str, cwd: &Path) -> bool {
    tokio::process::Command::new("bash")
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .is_ok_and(|status| status.success())
}

async fn run_task(
    name: &str,
    dir: &Path,
    scratch: &Path,
    args: &EvalArgs,
    keys: &[(String, String)],
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
    if dir.join("files").is_dir() {
        copy_tree(&dir.join("files"), &work).map_err(|e| e.to_string())?;
    }
    if let Some(setup) = &task.setup
        && !shell(setup, &work).await
    {
        return Err(format!("{name}: setup failed"));
    }

    let mut command =
        tokio::process::Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
    command
        .arg("--home")
        .arg(&home)
        .args(["run", "--yes"])
        .current_dir(&work)
        .envs(keys.iter().cloned())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(provider) = &args.provider {
        command.args(["--provider", provider]);
    }
    if let Some(model) = &args.model {
        command.args(["--model", model]);
    }
    command.arg(&task.prompt);

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
            .find(|line| line.starts_with("hx:"))
            .unwrap_or("hx failed")
            .to_string(),
        Ok(Ok(_)) => String::new(),
    };

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
                _ => {}
            }
        }
    }
    Ok(outcome)
}

pub async fn run(home: &Path, args: EvalArgs) -> Result<ExitCode, String> {
    let mut tasks: Vec<(String, PathBuf)> = std::fs::read_dir(&args.suite)
        .map_err(|e| format!("cannot read {}: {e}", args.suite.display()))?
        .flatten()
        .filter(|entry| entry.path().join("task.toml").is_file())
        .map(|entry| {
            (
                entry.file_name().to_string_lossy().into_owned(),
                entry.path(),
            )
        })
        .filter(|(name, _)| {
            args.filter
                .as_ref()
                .is_none_or(|f| name.contains(f.as_str()))
        })
        .collect();
    tasks.sort();
    if tasks.is_empty() {
        return Err(format!("no tasks found in {}", args.suite.display()));
    }

    // Each task gets an empty data directory, so the user's credentials are
    // handed over through the environment instead.
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let secrets = hx_app::load_secrets(home, &cwd)?;
    let keys: Vec<(String, String)> = secrets
        .names()
        .filter_map(|name| Some((name.to_string(), secrets.get(name)?.to_string())))
        .collect();

    let scratch = std::env::temp_dir().join(format!("hx-eval-{}", std::process::id()));
    let mut outcomes = Vec::new();
    for (name, dir) in &tasks {
        eprint!("{name} ... ");
        let outcome = run_task(name, dir, &scratch, &args, &keys).await?;
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

    if let Some(path) = &args.json {
        let report = json!({
            "provider": args.provider,
            "model": args.model,
            "passed": passed,
            "total": outcomes.len(),
            "tasks": outcomes.iter().map(|o| json!({
                "name": o.name, "passed": o.passed, "note": o.note, "seconds": o.seconds,
                "requests": o.requests, "tool_calls": o.tool_calls,
                "input_tokens": o.input_tokens, "cached_tokens": o.cached_tokens,
                "output_tokens": o.output_tokens,
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
