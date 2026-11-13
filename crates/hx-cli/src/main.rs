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
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run one turn: send a prompt and let the agent work until it answers.
    Run(run::RunArgs),
    /// List sessions.
    Sessions,
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
    let home = cli
        .home
        .or_else(|| std::env::var_os("HX_HOME").map(PathBuf::from))
        .or_else(|| std::env::home_dir().map(|home| home.join(".hx")))
        .ok_or("cannot find a home directory; pass --home or set HX_HOME")?;
    let store = SessionStore::new(home.join("sessions"));

    match cli.command {
        Command::Run(args) => return run::run(&store, args).await,
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
        Block::Thinking { text } => println!("{pad}(thinking) {text}"),
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
