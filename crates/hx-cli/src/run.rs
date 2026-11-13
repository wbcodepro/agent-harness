use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, ValueEnum};
use hx_agent::Session;
use hx_app::{NetworkChoice as NetworkMode, Options, SandboxMode as Mode};
use hx_session::{Block, CreateOptions, Delta, Event, Record, SessionStore, TurnEndReason};

use crate::{cwd_of, display, parse_id};

#[derive(Clone, Copy, ValueEnum)]
pub enum SandboxArg {
    ReadOnly,
    WorkspaceWrite,
    DangerFullAccess,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum NetworkArg {
    Off,
    Allowlist,
    Open,
}

#[derive(Args)]
pub struct RunArgs {
    /// What to ask the agent.
    prompt: String,
    /// Continue this session instead of starting a new one.
    #[arg(long)]
    session: Option<String>,
    /// Working directory for a new session. Defaults to the current one.
    #[arg(long)]
    cwd: Option<PathBuf>,
    /// deepseek, openai or anthropic. Defaults to $HX_PROVIDER, then the
    /// configuration, then whichever provider a key is set for.
    #[arg(long)]
    provider: Option<String>,
    /// Model name. Defaults to $HX_MODEL, then the configuration, then the
    /// provider's default.
    #[arg(long)]
    model: Option<String>,
    /// Root of the provider's API. Defaults to $HX_BASE_URL, then the
    /// configuration, then the provider's own.
    #[arg(long)]
    base_url: Option<String>,
    /// Most model requests allowed in the turn.
    #[arg(long, default_value_t = 50)]
    max_steps: u64,
    /// How tools are confined. Defaults to the configuration
    /// (workspace-write unless changed).
    #[arg(long, value_enum)]
    sandbox: Option<SandboxArg>,
    /// What confined tools may reach on the network.
    #[arg(long, value_enum)]
    network: Option<NetworkArg>,
    /// Answer yes to every approval question.
    #[arg(long)]
    yes: bool,
    /// Plan mode: investigate and propose a plan without changing anything.
    #[arg(long)]
    plan: bool,
}

pub async fn run(home: &Path, store: &SessionStore, args: RunArgs) -> Result<ExitCode, String> {
    let options = Options {
        sandbox: args.sandbox.map(|mode| match mode {
            SandboxArg::ReadOnly => Mode::ReadOnly,
            SandboxArg::WorkspaceWrite => Mode::WorkspaceWrite,
            SandboxArg::DangerFullAccess => Mode::DangerFullAccess,
        }),
        network: args.network.map(|mode| match mode {
            NetworkArg::Off => NetworkMode::Off,
            NetworkArg::Allowlist => NetworkMode::Allowlist,
            NetworkArg::Open => NetworkMode::Open,
        }),
        yes: args.yes,
        provider: args.provider.clone(),
        model: args.model.clone(),
        base_url: args.base_url.clone(),
        max_steps: args.max_steps,
        plan: args.plan,
        ..Options::default()
    };

    // Resolve the working directory first: a new session uses the flag or
    // the current directory, an existing one keeps its own.
    let existing = match &args.session {
        Some(id) => Some(parse_id(id)?),
        None => None,
    };
    let cwd = match &existing {
        Some(id) => PathBuf::from(cwd_of(&store.read(id).map_err(display)?)),
        None => {
            let cwd = match &args.cwd {
                Some(cwd) => cwd.clone(),
                None => std::env::current_dir().map_err(display)?,
            };
            cwd.canonicalize()
                .map_err(|e| format!("cannot use {} as working directory: {e}", cwd.display()))?
        }
    };

    // Build the agent before touching the store, so a configuration problem
    // does not leave an empty session behind.
    let runtime = hx_app::build(home, &cwd, &options).await?;

    let (records, writer) = match &existing {
        Some(id) => store.open(id).map_err(display)?,
        None => {
            let writer = store
                .create(CreateOptions {
                    id: None,
                    cwd: cwd.to_string_lossy().into_owned(),
                })
                .map_err(display)?;
            (store.read(writer.id()).map_err(display)?, writer)
        }
    };
    let mut session = Session::new(records, writer);
    eprintln!("session: {}", session.id());
    session.observe(printer());

    let outcome = tokio::select! {
        result = runtime.agent.run_turn(&mut session, vec![Block::text(args.prompt)]) => Some(result),
        _ = tokio::signal::ctrl_c() => None,
    };
    if let Some(proxy) = &runtime.proxy {
        for decision in proxy.decisions().iter().filter(|d| !d.allowed) {
            eprintln!("network: blocked {}:{}", decision.host, decision.port);
        }
    }
    match outcome {
        Some(Ok(())) => Ok(ExitCode::SUCCESS),
        Some(Err(e)) => Err(e.to_string()),
        None => {
            // The turn's future is gone; settle what it left open.
            session
                .close_open_turn(TurnEndReason::Cancelled)
                .map_err(display)?;
            eprintln!("\nhx: cancelled");
            Ok(ExitCode::from(130))
        }
    }
}

/// Streams the answer to stdout and a one-line trace of tool activity to
/// stderr.
fn printer() -> impl FnMut(&Record) + Send + 'static {
    let mut line_open = false;
    move |record| match &record.event {
        Event::AssistantDelta {
            delta: Delta::Text { text },
        } => {
            print!("{text}");
            let _ = std::io::stdout().flush();
            line_open = !text.ends_with('\n');
        }
        Event::AssistantMessage { stop, .. } => {
            if line_open {
                println!();
                line_open = false;
            }
            if *stop == hx_session::StopReason::Refusal {
                eprintln!("hx: the model declined to answer this request");
            }
        }
        Event::AssistantAttempt { .. } if line_open => {
            println!();
            line_open = false;
        }
        Event::Compaction { .. } => eprintln!("context: earlier history summarized"),
        Event::UserMessage { content } if matches!(content.first(), Some(Block::Text { text }) if text.starts_with("[hx verify]")) =>
        {
            eprintln!("verify: checks failed; sent back to the model");
        }
        Event::ToolCall { name, input, .. } => {
            eprintln!("> {name} {}", one_line(&describe(name, input)));
        }
        Event::ToolResult {
            content, is_error, ..
        } => {
            let text = match content.first() {
                Some(Block::Text { text }) => text.as_str(),
                _ => "",
            };
            let status = if *is_error { "error" } else { "ok" };
            eprintln!("< {status}: {}", one_line(text));
        }
        _ => {}
    }
}

fn describe(name: &str, input: &serde_json::Value) -> String {
    match (name, input["command"].as_str()) {
        ("bash", Some(command)) => command.to_string(),
        _ => input.to_string(),
    }
}

fn one_line(text: &str) -> String {
    let first = text.lines().next().unwrap_or("");
    let mut shown: String = first.chars().take(160).collect();
    if shown.len() < text.trim_end().len() {
        shown.push_str(" ...");
    }
    shown
}
