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
        /// A plugin directory or git repository, or a marketplace (a
        /// directory or repository with a catalogue of plugins).
        source: String,
        /// Which plugin of a marketplace to install.
        #[arg(long)]
        plugin: Option<String>,
        /// Where the plugin is inside the repository or directory.
        #[arg(long)]
        path: Option<PathBuf>,
        /// Install even though the scan found something dangerous, and
        /// replace a plugin of the same name.
        #[arg(long)]
        force: bool,
    },
    /// Fetch installed plugins again from where they came from, scan them
    /// again, and replace the installed copies.
    Update {
        /// The plugin to update. Without it, every installed plugin.
        name: Option<String>,
    },
    /// List installed plugins.
    List,
}

/// Where an installed plugin came from, kept beside it for updates.
const ORIGIN_FILE: &str = ".zcode-origin.json";

/// Fetches a git repository into a fresh temporary directory, at `commit`
/// when one is given.
fn fetch(url: &str, commit: Option<&str>, tag: &str) -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join(format!("zcode-install-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .status()
            .map_err(|e| format!("cannot run git: {e}"))
            .map(|status| status.success())
    };
    let at = dir.to_string_lossy().into_owned();
    if !git(&["clone", "--quiet", "--depth", "1", "--", url, &at])? {
        return Err(format!(
            "{url} is neither a directory nor a repository git can clone"
        ));
    }
    if let Some(commit) = commit {
        let fetched = git(&[
            "-C", &at, "fetch", "--quiet", "--depth", "1", "origin", commit,
        ])? && git(&["-C", &at, "checkout", "--quiet", "FETCH_HEAD"])?;
        if !fetched {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(format!("{url} has no commit or tag {commit}"));
        }
    }
    Ok(dir)
}

/// Installs one plugin from `source`, which may be a plugin or a
/// marketplace. Returns the process exit code.
fn install_plugin_from(
    home: &Path,
    source: &str,
    plugin: Option<&str>,
    path: Option<&Path>,
    force: bool,
    replace: bool,
) -> Result<ExitCode, String> {
    use zcode_ext::skill_install::{PluginSource, install_plugin, marketplace_entry};
    let mut fetched: Vec<PathBuf> = Vec::new();
    let result: Result<Outcome, String> = (|| {
        let local = PathBuf::from(source);
        let base = if local.is_dir() {
            local
        } else {
            let dir = fetch(source, None, "a")?;
            fetched.push(dir.clone());
            dir
        };
        let base = match path {
            Some(path) => base.join(path),
            None => base,
        };
        // A marketplace points at the plugin; anything else is the plugin.
        let plugin_dir = match marketplace_entry(&base, plugin)? {
            None => base,
            Some(PluginSource::Local(dir)) => dir,
            Some(PluginSource::Git { url, commit, path }) => {
                let dir = fetch(&url, commit.as_deref(), "b")?;
                fetched.push(dir.clone());
                match path {
                    Some(path) => dir.join(path),
                    None => dir,
                }
            }
        };
        let outcome = install_plugin(&plugin_dir, &home.join("plugins"), force, replace)?;
        if let Some(installed) = &outcome.0 {
            let origin = serde_json::json!({"source": source, "plugin": plugin, "path": path});
            let _ = std::fs::write(installed.dir.join(ORIGIN_FILE), origin.to_string());
        }
        Ok(outcome)
    })();
    for dir in fetched {
        let _ = std::fs::remove_dir_all(dir);
    }
    report(result?)
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
            plugin,
            path,
            force,
        } => install_plugin_from(
            home,
            &source,
            plugin.as_deref(),
            path.as_deref(),
            force,
            false,
        ),
        PluginCommand::Update { name } => {
            let mut failed = false;
            let mut updated = 0;
            for plugin in zcode_app::installed_plugins(home) {
                if name.as_ref().is_some_and(|name| *name != plugin.name) {
                    continue;
                }
                let origin = std::fs::read_to_string(plugin.root.join(ORIGIN_FILE))
                    .ok()
                    .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
                let Some(origin) = origin else {
                    eprintln!(
                        "zcode: {} has no recorded origin; install it again",
                        plugin.name
                    );
                    failed = true;
                    continue;
                };
                eprintln!("updating {}", plugin.name);
                let code = install_plugin_from(
                    home,
                    origin["source"].as_str().unwrap_or(""),
                    origin["plugin"].as_str(),
                    origin["path"].as_str().map(Path::new),
                    false,
                    true,
                )?;
                failed |= code != ExitCode::SUCCESS;
                updated += 1;
            }
            if updated == 0 && !failed {
                eprintln!("zcode: no installed plugin to update");
            }
            Ok(if failed {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            })
        }
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
        Some(fetch(source, None, "s")?)
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
    report(outcome?)
}

/// Prints what a scan found and whether the thing was installed.
fn report((installed, findings): Outcome) -> Result<ExitCode, String> {
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
