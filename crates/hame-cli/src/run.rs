use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, ValueEnum};
use hame_agent::Session;
use hame_app::{NetworkChoice as NetworkMode, Options, SandboxMode as Mode};
use hame_session::{Block, CreateOptions, Delta, Event, Record, SessionStore, TurnEndReason};

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

#[derive(Clone, Copy, Default, PartialEq, ValueEnum)]
pub enum OutputArg {
    /// The answer on stdout, a trace of tool activity on stderr.
    #[default]
    Text,
    /// One JSON object on stdout when the turn is over.
    Json,
    /// Every event as a JSON line on stdout as it is logged, then the
    /// same final object.
    StreamJson,
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
    /// deepseek, openai or anthropic. Defaults to $HAME_PROVIDER, then the
    /// configuration, then whichever provider a key is set for.
    #[arg(long)]
    provider: Option<String>,
    /// Model name. Defaults to $HAME_MODEL, then the configuration, then the
    /// provider's default.
    #[arg(long)]
    model: Option<String>,
    /// Root of the provider's API. Defaults to $HAME_BASE_URL, then the
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
    /// What to print: text for people, json or stream-json for programs.
    #[arg(long, value_enum, default_value_t)]
    output: OutputArg,
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
            hame_kernel::real_path(&cwd)
                .map_err(|e| format!("cannot use {} as working directory: {e}", cwd.display()))?
        }
    };

    // Build the agent before touching the store, so a configuration problem
    // does not leave an empty session behind.
    let runtime = hame_app::build(home, &cwd, &options).await?;

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
    match args.output {
        OutputArg::Text => {
            eprintln!("session: {}", session.id());
            session.observe(printer());
        }
        OutputArg::Json => {}
        OutputArg::StreamJson => session.observe(|record: &Record| {
            // Partial text is for people watching; a program reads the
            // whole message that follows.
            if !matches!(record.event, Event::AssistantDelta { .. })
                && let Ok(line) = serde_json::to_string(record)
            {
                println!("{line}");
            }
        }),
    }
    let logged_before = session.records().len();

    let outcome = tokio::select! {
        result = runtime.agent.run_turn(&mut session, vec![Block::text(hame_app::expand_command(home, &cwd, &args.prompt))]) => Some(result),
        _ = tokio::signal::ctrl_c() => None,
    };
    if let Some(proxy) = &runtime.proxy {
        for decision in proxy.decisions().iter().filter(|d| !d.allowed) {
            eprintln!("network: blocked {}:{}", decision.host, decision.port);
        }
    }
    let spent = Spent::of(&session.records()[logged_before..]);
    let proof = hame_verify::evidence(session.records());
    let sources = hame_context::context_sources(session.records());
    if args.output != OutputArg::Text {
        let (status, error, code) = match &outcome {
            Some(Ok(())) => ("completed", None, ExitCode::SUCCESS),
            Some(Err(e)) => ("error", Some(e.to_string()), ExitCode::FAILURE),
            None => {
                session
                    .close_open_turn(TurnEndReason::Cancelled)
                    .map_err(display)?;
                ("cancelled", None, ExitCode::from(130))
            }
        };
        let new = &session.records()[logged_before..];
        let result = serde_json::json!({
            "type": "result",
            "session": session.id().to_string(),
            "status": status,
            "error": error,
            "text": final_text(new),
            "requests": spent.requests,
            "tool_calls": new.iter().filter(|r| matches!(r.event, Event::ToolCall { .. })).count(),
            "input_tokens": spent.input,
            "cached_tokens": spent.cached,
            "output_tokens": spent.output,
            // Ending is not the same as being right: what was run after
            // the last change, and how it came out.
            // What the next request would carry, by where it comes from
            // (estimated tokens).
            "context": sources.iter().map(|(source, tokens)| (source.clone(), serde_json::Value::from(*tokens))).collect::<serde_json::Map<String, serde_json::Value>>(),
            "evidence": proof.status(),
            "evidence_command": proof.command(),
        });
        println!("{result}");
        return Ok(code);
    }
    if spent.requests > 0 {
        eprintln!("{spent}");
        // The four largest parts of what every further request carries.
        let mut largest = sources.clone();
        largest.sort_by_key(|(_, tokens)| std::cmp::Reverse(*tokens));
        let parts: Vec<String> = largest
            .iter()
            .filter(|(_, tokens)| *tokens > 0)
            .take(4)
            .map(|(source, tokens)| format!("{source} ~{tokens}"))
            .collect();
        if !parts.is_empty() {
            eprintln!("context now (est. tokens): {}", parts.join(" · "));
        }
        match &proof {
            hame_verify::Evidence::NoChange => {}
            hame_verify::Evidence::Unchecked => {
                eprintln!("evidence: none; code was changed and nothing was run afterwards");
            }
            hame_verify::Evidence::Failing(command) => {
                eprintln!("evidence: FAILING; the last check after the changes failed: {command}");
            }
            hame_verify::Evidence::Passed(command) => {
                eprintln!("evidence: passed after the last change: {command}");
            }
        }
        if spent.cache_is_cold() {
            eprintln!(
                "cache: little of the prompt was reused between requests; with this provider \
                 each request is paying full price for the whole history"
            );
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
            eprintln!("\nhame: cancelled");
            Ok(ExitCode::from(130))
        }
    }
}

