mod run;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use hx_session::{Block, Event, Message, Record, Role, SessionId, SessionStore, derive_history};

#[derive(Parser)]
#[command(name = "hx", version, about = "The hx agent harness")]
struct Cli {
    /// Data directory. Defaults to $HX_HOME, then ~/.hx.
    #[arg(long, global = true)]
    home: Option<PathBuf>,
    /// With no command, opens the interactive interface.
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run one turn: send a prompt and let the agent work until it answers.
    Run(run::RunArgs),
    /// Open the interactive interface (the default).
    Tui {
        /// Continue this session instead of starting a new one.
        #[arg(long)]
        session: Option<String>,
        /// Start in plan mode.
        #[arg(long)]
        plan: bool,
    },
    /// Serve the Agent Client Protocol on stdin/stdout, for editors.
    Acp,
    /// List sessions.
    Sessions,
    /// Import a session recorded by another harness.
    Import {
        /// Where the session comes from. Supported: dsh (DeepSeek Harness).
        format: String,
        /// The session log file.
        file: PathBuf,
        /// Working directory for the imported session. Defaults to the one
        /// it was recorded in.
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    /// Undo a turn: restore the workspace to how it was when the turn began
    /// and continue in a new session that ends just before it.
    Rewind {
        id: String,
        /// Turn to undo, together with everything after it. Defaults to the
        /// last turn.
        #[arg(long)]
        turn: Option<u64>,
    },
    /// Runs one tool call inside the sandbox. Internal.
    #[command(hide = true)]
    ToolWorker,
    /// Applies a sandbox policy to itself, then executes a program. Internal.
    #[command(hide = true)]
    SandboxExec {
        #[arg(long)]
        policy: String,
        #[arg(last = true, required = true)]
        command: Vec<std::ffi::OsString>,
    },
    /// Print what the model would see for a session.
    Show {
        id: String,
        /// Print the raw event log instead.
        #[arg(long)]
        events: bool,
    },
    /// Close a session that was left mid-turn by a crash.
    Recover { id: String },
    /// Start a new session from a prefix of an existing one.
    Fork {
        id: String,
        /// Last record to copy. Defaults to the whole session.
        #[arg(long)]
        at: Option<u64>,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli).await {
        Ok(code) => code,
        Err(message) => {
            eprintln!("hx: {message}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<ExitCode, String> {
    // The internal commands run confined and must not touch the data
    // directory.
    let command = cli.command.unwrap_or(Command::Tui {
        session: None,
        plan: false,
    });
    match &command {
        Command::ToolWorker => {
            let code = hx_sandbox::worker_main(&hx_tools::ToolSet::builtin()).await;
            return Ok(ExitCode::from(code));
        }
        Command::SandboxExec { policy, command } => return sandbox_exec(policy, command),
        _ => {}
    }
    let home = cli
        .home
        .or_else(|| std::env::var_os("HX_HOME").map(PathBuf::from))
        .or_else(|| std::env::home_dir().map(|home| home.join(".hx")))
        .ok_or("cannot find a home directory; pass --home or set HX_HOME")?;
    let store = SessionStore::new(home.join("sessions"));

    match command {
        Command::Tui { session, plan } => {
            let resume = session.as_deref().map(parse_id).transpose()?;
            let cwd = match &resume {
                Some(id) => hx_app::session_cwd(&store, id)?,
                None => std::env::current_dir()
                    .and_then(|cwd| cwd.canonicalize())
                    .map_err(display)?,
            };
            let options = hx_app::Options {
                max_steps: 50,
                plan,
                ..hx_app::Options::default()
            };
            hx_tui::run(home, cwd, options, resume).await?;
        }
        Command::Run(args) => return run::run(&home, &store, args).await,
        Command::ToolWorker | Command::SandboxExec { .. } => unreachable!("handled above"),
        Command::Acp => {
            let options = hx_app::Options {
                max_steps: 50,
                ..hx_app::Options::default()
            };
            hx_acp::serve(home, options, tokio::io::stdin(), tokio::io::stdout())
                .await
                .map_err(display)?;
        }
        Command::Rewind { id, turn } => {
            let rewound = hx_app::rewind(&home, &store, &parse_id(&id)?, turn).await?;
            for name in &rewound.restored.removed {
                eprintln!("removed {name}");
            }
            eprintln!(
                "workspace restored to the start of turn {}; to undo: checkpoint {}",
                rewound.turn, rewound.restored.before
            );
            println!("{}", rewound.session);
        }
        Command::Import { format, file, cwd } => match format.as_str() {
            "dsh" => println!("{}", hx_app::import_dsh(&store, &file, cwd.as_deref())?),
            other => return Err(format!("cannot import {other:?} sessions; supported: dsh")),
        },
        Command::Sessions => {
            for header in store.list().map_err(display)? {
                println!("{}\t{}", header.id, header.cwd);
            }
        }
        Command::Show { id, events } => {
            let records = store.read(&parse_id(&id)?).map_err(display)?;
            if events {
                for record in &records {
                    println!("{}", serde_json::to_string(record).map_err(display)?);
                }
            } else {
                print_history(&records);
            }
        }
        Command::Recover { id } => {
            let id = parse_id(&id)?;
            let before = store.read(&id).map_err(display)?.len();
            let (records, _writer) = store.open(&id).map_err(display)?;
            match records.len().saturating_sub(before) {
                0 => println!("{id}: nothing to recover"),
                added => println!("{id}: closed with {added} recovery event(s)"),
            }
        }
        Command::Fork { id, at } => {
            let id = parse_id(&id)?;
            let at = match at {
                Some(at) => at,
                None => store.read(&id).map_err(display)?.len() as u64 - 1,
            };
            let options = hx_session::CreateOptions {
                id: None,
                cwd: cwd_of(&store.read(&id).map_err(display)?),
            };
            let (_records, writer) = store.fork(&id, at, options).map_err(display)?;
            println!("{}", writer.id());
        }
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(target_os = "linux")]
fn sandbox_exec(policy: &str, command: &[std::ffi::OsString]) -> Result<ExitCode, String> {
    use std::os::unix::process::CommandExt;
    let policy: hx_sandbox::SandboxPolicy =
        serde_json::from_str(policy).map_err(|e| format!("invalid sandbox policy: {e}"))?;
    hx_sandbox::apply_to_current_process(&policy).map_err(display)?;
    let error = std::process::Command::new(&command[0])
        .args(&command[1..])
        .exec();
    Err(format!(
        "cannot execute {}: {error}",
        command[0].to_string_lossy()
    ))
}

#[cfg(not(target_os = "linux"))]
fn sandbox_exec(_policy: &str, _command: &[std::ffi::OsString]) -> Result<ExitCode, String> {
    Err("sandbox-exec is only used on Linux".into())
}

fn parse_id(raw: &str) -> Result<SessionId, String> {
    SessionId::parse(raw).map_err(display)
}

fn display(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn cwd_of(records: &[Record]) -> String {
    match records.first().map(|record| &record.event) {
        Some(Event::Session(header)) => header.cwd.clone(),
        _ => String::new(),
    }
}

fn print_history(records: &[Record]) {
    let history = derive_history(records);
    if let Some(system) = &history.system {
        println!("[system]\n{system}\n");
    }
    for Message { role, content } in &history.messages {
        let label = match role {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        };
        println!("[{label}]");
        for block in content {
            print_block(block, 0);
        }
        println!();
    }
}

fn print_block(block: &Block, depth: usize) {
    let pad = "  ".repeat(depth);
    match block {
        Block::Text { text } => println!("{pad}{text}"),
        Block::Thinking { text, .. } => println!("{pad}(thinking) {text}"),
        Block::ToolUse { id, name, input } => println!("{pad}-> {name} {input} ({id})"),
        Block::ToolResult {
            call_id,
            content,
            is_error,
        } => {
            let status = if *is_error { "error" } else { "ok" };
            println!("{pad}<- {call_id} [{status}]");
            for inner in content {
                print_block(inner, depth + 1);
            }
        }
    }
}
