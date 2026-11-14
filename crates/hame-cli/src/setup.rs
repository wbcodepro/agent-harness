//! `hame setup` and `hame doctor`: getting from a fresh install to a first
//! working session, and finding out what stands in the way.

use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

use clap::Args;
use hame_sandbox::{Mode, SandboxPolicy, SandboxedRunner};
use hame_tools::{Tool, ToolContext, ToolRunner};

#[derive(Args)]
pub struct DoctorArgs {
    /// Also send the model one short request (a few tokens) to prove the
    /// key, address and model name work.
    #[arg(long)]
    ping: bool,
}

#[derive(Args)]
pub struct SetupArgs {
    /// deepseek, anthropic, openai, or compatible (any server that speaks
    /// OpenAI's chat completions: OpenRouter, Gemini, Ollama, vLLM...).
    #[arg(long)]
    provider: Option<String>,
    /// Model name. Needed for openai and compatible.
    #[arg(long)]
    model: Option<String>,
    /// Address of a compatible server.
    #[arg(long)]
    base_url: Option<String>,
}

/// One line of the report.
struct Check {
    ok: Option<bool>,
}

impl Check {
    fn line(label: &str, ok: Option<bool>, text: &str) -> Self {
        let mark = match ok {
            Some(true) => "ok  ",
            Some(false) => "FAIL",
            None => "--  ",
        };
        println!("{mark} {label:<9} {text}");
        Self { ok }
    }
}

pub async fn doctor(home: &Path, args: DoctorArgs) -> Result<ExitCode, String> {
    let checks = report(home, args.ping).await;
    let failed = checks
        .iter()
        .filter(|check| check.ok == Some(false))
        .count();
    println!();
    if failed == 0 {
        println!("Ready. Start with `hame` in a project directory, or `hame run \"...\"`.");
        Ok(ExitCode::SUCCESS)
    } else {
        println!("{failed} check(s) failed; the lines above say what to do.");
        Ok(ExitCode::FAILURE)
    }
}

async fn report(home: &Path, ping: bool) -> Vec<Check> {
    let mut checks = Vec::new();
    println!("hame {}", env!("CARGO_PKG_VERSION"));
    checks.push(Check::line(
        "home",
        Some(true),
        &format!(
            "{}{}",
            home.display(),
            if home.is_dir() {
                ""
            } else {
                " (not created yet)"
            }
        ),
    ));

    let config = match hame_app::load_config(home) {
        Ok(config) => {
            let path = home.join("config.toml");
            let text = if path.is_file() {
                path.display().to_string()
            } else {
                "none; using defaults".into()
            };
            checks.push(Check::line("config", Some(true), &text));
            config
        }
        Err(e) => {
            checks.push(Check::line("config", Some(false), &e));
            return checks;
        }
    };

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let options = hame_app::Options::default();
    let secrets = match hame_app::load_secrets(home, &cwd) {
        Ok(secrets) => secrets,
        Err(e) => {
            checks.push(Check::line("keys", Some(false), &e));
            return checks;
        }
    };
    let chosen = hame_app::choose_model(&config, &secrets, &options);
    match &chosen {
        Ok(chosen) => {
            let from = if std::env::var_os(&chosen.key_name).is_some() {
                "the environment"
            } else {
                ".env"
            };
            checks.push(Check::line(
                "model",
                Some(true),
                &format!(
                    "{} {} at {} (provider from {}; key {} from {from})",
                    chosen.provider.name(),
                    chosen.model,
                    chosen.base_url,
                    chosen.provider_from,
                    chosen.key_name
                ),
            ));
        }
        Err(e) => checks.push(Check::line("model", Some(false), e)),
    }

    checks.push(sandbox_check(&config).await);
    checks.push(program_check(
        "bash",
        &hame_kernel::bash_program().to_string_lossy(),
        true,
        "the shell tool needs it; on Windows install Git for Windows",
    ));
    checks.push(program_check(
        "git",
        "git",
        true,
        "checkpoints and rewind need it",
    ));
    let python = if cfg!(windows) { "python" } else { "python3" };
    checks.push(program_check(
        "python",
        python,
        false,
        "optional: syntax checks of Python edits use it",
    ));

    if ping {
        let text = match &chosen {
            Err(_) => Check::line("ping", Some(false), "skipped: no model to ask"),
            Ok(_) => {
                let started = Instant::now();
                match hame_app::ping_model(&config, &secrets, &options).await {
                    Ok((answer, usage)) => Check::line(
                        "ping",
                        Some(true),
                        &format!(
                            "answered {:?} in {:.1} s ({} tokens in, {} out)",
                            answer.chars().take(40).collect::<String>(),
                            started.elapsed().as_secs_f64(),
                            usage.input_tokens + usage.cache_read_tokens,
                            usage.output_tokens
                        ),
                    ),
                    Err(e) => Check::line("ping", Some(false), &e),
                }
            }
        };
        checks.push(text);
    }
    checks
}

