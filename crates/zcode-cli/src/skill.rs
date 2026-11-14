//! `zcode skill`: install skills from elsewhere, after scanning them.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Subcommand;
use zcode_ext::skill_install::{Severity, install};

#[derive(Subcommand)]
pub enum SkillCommand {
    /// Copy a skill into your skills directory after scanning it for
    /// hidden instructions and dangerous commands.
    Install {
        /// A directory holding a SKILL.md, or a git repository URL.
        source: String,
        /// Where the skill is inside the repository or directory.
        #[arg(long)]
        path: Option<PathBuf>,
        /// Install even though the scan found something dangerous, and
        /// replace a skill of the same name.
        #[arg(long)]
        force: bool,
    },
    /// List the skills the agent can load in this directory.
    List,
}

pub fn run(home: &Path, command: SkillCommand) -> Result<ExitCode, String> {
    match command {
        SkillCommand::List => {
            let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
            let skills = zcode_ext::skills::discover(&[
                cwd.join(".zcode/skills"),
                cwd.join(".claude/skills"),
                home.join("skills"),
            ]);
            for skill in skills {
                println!(
                    "{}\t{}\t{}",
                    skill.name,
                    skill.dir.display(),
                    skill.description
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        SkillCommand::Install {
            source,
            path,
            force,
        } => install_from(&source, path.as_deref(), |dir| {
            install(dir, &home.join("skills"), force)
        }),
    }
}

#[derive(Subcommand)]
pub enum PluginCommand {
    /// Copy a plugin packaged for Claude Code into your plugins directory
    /// after scanning it. Installed plugins are loaded in every session.
    Install {
        /// A plugin directory, or a git repository URL.
        source: String,
        /// Where the plugin is inside the repository or directory.
        #[arg(long)]
        path: Option<PathBuf>,
        /// Install even though the scan found something dangerous, and
        /// replace a plugin of the same name.
        #[arg(long)]
        force: bool,
    },
    /// List installed plugins.
    List,
}

pub fn run_plugin(home: &Path, command: PluginCommand) -> Result<ExitCode, String> {
    match command {
        PluginCommand::List => {
            for plugin in zcode_app::installed_plugins(home) {
                println!("{}\t{}", plugin.name, plugin.root.display());
            }
            Ok(ExitCode::SUCCESS)
        }
        PluginCommand::Install {
            source,
            path,
            force,
        } => install_from(&source, path.as_deref(), |dir| {
            zcode_ext::skill_install::install_plugin(dir, &home.join("plugins"), force)
        }),
    }
}

type Outcome = (
    Option<zcode_ext::skill_install::Installed>,
    Vec<zcode_ext::skill_install::Finding>,
);

/// Fetches `source` if it is not a local directory, hands the directory to
/// `install`, and reports what the scan found.
fn install_from(
    source: &str,
    path: Option<&Path>,
    install: impl FnOnce(&Path) -> Result<Outcome, String>,
) -> Result<ExitCode, String> {
    // A local directory is used where it is; anything else is fetched with
    // git into a directory that is removed afterwards.
    let local = PathBuf::from(source);
    let fetched = if local.is_dir() {
        None
    } else {
        let dir = std::env::temp_dir().join(format!("zcode-install-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cloned = std::process::Command::new("git")
            .args(["clone", "--quiet", "--depth", "1", "--"])
            .arg(source)
            .arg(&dir)
            .status()
            .map_err(|e| format!("cannot run git: {e}"))?;
        if !cloned.success() {
            return Err(format!(
                "{source} is neither a directory nor a repository git can clone"
            ));
        }
        Some(dir)
    };
    let base = fetched.clone().unwrap_or(local);
    let target = match path {
        Some(path) => base.join(path),
        None => base,
    };
    let outcome = install(&target);
    if let Some(dir) = fetched {
        let _ = std::fs::remove_dir_all(dir);
    }
    let (installed, findings) = outcome?;

    for finding in &findings {
        let place = match finding.line {
            0 => finding.file.clone(),
            line => format!("{}:{line}", finding.file),
        };
        let label = match finding.severity {
            Severity::Danger => "DANGER",
            Severity::Notice => "notice",
        };
        eprintln!("{label}  {place}  {}", finding.what);
    }
    match installed {
        Some(installed) => {
            println!(
                "installed {} in {}",
                installed.name,
                installed.dir.display()
            );
            Ok(ExitCode::SUCCESS)
        }
        None => {
            eprintln!(
                "zcode: not installed. Read the lines marked DANGER above; if you have checked \
                 them and still want it, run again with --force."
            );
            Ok(ExitCode::FAILURE)
        }
    }
}

#[derive(Subcommand)]
pub enum McpCommand {
    /// Sign in to an MCP server that uses OAuth. Opens a browser; the
    /// tokens are kept in the data directory and renewed when needed.
    Login {
        /// The server's name in the configuration.
        server: String,
    },
    /// Forget the tokens of a server.
    Logout { server: String },
}

pub async fn run_mcp(home: &Path, command: McpCommand) -> Result<ExitCode, String> {
    match command {
        McpCommand::Login { server } => {
            let config = zcode_app::mcp_http_server(home, &server)?;
            let url = config.url.clone().unwrap_or_default();
            let client = config
                .oauth_client_id
                .as_deref()
                .map(|id| (id, config.oauth_redirect_port));
            let tokens = zcode_ext::mcp_auth::login(&url, client, |address| {
                eprintln!("Open this address to sign in to {server}:\n\n{address}\n");
                // Best effort: the address above works by hand too.
                let opener = if cfg!(target_os = "macos") {
                    ("open", vec![address])
                } else if cfg!(windows) {
                    ("rundll32", vec!["url.dll,FileProtocolHandler", address])
                } else {
                    ("xdg-open", vec![address])
                };
                let _ = std::process::Command::new(opener.0)
                    .args(opener.1)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn();
            })
            .await?;
            zcode_ext::mcp_auth::save(&zcode_ext::mcp_auth::token_path(home, &server), &tokens)?;
            println!("signed in to {server}");
            Ok(ExitCode::SUCCESS)
        }
        McpCommand::Logout { server } => {
            let path = zcode_ext::mcp_auth::token_path(home, &server);
            match std::fs::remove_file(&path) {
                Ok(()) => println!("signed out of {server}"),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    println!("not signed in to {server}");
                }
                Err(e) => return Err(format!("cannot remove {}: {e}", path.display())),
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}
