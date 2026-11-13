//! Turns the user's configuration into a runnable [`Agent`]: model adapter,
//! confined tool runner, egress proxy, permission layers and checkpoints.
//! Every front end (one-shot CLI, TUI, server) builds its agent here so they
//! all enforce the same policy.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use hx_agent::{Agent, Checkpointer, ToolPipeline, TurnHook};
use hx_checkpoint::GitCheckpoints;
use hx_context::{Compactor, SpillLayer, load_instructions};
use hx_eval::{Recording, ScriptedLlm};
use hx_kernel::BoxFuture;
use hx_llm::{ChatCompletions, Llm};
use hx_policy::{
    ApprovalLayer, Config, NetworkMode, ProjectConfig, Prompter, RedactLayer, RuleAction, Secrets,
};
use hx_sandbox::{Allowlist, EgressProxy, Mode, Network, SandboxPolicy, SandboxedRunner};
use hx_tools::{Bash, ToolContext, ToolRunner, ToolSet};
use hx_verify::Verifier;

pub use hx_policy::NetworkMode as NetworkChoice;
pub use hx_sandbox::Mode as SandboxMode;

const DEFAULT_BASE_URL: &str = "https://api.deepseek.com";
const DEFAULT_MODEL: &str = "deepseek-flash";

/// Per-invocation choices that override the configuration file.
#[derive(Debug, Clone, Default)]
pub struct Options {
    pub sandbox: Option<Mode>,
    pub network: Option<NetworkMode>,
    /// Answer every approval question with yes.
    pub yes: bool,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub max_steps: u64,
}

/// An agent plus the background pieces that must live as long as it does.
pub struct Runtime {
    pub agent: Agent,
    pub policy: SandboxPolicy,
    /// Present when the network is limited to an allowlist.
    pub proxy: Option<EgressProxy>,
    pub secrets: Secrets,
}

/// The directory where checkpoints of `cwd` are kept.
pub fn checkpoint_dir(home: &Path, cwd: &Path) -> PathBuf {
    // FNV-1a: stable across builds, unlike the standard hasher.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in cwd.to_string_lossy().bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    home.join("checkpoints").join(format!("{hash:016x}"))
}

pub fn load_config(home: &Path) -> Result<Config, String> {
    Config::load(&home.join("config.toml")).map_err(|e| e.to_string())
}

/// Everything the harness may use as a credential: the process environment,
/// then `$HX_HOME/.env`, then `.env` in the working directory.
pub fn load_secrets(home: &Path, cwd: &Path) -> Result<Secrets, String> {
    let mut secrets = Secrets::new();
    secrets.load_process_env();
    for file in [home.join(".env"), cwd.join(".env")] {
        secrets
            .load_env_file(&file)
            .map_err(|e| format!("cannot read {}: {e}", file.display()))?;
    }
    Ok(secrets)
}

pub async fn build(home: &Path, cwd: &Path, options: &Options) -> Result<Runtime, String> {
    let config = load_config(home)?;
    let secrets = load_secrets(home, cwd)?;
    let llm = llm(&config, &secrets, options)?;

    // Confinement.
    let mode = options.sandbox.unwrap_or(config.sandbox.mode);
    let mut policy = match mode {
        Mode::WorkspaceWrite => {
            let mut policy = SandboxPolicy::workspace(cwd);
            if !config.sandbox.writable_tmp {
                policy.writable.truncate(1);
            }
            policy.writable.extend(config.sandbox.writable.clone());
            policy
        }
        Mode::ReadOnly => SandboxPolicy::read_only(),
        Mode::DangerFullAccess => SandboxPolicy::full_access(),
    };
    let mut proxy = None;
    if policy.is_confined() {
        policy.unreadable.extend(config.sandbox.unreadable.clone());
        // The harness's own data: credentials, settings, other sessions.
        // The spill directory stays readable; tools are pointed at it.
        let home = home.canonicalize().unwrap_or_else(|_| home.to_path_buf());
        for private in [".env", "config.toml", "sessions", "checkpoints"] {
            policy.unreadable.push(home.join(private));
        }
        policy.hide_env_files = config.sandbox.hide_env_files;
        policy.network = match options.network.unwrap_or(config.network.mode) {
            NetworkMode::Off => Network::Blocked,
            NetworkMode::Open => Network::Open,
            NetworkMode::Allowlist => {
                let started = EgressProxy::start(Allowlist::new(config.network.allow.clone()))
                    .await
                    .map_err(|e| format!("cannot start the egress proxy: {e}"))?;
                let port = started.port();
                proxy = Some(started);
                Network::Proxy { port }
            }
        };
    }

    let granted: Vec<(String, String)> = config
        .secrets
        .grant
        .iter()
        .filter_map(|name| Some((name.clone(), secrets.get(name)?.to_string())))
        .collect();
    let runner: Arc<dyn ToolRunner> = Arc::new(SandboxedRunner {
        helper: std::env::current_exe()
            .map_err(|e| format!("cannot find the hx executable: {e}"))?,
        policy: policy.clone(),
        env: granted,
    });

    // Layers, outermost first: spill what is left after redaction, redact
    // (so refusals are covered too), then decide whether the call runs.
    let pipeline = ToolPipeline::new();
    pipeline.push(SpillLayer {
        dir: checkpoint_dir(home, cwd)
            .file_name()
            .map(|workspace| home.join("spill").join(workspace))
            .unwrap_or_else(|| home.join("spill")),
        threshold: config.context.spill_at_bytes,
        preview: config.context.spill_at_bytes / 4,
    });
    pipeline.push(RedactLayer {
        secrets: secrets.clone(),
    });
    let prompter: Arc<dyn Prompter> = if options.yes {
        Arc::new(Always(Some(true)))
    } else if std::io::stdin().is_terminal() {
        Arc::new(Terminal)
    } else {
        Arc::new(Always(None))
    };
    pipeline.push(ApprovalLayer {
        rules: config.rules.clone(),
        default: config.approval.default.unwrap_or(if policy.is_confined() {
            RuleAction::Allow
        } else {
            RuleAction::Ask
        }),
        prompter,
    });

    let checkpointer: Option<Arc<dyn Checkpointer>> =
        if config.checkpoints.enabled && checkpointable(cwd) {
            Some(Arc::new(GitCheckpoints::new(
                checkpoint_dir(home, cwd),
                cwd,
            )))
        } else {
            None
        };

    let mut hooks: Vec<Arc<dyn TurnHook>> = vec![Arc::new(Compactor {
        llm: Arc::clone(&llm),
        threshold_tokens: config.context.compact_at_tokens,
    })];
    let project = ProjectConfig::load(&cwd.join(".hx/config.toml")).map_err(|e| e.to_string())?;
    let verify = project.verify.unwrap_or_else(|| config.verify.clone());
    if !verify.commands.is_empty() {
        hooks.push(Arc::new(Verifier {
            commands: verify.commands,
            max_rounds: verify.max_rounds,
            timeout_ms: verify.timeout_ms,
            runner: Arc::clone(&runner),
            shell: Arc::new(Bash),
            context: ToolContext::new(cwd),
        }));
    }

    let agent = Agent {
        llm,
        tools: ToolSet::builtin(),
        pipeline,
        tool_context: ToolContext::new(cwd),
        runner,
        checkpointer,
        hooks,
        system: system_prompt(cwd, &policy) + &load_instructions(home, cwd),
        max_steps: options.max_steps,
        max_tokens: None,
    };
    Ok(Runtime {
        agent,
        policy,
        proxy,
        secrets,
    })
}