fn program_check(label: &str, program: &str, required: bool, why: &str) -> Check {
    let found = std::process::Command::new(program)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .next()
                .unwrap_or("")
                .trim()
                .to_string()
        });
    match found {
        Some(version) => Check::line(label, Some(true), &version),
        None if required => Check::line(label, Some(false), &format!("not found: {why}")),
        None => Check::line(label, None, &format!("not found ({why})")),
    }
}

/// Runs a command through the same confined worker sessions use, and
/// checks that a write outside the workspace is refused.
async fn sandbox_check(config: &hame_policy::Config) -> Check {
    let mechanism = if cfg!(target_os = "macos") {
        "Seatbelt"
    } else if cfg!(windows) {
        "low integrity level (writes only)"
    } else {
        "Landlock and seccomp"
    };
    if cfg!(windows)
        && !config.sandbox.allow_partial
        && config.sandbox.mode != Mode::DangerFullAccess
    {
        return Check::line(
            "sandbox",
            Some(false),
            "on Windows hame confines only where tools write; accept that with \
             `allow_partial = true` under [sandbox] in config.toml (`hame setup` asks), or \
             run with --sandbox danger-full-access and approve each change",
        );
    }
    let work = std::env::temp_dir().join(format!("hame-doctor-{}", std::process::id()));
    if let Err(e) = std::fs::create_dir_all(&work) {
        return Check::line(
            "sandbox",
            Some(false),
            &format!("cannot make a test directory: {e}"),
        );
    }
    let work = hame_kernel::real_path(&work).unwrap_or(work);
    let result = confined_probe(&work).await;
    let _ = std::fs::remove_dir_all(&work);
    match result {
        Ok(()) => {
            let note = if config.sandbox.mode == Mode::DangerFullAccess {
                " (but config.toml turns it off: sandbox mode is danger-full-access)"
            } else {
                ""
            };
            Check::line(
                "sandbox",
                Some(true),
                &format!(
                    "tools run confined by {mechanism}; a write outside the workspace was refused{note}"
                ),
            )
        }
        Err(e) => Check::line("sandbox", Some(false), &e),
    }
}

async fn confined_probe(work: &Path) -> Result<(), String> {
    #[allow(unused_mut)]
    let mut policy = SandboxPolicy::workspace(work);
    #[cfg(windows)]
    {
        policy.unreadable.clear();
        policy.hide_env_files = false;
        policy.network = hame_sandbox::Network::Open;
    }
    hame_sandbox::prepare(&policy).map_err(|e| e.to_string())?;
    let runner = SandboxedRunner {
        helper: std::env::current_exe().map_err(|e| e.to_string())?,
        policy,
        env: Vec::new(),
        background: Default::default(),
    };
    let bash: Arc<dyn Tool> = Arc::new(hame_tools::Bash);
    let ctx = ToolContext::new(work);
    let inside = runner
        .run(
            &bash,
            &serde_json::json!({"command": "echo ok > inside.txt && cat inside.txt"}),
            &ctx,
        )
        .await;
    if inside.is_error || !inside.text.starts_with("ok") {
        return Err(format!(
            "a confined command could not run: {}",
            inside.text.lines().next().unwrap_or("")
        ));
    }
    let outside = std::env::home_dir()
        .ok_or("no home directory to test against")?
        .join(format!(".hame-doctor-probe-{}", std::process::id()));
    let quoted = outside
        .to_string_lossy()
        .replace('\\', "/")
        .replace('\'', "'\\''");
    runner
        .run(
            &bash,
            &serde_json::json!({ "command": format!("echo x > '{quoted}'") }),
            &ctx,
        )
        .await;
    if outside.exists() {
        let _ = std::fs::remove_file(&outside);
        return Err(
            "a confined command wrote outside the workspace: the sandbox is not working \
                    here; do not run hame until this is fixed"
                .into(),
        );
    }
    Ok(())
}