/// The text of the last answer in a stretch of log.
fn final_text(records: &[Record]) -> String {
    records
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
        .unwrap_or_default()
}

/// What the model requests in a stretch of log cost.
#[derive(Debug, Default, PartialEq)]
struct Spent {
    requests: u64,
    /// Everything sent, whether read from the provider's cache or not.
    input: u64,
    cached: u64,
    output: u64,
}

impl Spent {
    fn of(records: &[Record]) -> Self {
        let mut spent = Self::default();
        for record in records {
            if let Event::AssistantMessage { usage, .. } | Event::Compaction { usage, .. } =
                &record.event
            {
                spent.requests += 1;
                spent.input +=
                    usage.input_tokens + usage.cache_read_tokens + usage.cache_write_tokens;
                spent.cached += usage.cache_read_tokens;
                spent.output += usage.output_tokens;
            }
        }
        spent
    }

    fn cached_percent(&self) -> u64 {
        (self.cached * 100).checked_div(self.input).unwrap_or(0)
    }

    /// From the third request on, most of each prompt is history the
    /// provider has already seen. If under a third of all input came from
    /// its cache, something keeps changing the start of the prompt (or the
    /// provider does not cache).
    fn cache_is_cold(&self) -> bool {
        self.requests >= 4 && self.input >= 20_000 && self.cached_percent() < 33
    }
}

impl std::fmt::Display for Spent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "tokens: {} in ({}% from cache), {} out, {} requests",
            self.input,
            self.cached_percent(),
            self.output,
            self.requests
        )
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
            if *stop == hame_session::StopReason::Refusal {
                eprintln!("hame: the model declined to answer this request");
            }
        }
        Event::AssistantAttempt { .. } if line_open => {
            println!();
            line_open = false;
        }
        Event::Compaction { .. } => eprintln!("context: earlier history summarized"),
        Event::Masked { .. } => eprintln!("context: old tool output cleared"),
        Event::ModelSwitched { model } => eprintln!("model: now {model}"),
        Event::UserMessage { content } if matches!(content.first(), Some(Block::Text { text }) if text.starts_with("[hame verify]")) =>
        {
            eprintln!("verify: checks failed; sent back to the model");
        }
        Event::UserMessage { content } if matches!(content.first(), Some(Block::Text { text }) if text.starts_with("[hame checklist]")) =>
        {
            eprintln!("verify: asked the model to check its work against the request");
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

#[cfg(test)]
mod tests {
    use super::*;
    use hame_session::{StopReason, Usage};

    fn response(input: u64, cached: u64, output: u64) -> Record {
        Record {
            seq: 0,
            ts: 0,
            event: Event::AssistantMessage {
                content: Vec::new(),
                stop: StopReason::EndTurn,
                usage: Usage {
                    input_tokens: input,
                    output_tokens: output,
                    cache_read_tokens: cached,
                    cache_write_tokens: 0,
                },
            },
        }
    }

    #[test]
    fn a_run_that_keeps_resending_uncached_history_is_called_out() {
        let warm: Vec<Record> = (1..=5).map(|n| response(500, n * 8000, 100)).collect();
        let spent = Spent::of(&warm);
        assert_eq!(spent.requests, 5);
        assert_eq!(spent.input, 122_500);
        assert_eq!(spent.cached_percent(), 97);
        assert!(!spent.cache_is_cold());
        assert_eq!(
            spent.to_string(),
            "tokens: 122500 in (97% from cache), 500 out, 5 requests"
        );

        let cold: Vec<Record> = (1..=5).map(|n| response(n * 8000, 0, 100)).collect();
        assert!(Spent::of(&cold).cache_is_cold());

        // Too short a run to judge.
        assert!(!Spent::of(&cold[..2]).cache_is_cold());
        assert_eq!(Spent::of(&[]).cached_percent(), 0);
    }
}