/// Checkpointing a home directory or a filesystem root would try to copy
/// everything the user owns.
fn checkpointable(cwd: &Path) -> bool {
    let is_home = std::env::home_dir().is_some_and(|home| home == cwd);
    let has_git = std::process::Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success());
    !is_home && cwd.parent().is_some() && has_git
}

fn system_prompt(cwd: &Path, policy: &SandboxPolicy) -> String {
    let confinement = match (policy.mode, policy.network) {
        (Mode::DangerFullAccess, _) => {
            "Tools run with the user's full privileges; changes may need the user's approval."
                .to_string()
        }
        (mode, network) => format!(
            "Tools run in a sandbox. {} {} Credential files and .env files cannot be read. \
             If a tool fails because of the sandbox, say so instead of working around it.",
            match mode {
                Mode::ReadOnly => "Nothing can be written.",
                _ =>
                    "Files can be written only under the working directory and the temp directories.",
            },
            match network {
                Network::Blocked => "There is no network access.",
                Network::Proxy { .. } =>
                    "Network access is limited to an allowlist of hosts through an HTTP proxy.",
                Network::Open => "Network access is open.",
            }
        ),
    };
    format!(
        "You are hx, a coding agent working in the user's terminal.\n\
         Working directory: {}\n\
         Use the tools to inspect and change files and to run commands; do not guess at file \
         contents. Be concise. When the task is done, say what you did and what you verified.\n\
         {confinement}",
        cwd.display()
    )
}

fn llm(config: &Config, secrets: &Secrets, options: &Options) -> Result<Arc<dyn Llm>, String> {
    // Replays recorded responses instead of calling a provider; for tests
    // and evals.
    if let Some(script) = std::env::var_os("HX_SCRIPT") {
        let delay = std::env::var("HX_SCRIPT_DELAY_MS")
            .ok()
            .and_then(|ms| ms.parse().ok())
            .unwrap_or(0);
        let recording = Recording::load(&script).map_err(|e| e.to_string())?;
        return Ok(Arc::new(
            ScriptedLlm::new(recording).with_delay(Duration::from_millis(delay)),
        ));
    }
    let env = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
    let key_names: Vec<&str> = match &config.model.api_key_env {
        Some(name) => vec![name.as_str()],
        None => vec!["HX_API_KEY", "DEEPSEEK_API_KEY"],
    };
    let api_key = key_names
        .iter()
        .find_map(|name| env(name).or_else(|| secrets.get(name).map(String::from)))
        .ok_or_else(|| format!("no API key: set {}", key_names.join(" or ")))?;
    let base_url = options
        .base_url
        .clone()
        .or_else(|| env("HX_BASE_URL"))
        .or_else(|| config.model.base_url.clone())
        .unwrap_or_else(|| DEFAULT_BASE_URL.into());
    let model = options
        .model
        .clone()
        .or_else(|| env("HX_MODEL"))
        .or_else(|| config.model.name.clone())
        .unwrap_or_else(|| DEFAULT_MODEL.into());
    Ok(Arc::new(ChatCompletions::new(&base_url, api_key, model)))
}

/// Gives every question the same answer.
struct Always(Option<bool>);

impl Prompter for Always {
    fn ask(&self, _question: String) -> BoxFuture<'_, Option<bool>> {
        Box::pin(async move { self.0 })
    }
}

/// Asks on the controlling terminal.
struct Terminal;

impl Prompter for Terminal {
    fn ask(&self, question: String) -> BoxFuture<'_, Option<bool>> {
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                eprint!("{question}");
                let mut answer = String::new();
                let read = std::io::stdin().read_line(&mut answer).is_ok();
                Some(read && matches!(answer.trim(), "y" | "Y" | "yes"))
            })
            .await
            .unwrap_or(None)
        })
    }
}