pub async fn setup(home: &Path, args: SetupArgs) -> Result<ExitCode, String> {
    let interactive = std::io::stdin().is_terminal();
    let provider = match args.provider {
        Some(provider) => provider,
        None if interactive => choose_provider()?,
        None => return Err("pass --provider when not at a terminal".into()),
    };
    let provider = provider.to_ascii_lowercase();
    let (config_provider, base_url, key_name, model) = match provider.as_str() {
        "deepseek" | "anthropic" | "claude" | "openai" => {
            let parsed = hame_app::Provider::parse(&provider)?;
            let model = match (args.model, parsed) {
                (Some(model), _) => Some(model),
                (None, hame_app::Provider::Openai) => Some(ask("Model name (e.g. gpt-6.1)")?),
                (None, _) => None,
            };
            (
                parsed.name(),
                args.base_url,
                parsed.key_names()[0].to_string(),
                model,
            )
        }
        "compatible" => {
            let base_url = match args.base_url {
                Some(url) => url,
                None => ask(
                    "Server address (e.g. https://openrouter.ai/api/v1, http://localhost:11434/v1)",
                )?,
            };
            let model = match args.model {
                Some(model) => model,
                None => ask("Model name")?,
            };
            (
                "openai",
                Some(base_url),
                "HAME_API_KEY".to_string(),
                Some(model),
            )
        }
        other => {
            return Err(format!(
                "unknown provider {other:?}: use deepseek, anthropic, openai or compatible"
            ));
        }
    };

    let env_file = home.join(".env");
    let existing = std::env::var(&key_name)
        .ok()
        .filter(|value| !value.is_empty())
        .is_some()
        || read_env_names(&env_file).contains(&key_name);
    let prompt = if existing {
        format!("{key_name} is already set; paste a new key, or press Enter to keep it")
    } else if provider == "compatible" {
        format!("API key, saved as {key_name} (any text for a local server that needs none)")
    } else {
        format!("API key, saved as {key_name}")
    };
    let key = if interactive {
        read_hidden(&prompt)?
    } else {
        let mut line = String::new();
        std::io::stdin()
            .lock()
            .read_line(&mut line)
            .map_err(|e| e.to_string())?;
        line.trim().to_string()
    };
    std::fs::create_dir_all(home).map_err(|e| e.to_string())?;
    if !key.is_empty() {
        save_env(&env_file, &key_name, &key)?;
        println!(
            "Saved {key_name} in {} (readable by you only).",
            env_file.display()
        );
    } else if !existing {
        return Err(format!(
            "no key given; nothing saved. Run `hame setup` again when you have one for {provider}"
        ));
    }

    let mut settings: Vec<(&str, &str, toml::Value)> =
        vec![("model", "provider", config_provider.into())];
    if let Some(model) = &model {
        settings.push(("model", "name", model.clone().into()));
    }
    if let Some(base_url) = &base_url {
        settings.push(("model", "base_url", base_url.clone().into()));
    }
    if provider == "compatible" {
        settings.push(("model", "api", "chat-completions".into()));
    }
    if cfg!(windows) && interactive {
        println!(
            "\nOn Windows hame can confine where tools write, but cannot hide your files from \
             them or limit their network access."
        );
        if ask("Accept that and keep tools confined for writes? [Y/n]")?
            .to_ascii_lowercase()
            .starts_with('n')
        {
            println!(
                "Then run hame with --sandbox danger-full-access; it asks before every change."
            );
        } else {
            settings.push(("sandbox", "allow_partial", true.into()));
        }
    }
    let config_file = home.join("config.toml");
    if let Some(backup) = update_config(&config_file, &settings)? {
        println!(
            "Updated {} (the previous one, with its comments, is at {}).",
            config_file.display(),
            backup.display()
        );
    } else {
        println!("Wrote {}.", config_file.display());
    }

    println!();
    let checks = report(home, false).await;
    if checks.iter().any(|check| check.ok == Some(false)) {
        return Ok(ExitCode::FAILURE);
    }
    let ping = interactive
        && !ask("\nSend the model one short test request (a few tokens)? [Y/n]")?
            .to_ascii_lowercase()
            .starts_with('n');
    if ping {
        let config = hame_app::load_config(home)?;
        let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
        let secrets = hame_app::load_secrets(home, &cwd)?;
        let started = Instant::now();
        match hame_app::ping_model(&config, &secrets, &hame_app::Options::default()).await {
            Ok(_) => println!(
                "The model answered in {:.1} s. Ready: run `hame` in a project directory.",
                started.elapsed().as_secs_f64()
            ),
            Err(e) => {
                println!("The test request failed: {e}");
                return Ok(ExitCode::FAILURE);
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn choose_provider() -> Result<String, String> {
    println!("Which model provider?");
    println!("  1) DeepSeek     cheapest; key from platform.deepseek.com");
    println!("  2) Anthropic    Claude models; key from console.anthropic.com");
    println!("  3) OpenAI       key from platform.openai.com");
    println!("  4) Another server that speaks OpenAI's chat completions");
    println!("     (OpenRouter, Gemini's OpenAI endpoint, Ollama, vLLM, ...)");
    loop {
        match ask("Choose 1-4")?.as_str() {
            "1" => return Ok("deepseek".into()),
            "2" => return Ok("anthropic".into()),
            "3" => return Ok("openai".into()),
            "4" => return Ok("compatible".into()),
            _ => println!("Type a number from 1 to 4."),
        }
    }
}

fn ask(prompt: &str) -> Result<String, String> {
    print!("{prompt}: ");
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    let mut line = String::new();
    let read = std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    if read == 0 {
        return Err("input ended".into());
    }
    Ok(line.trim().to_string())
}

/// Reads a line without showing it, so a key pasted in does not stay on
/// the screen or in a terminal recording.
fn read_hidden(prompt: &str) -> Result<String, String> {
    use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, read};
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode};

    print!("{prompt}: ");
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    enable_raw_mode().map_err(|e| e.to_string())?;
    let mut text = String::new();
    let outcome = loop {
        match read() {
            Ok(Event::Key(key)) if key.kind != KeyEventKind::Release => match key.code {
                KeyCode::Enter => break Ok(()),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    break Err("cancelled".to_string());
                }
                KeyCode::Char(c) => text.push(c),
                KeyCode::Backspace => {
                    text.pop();
                }
                KeyCode::Esc => break Err("cancelled".to_string()),
                _ => {}
            },
            Ok(Event::Paste(pasted)) => text.push_str(&pasted),
            Ok(_) => {}
            Err(e) => break Err(e.to_string()),
        }
    };
    let _ = disable_raw_mode();
    println!();
    outcome.map(|()| text.trim().to_string())
}

fn read_env_names(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let line = line.trim().strip_prefix("export ").unwrap_or(line.trim());
            line.split_once('=')
                .map(|(name, _)| name.trim().to_string())
        })
        .collect()
}

