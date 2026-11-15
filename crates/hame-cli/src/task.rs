//! `hame task`: work on a task file until its acceptance conditions hold,
//! then verify them independently and report the evidence.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

use clap::Args;
use hame_agent::Session;
use hame_app::Options;
use hame_session::{Block, CreateOptions, Event, SessionStore, TurnEndReason};

use crate::display;
use crate::run::{Spent, printer};

#[derive(Args)]
pub struct TaskArgs {
    /// The task file (TOML): outcome, acceptance conditions, grants and
    /// budget. See `hame_verify::task_file`.
    file: PathBuf,
    /// Working directory. Defaults to the current one.
    #[arg(long)]
    cwd: Option<PathBuf>,
    /// deepseek, openai or anthropic.
    #[arg(long)]
    provider: Option<String>,
    #[arg(long)]
    model: Option<String>,
    #[arg(long)]
    base_url: Option<String>,
    /// Answer yes to every approval question.
    #[arg(long)]
    yes: bool,
    /// Write a trace of every model attempt and tool call here (`session`
    /// puts it next to the session log).
    #[arg(long)]
    trace: Option<PathBuf>,
    /// Print one JSON object with the outcome and evidence instead of text.
    #[arg(long)]
    json: bool,
}

pub async fn task(home: &Path, store: &SessionStore, args: TaskArgs) -> Result<ExitCode, String> {
    let contract = hame_verify::task_file::load(&args.file)?;
    let cwd = match &args.cwd {
        Some(cwd) => cwd.clone(),
        None => std::env::current_dir().map_err(display)?,
    };
    let cwd = hame_kernel::real_path(&cwd)
        .map_err(|e| format!("cannot use {} as working directory: {e}", cwd.display()))?;
    let tracer = args.trace.as_ref().map(|_| hame_trace::Tracer::new());
    let options = Options {
        provider: args.provider.clone(),
        model: args.model.clone(),
        base_url: args.base_url.clone(),
        yes: args.yes,
        max_steps: contract.budget.max_steps.unwrap_or(0),
        tracer: tracer.clone(),
        task: Some(contract.clone()),
        ..Options::default()
    };
    let mut runtime = hame_app::build(home, &cwd, &options).await?;
    let writer = store
        .create(CreateOptions {
            id: None,
            cwd: cwd.to_string_lossy().into_owned(),
        })
        .map_err(display)?;
    let records = store.read(writer.id()).map_err(display)?;
    let mut session = Session::new(records, writer);
    if let (Some(tracer), Some(path)) = (&tracer, &args.trace) {
        let path = if path.as_os_str() == "session" {
            store
                .root()
                .join(session.id().as_str())
                .join(hame_trace::TRACE_FILE)
        } else {
            path.clone()
        };
        if let Err(e) = tracer.attach_file(&path) {
            eprintln!("hame: cannot write the trace to {}: {e}", path.display());
        }
    }
    runtime.computer.probe.set_session(session.id().as_str());
    session
        .append(Event::TaskStart {
            contract: Box::new(contract.clone()),
        })
        .map_err(display)?;
    // First, so a spent budget stops the turn before anything else runs.
    runtime.agent.hooks.insert(
        0,
        Arc::new(
            hame_verify::BudgetGuard::new(contract.budget.clone(), &session)
                .with_model(runtime.model.clone()),
        ),
    );
    if !args.json {
        eprintln!("session: {}", session.id());
        eprintln!("task: {}", contract.id);
        session.observe(printer());
    }
    let logged_before = session.records().len();
    let started = Instant::now();
    let turn_span = tracer
        .as_ref()
        .map(|t| t.span(hame_trace::SpanKind::Turn, "task"));
    let prompt = hame_verify::task_file::prompt(&contract);
    let outcome = tokio::select! {
        result = runtime.agent.run_turn(&mut session, vec![Block::text(prompt)]) => Some(result),
        _ = tokio::signal::ctrl_c() => None,
    };
    if let Some(span) = turn_span {
        span.end(match &outcome {
            Some(Ok(())) => hame_trace::SpanStatus::Ok,
            _ => hame_trace::SpanStatus::Error,
        });
    }
    let agent_ms = started.elapsed().as_millis() as u64;
    let (status, error) = match &outcome {
        Some(Ok(())) => ("completed", None),
        Some(Err(e)) => ("error", Some(e.to_string())),
        None => {
            session
                .close_open_turn(TurnEndReason::Cancelled)
                .map_err(display)?;
            ("cancelled", None)
        }
    };

    // The verdict comes from the state, not from the agent's last message.
    let verify_span = tracer
        .as_ref()
        .map(|t| t.span(hame_trace::SpanKind::Verification, "acceptance"));
    let probe: Arc<dyn hame_verify::StateProbe> = runtime.computer.probe.clone();
    let report = hame_verify::verify(
        &contract,
        &cwd,
        Some(probe.as_ref()),
        runtime.confine.as_ref(),
    )
    .await;
    if let Some(span) = verify_span {
        span.end(if report.passed {
            hame_trace::SpanStatus::Ok
        } else {
            hame_trace::SpanStatus::Error
        });
    }
    session
        .append(Event::TaskVerified {
            report: Box::new(report.clone()),
        })
        .map_err(display)?;
    runtime.computer.shutdown().await;
    let total_ms = started.elapsed().as_millis() as u64;

    let new = &session.records()[logged_before..];
    let spent = Spent::of(new);
    let receipts: Vec<_> = new
        .iter()
        .filter_map(|r| match &r.event {
            Event::ActionReceipt { receipt } => Some(receipt.as_ref().clone()),
            _ => None,
        })
        .collect();
    if args.json {
        println!(
            "{}",
            serde_json::json!({
                "type": "task",
                "task": contract.id,
                "session": session.id().to_string(),
                "status": status,
                "error": error,
                "verified": report.passed,
                "verification": report,
                "agent_ms": agent_ms,
                "verification_ms": report.ms,
                "total_ms": total_ms,
                "requests": spent.requests,
                "input_tokens": spent.input,
                "cached_tokens": spent.cached,
                "output_tokens": spent.output,
                "actions": receipts,
            })
        );
    } else {
        if let Some(error) = &error {
            eprintln!("hame: the turn ended with an error: {error}");
        }
        if spent.requests > 0 {
            eprintln!("{spent}");
        }
        if !receipts.is_empty() {
            eprintln!("actions on applications: {}", receipts.len());
            for receipt in &receipts {
                eprintln!(
                    "  {:?} {} {}",
                    receipt.status,
                    receipt.action,
                    receipt.target.label()
                );
            }
        }
        eprintln!("{}", hame_verify::describe(&report));
        eprintln!("time: {agent_ms} ms working, {} ms verifying", report.ms);
    }
    Ok(match (status, report.passed) {
        (_, true) => ExitCode::SUCCESS,
        ("cancelled", false) => ExitCode::from(130),
        _ => ExitCode::from(1),
    })
}
