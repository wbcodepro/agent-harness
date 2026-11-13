use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use clap::Args;
use hx_agent::{Agent, Session, ToolPipeline, ToolRequest};
use hx_eval::{Recording, ScriptedLlm};
use hx_kernel::{BoxFuture, Layer, Next};
use hx_llm::{ChatCompletions, Llm};
use hx_session::{Block, CreateOptions, Delta, Event, Record, SessionStore, TurnEndReason};
use hx_tools::{Effect, ToolContext, ToolOutput, ToolSet};

use crate::{cwd_of, display, parse_id};

const DEFAULT_BASE_URL: &str = "https://api.deepseek.com";
const DEFAULT_MODEL: &str = "deepseek-flash";

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
    /// Model name. Defaults to $HX_MODEL, then deepseek-flash.
    #[arg(long)]
    model: Option<String>,
    /// Root of an OpenAI-compatible API. Defaults to $HX_BASE_URL, then
    /// https://api.deepseek.com. The key is read from $HX_API_KEY, then
    /// $DEEPSEEK_API_KEY.
    #[arg(long)]
    base_url: Option<String>,
    /// Most model requests allowed in the turn.
    #[arg(long, default_value_t = 50)]
    max_steps: u64,
    /// Let the agent write files and run commands without asking.
    #[arg(long)]
    yes: bool,
}

pub async fn run(store: &SessionStore, args: RunArgs) -> Result<ExitCode, String> {
    let llm = llm(&args)?;

    let (records, writer) = match &args.session {
        Some(id) => store.open(&parse_id(id)?).map_err(display)?,
        None => {
            let cwd = match &args.cwd {
                Some(cwd) => cwd.clone(),
                None => std::env::current_dir().map_err(display)?,
            };
            let cwd = cwd
                .canonicalize()
                .map_err(|e| format!("cannot use {} as working directory: {e}", cwd.display()))?;
            let writer = store
                .create(CreateOptions {
                    id: None,
                    cwd: cwd.to_string_lossy().into_owned(),
                })
                .map_err(display)?;
            (store.read(writer.id()).map_err(display)?, writer)
        }
    };
    let cwd = cwd_of(&records);
    let mut session = Session::new(records, writer);
    eprintln!("session: {}", session.id());
    session.observe(printer());

    let agent = Agent {
        llm,
        tools: ToolSet::builtin(),
        pipeline: ToolPipeline::new(),
        tool_context: ToolContext {
            cwd: PathBuf::from(&cwd),
        },
        system: format!(
            "You are hx, a coding agent working in the user's terminal.\n\
             Working directory: {cwd}\n\
             Use the tools to inspect and change files and to run commands; do not guess at \
             file contents. Be concise. When the task is done, say what you did and what you \
             verified."
        ),
        max_steps: args.max_steps,
        max_tokens: None,
    };
    agent.pipeline.push(Approval {
        mode: if args.yes {
            ApprovalMode::Allow
        } else if std::io::stdin().is_terminal() {
            ApprovalMode::Ask
        } else {
            ApprovalMode::Deny
        },
    });

    let outcome = tokio::select! {
        result = agent.run_turn(&mut session, vec![Block::text(args.prompt)]) => Some(result),
        _ = tokio::signal::ctrl_c() => None,
    };
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

fn llm(args: &RunArgs) -> Result<Arc<dyn Llm>, String> {
    // Replays recorded responses instead of calling a provider; for tests
    // and evals.
    if let Some(script) = std::env::var_os("HX_SCRIPT") {
        let delay = std::env::var("HX_SCRIPT_DELAY_MS")
            .ok()
            .and_then(|ms| ms.parse().ok())
            .unwrap_or(0);
        let recording = Recording::load(&script).map_err(display)?;
        return Ok(Arc::new(
            ScriptedLlm::new(recording).with_delay(Duration::from_millis(delay)),
        ));
    }
    let env = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
    let api_key = env("HX_API_KEY")
        .or_else(|| env("DEEPSEEK_API_KEY"))
        .ok_or("no API key: set HX_API_KEY or DEEPSEEK_API_KEY")?;
    let base_url = args
        .base_url
        .clone()
        .or_else(|| env("HX_BASE_URL"))
        .unwrap_or_else(|| DEFAULT_BASE_URL.into());
    let model = args
        .model
        .clone()
        .or_else(|| env("HX_MODEL"))
        .unwrap_or_else(|| DEFAULT_MODEL.into());
    Ok(Arc::new(ChatCompletions::new(&base_url, api_key, model)))
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
        Event::AssistantMessage { .. } | Event::AssistantAttempt { .. } if line_open => {
            println!();
            line_open = false;
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

enum ApprovalMode {
    Allow,
    Ask,
    Deny,
}

/// Gates every tool that can change something. These tools run with the
/// user's full privileges, so nothing but a read happens unasked.
struct Approval {
    mode: ApprovalMode,
}

impl Layer<ToolRequest, ToolOutput> for Approval {
    fn call<'a>(
        &'a self,
        request: ToolRequest,
        next: Next<'a, ToolRequest, ToolOutput>,
    ) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            if request.effect == Effect::ReadOnly {
                return next.run(request).await;
            }
            match self.mode {
                ApprovalMode::Allow => next.run(request).await,
                ApprovalMode::Deny => ToolOutput::error(
                    "Denied: this tool needs the user's approval and nobody is at the terminal \
                     to give it. Do not retry; tell the user to rerun with --yes if they want \
                     this allowed.",
                ),
                ApprovalMode::Ask => {
                    let question = format!(
                        "Allow {} {}? [y/N] ",
                        request.name,
                        one_line(&describe(&request.name, &request.input))
                    );
                    let allowed = tokio::task::spawn_blocking(move || {
                        eprint!("{question}");
                        let mut answer = String::new();
                        std::io::stdin().read_line(&mut answer).is_ok()
                            && matches!(answer.trim(), "y" | "Y" | "yes")
                    })
                    .await
                    .unwrap_or(false);
                    if allowed {
                        next.run(request).await
                    } else {
                        ToolOutput::error("Denied by the user.")
                    }
                }
            }
        })
    }
}