/// Sets `name` in a `.env` file, replacing an earlier value and keeping
/// every other line, and leaves the file readable by its owner only.
fn save_env(path: &Path, name: &str, value: &str) -> Result<(), String> {
    if value.contains(['\n', '\r']) {
        return Err("the key contains a line break; paste it again".into());
    }
    let old = std::fs::read_to_string(path).unwrap_or_default();
    let mut lines: Vec<String> = old
        .lines()
        .filter(|line| {
            let line = line.trim().strip_prefix("export ").unwrap_or(line.trim());
            line.split_once('=')
                .is_none_or(|(existing, _)| existing.trim() != name)
        })
        .map(String::from)
        .collect();
    lines.push(format!("{name}={value}"));
    let text = lines.join("\n") + "\n";
    hame_tools::write_atomic(path, text.as_bytes()).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Sets `table.key = value` for each setting in config.toml. A file that
/// had comments is kept next to it first, since rewriting the file drops
/// them; returns where.
fn update_config(
    path: &Path,
    settings: &[(&str, &str, toml::Value)],
) -> Result<Option<PathBuf>, String> {
    let old = std::fs::read_to_string(path).unwrap_or_default();
    let mut table: toml::Table = old
        .parse()
        .map_err(|e| format!("{} is not valid TOML: {e}", path.display()))?;
    for (section, key, value) in settings {
        let entry = table
            .entry(section.to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        let toml::Value::Table(section_table) = entry else {
            return Err(format!("{section} in {} is not a table", path.display()));
        };
        section_table.insert(key.to_string(), value.clone());
    }
    let text = toml::to_string_pretty(&table).map_err(|e| e.to_string())?;
    // What is about to be written must load, or the next run fails.
    hame_policy::Config::parse(&text)?;
    let backup = if old.contains('#') {
        let backup = path.with_extension("toml.bak");
        std::fs::write(&backup, &old).map_err(|e| e.to_string())?;
        Some(backup)
    } else {
        None
    };
    hame_tools::write_atomic(path, text.as_bytes()).map_err(|e| e.to_string())?;
    Ok(backup)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_key_replaces_the_old_one_and_keeps_other_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        std::fs::write(&path, "OTHER=1\nexport DEEPSEEK_API_KEY=old\n").unwrap();
        save_env(&path, "DEEPSEEK_API_KEY", "new").unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "OTHER=1\nDEEPSEEK_API_KEY=new\n"
        );
        assert!(save_env(&path, "X", "a\nb").is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn settings_are_merged_into_an_existing_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "# mine\n[model]\neffort = \"high\"\n\n[sandbox]\nwritable_tmp = false\n",
        )
        .unwrap();
        let backup = update_config(
            &path,
            &[
                ("model", "provider", "anthropic".into()),
                ("sandbox", "allow_partial", true.into()),
            ],
        )
        .unwrap();
        let config = hame_policy::Config::load(&path).unwrap();
        assert_eq!(config.model.provider.as_deref(), Some("anthropic"));
        assert_eq!(config.model.effort.as_deref(), Some("high"));
        assert!(!config.sandbox.writable_tmp && config.sandbox.allow_partial);
        let backup = backup.expect("comments are kept in a backup");
        assert!(
            std::fs::read_to_string(backup)
                .unwrap()
                .starts_with("# mine")
        );

        // A setting the configuration does not know is refused before writing.
        let before = std::fs::read_to_string(&path).unwrap();
        assert!(update_config(&path, &[("model", "nope", 1.into())]).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    }
}
