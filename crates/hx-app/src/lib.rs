//! Turns the user's configuration into a runnable [`Agent`]: model adapter,
//! confined tool runner, egress proxy, permission layers and checkpoints.
//! Every front end (one-shot CLI, TUI, server) builds its agent here so they
//! all enforce the same policy.

mod import;

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use hx_agent::{Agent, Checkpointer, ToolPipeline, TurnHook};
use hx_agent::{Session, ToolRequest};
use hx_checkpoint::GitCheckpoints;
use hx_checkpoint::Restored;
use hx_context::{Compactor, SpillLayer, load_instructions};
use hx_eval::{Recording, ScriptedLlm};
use hx_kernel::BoxFuture;
use hx_llm::{ChatCompletions, Llm};
use hx_policy::{
    ApprovalLayer, Config, NetworkMode, ProjectConfig, RedactLayer, RuleAction, Secrets,
};
use hx_sandbox::{Allowlist, EgressProxy, Mode, Network, SandboxPolicy, SandboxedRunner};
use hx_session::{CreateOptions, Event, SessionId, SessionStore};
use hx_tools::{Bash, Effect, ToolContext, ToolRunner, ToolSet};
use hx_verify::Verifier;

pub use hx_policy::NetworkMode as NetworkChoice;
pub use hx_policy::Prompter;
pub use hx_sandbox::Mode as SandboxMode;
pub use import::import_dsh;

/// A model provider hx can talk to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Deepseek,
    Openai,
    Anthropic,
}

impl Provider {
    pub fn parse(name: &str) -> Result<Self, String> {
        match name.to_ascii_lowercase().as_str() {
            "deepseek" => Ok(Self::Deepseek),
            "openai" => Ok(Self::Openai),
            "anthropic" | "claude" => Ok(Self::Anthropic),
            other => Err(format!(
                "unknown provider {other:?}: use deepseek, openai or anthropic"
            )),
        }
    }

    /// Environment variables (or secrets) searched for the key, in order.
    fn key_names(self) -> &'static [&'static str] {
        match self {
            Self::Deepseek => &["DEEPSEEK_API_KEY"],
            Self::Openai => &["OPENAI_API_KEY"],
            Self::Anthropic => &["ANTHROPIC_API_KEY", "CLAUDE_API_KEY"],
        }
    }

    fn base_url(self) -> &'static str {
        match self {
            Self::Deepseek => "https://api.deepseek.com",
            Self::Openai => "https://api.openai.com/v1",
            Self::Anthropic => "https://api.anthropic.com",
        }
    }

    fn default_model(self) -> Option<&'static str> {
        match self {
            Self::Deepseek => Some("deepseek-flash"),
            Self::Anthropic => Some("claude-opus-5-5"),
            // No safe default: model names here change often.
            Self::Openai => None,
        }
    }
}

/// Per-invocation choices that override the configuration file.
#[derive(Clone, Default)]
pub struct Options {
    pub sandbox: Option<Mode>,
    pub network: Option<NetworkMode>,
    /// Answer every approval question with yes.
    pub yes: bool,
    /// `deepseek`, `openai` or `anthropic`. Defaults to the configuration,
    /// then to whichever provider a key is available for.
    pub provider: Option<String>,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub max_steps: u64,
    /// Plan mode: nothing can be changed, and the agent is told to produce
    /// a plan instead of doing the work.
    pub plan: bool,
    /// Who answers approval questions. Defaults to the terminal when there
    /// is one, otherwise nobody.
    pub prompter: Option<Arc<dyn Prompter>>,
    /// Model to use instead of the configured provider.
    pub llm: Option<Arc<dyn Llm>>,
    /// Tool runner to use instead of confined worker processes. For
    /// embedding and tests; it bypasses the sandbox.
    pub runner: Option<Arc<dyn ToolRunner>>,
    /// Set by the harness when it builds a subagent: how deep the agent is,
    /// and what it shares with the agents above it.
    pub lineage: Option<Lineage>,
}

/// What a subagent inherits from the agent that started it.
#[derive(Clone)]
pub struct Lineage {
    depth: u32,
    budget: Option<Arc<hx_multi::TreeBudget>>,
    /// The task board of the root agent's workspace.
    board: hx_multi::TaskBoard,
}

const PLAN_MODE: &str = "\n\nYou are in plan mode. Investigate with the read-only tools, then \
answer with a concrete, step-by-step plan for the user to approve: what you will change, in \
which files, and how you will verify it. Do not change anything; the sandbox is read-only.";

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

/// Assembles an agent for `cwd` from the configuration under `home`.
pub fn build<'a>(
    home: &'a Path,
    cwd: &'a Path,
    options: &'a Options,
) -> BoxFuture<'a, Result<Runtime, String>> {
    // Boxed because building an agent registers a factory that builds its
    // subagents with this same function.
    Box::pin(build_inner(home, cwd, options))
}

async fn build_inner(home: &Path, cwd: &Path, options: &Options) -> Result<Runtime, String> {
    // Paths end up on other programs' command lines; keep them in the form
    // those programs accept.
    let (home, cwd) = (
        &hx_kernel::simplify_path(home),
        &hx_kernel::simplify_path(cwd),
    );
    let config = load_config(home)?;
    let secrets = load_secrets(home, cwd)?;
    let (llm, summarizer) = match &options.llm {
        Some(llm) => (Arc::clone(llm), Arc::clone(llm)),
        None => llm(&config, &secrets, options)?,
    };

    // Confinement.
    let mode = if options.plan {
        Mode::ReadOnly
    } else {
        options.sandbox.unwrap_or(config.sandbox.mode)
    };
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
    let writes_only = cfg!(windows) && policy.is_confined();
    if writes_only {
        if !config.sandbox.allow_partial {
            return Err(
                "on Windows hx can confine where tools write, but cannot hide credential \
                 files from them or limit their network access. To accept that, set \
                 `allow_partial = true` under [sandbox] in config.toml; or run with \
                 --sandbox danger-full-access, where every change needs your approval"
                    .into(),
            );
        }
        eprintln!(
            "hx: tools are confined for writes only; they can read any file you can and \
             reach the network"
        );
        policy.unreadable.clear();
        policy.hide_env_files = false;
        policy.network = Network::Open;
        hx_sandbox::prepare(&policy).map_err(|e| e.to_string())?;
    } else if policy.is_confined() {
        policy.unreadable.extend(config.sandbox.unreadable.clone());
        // The harness's own data: credentials, settings, other sessions.
        // The spill directory stays readable; tools are pointed at it.
        let home = hx_kernel::real_path(home).unwrap_or_else(|_| home.to_path_buf());
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
    let runner: Arc<dyn ToolRunner> = match &options.runner {
        Some(runner) => Arc::clone(runner),
        None => Arc::new(SandboxedRunner {
            helper: std::env::current_exe()
                .map_err(|e| format!("cannot find the hx executable: {e}"))?,
            policy: policy.clone(),
            env: granted,
        }),
    };

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
    } else if let Some(prompter) = &options.prompter {
        Arc::clone(prompter)
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

    if !config.hooks.is_empty() {
        pipeline.push(hx_ext::hooks::HookLayer {
            hooks: config.hooks.clone(),
            cwd: cwd.to_path_buf(),
        });
    }

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
        llm: summarizer,
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

    let mut tools = ToolSet::builtin();
    let mut system = system_prompt(cwd, &policy) + &load_instructions(home, cwd);

    // Extensions. One that fails to load is reported and left out; the
    // rest of the agent still works.
    let skills = hx_ext::skills::discover(&[
        cwd.join(".hx/skills"),
        cwd.join(".claude/skills"),
        home.join("skills"),
    ]);
    if !skills.is_empty() {
        system.push_str(&hx_ext::skills::prompt_section(&skills));
        tools.insert(Arc::new(hx_ext::skills::SkillTool::new(skills)));
    }
    let helper = std::env::current_exe().unwrap_or_default();
    for (name, server) in &config.mcp {
        let started = async {
            let confinement = policy.is_confined().then_some(&policy);
            let client =
                hx_ext::mcp::McpClient::start(name, server, confinement, &helper, cwd).await?;
            client.tools().await
        };
        match started.await {
            Ok(server_tools) => server_tools.into_iter().for_each(|tool| tools.insert(tool)),
            Err(e) => eprintln!("hx: {e}"),
        }
    }
    if !config.lsp.is_empty() {
        tools.insert(Arc::new(hx_ext::lsp::DiagnosticsTool::new(
            &config.lsp,
            policy.is_confined().then(|| policy.clone()),
            helper.clone(),
            cwd.to_path_buf(),
        )));
    }
    for (name, plugin) in &config.plugins {
        let loaded = hx_ext::wasm::WasmPlugin::load(name, plugin, cwd).and_then(|p| p.tools());
        match loaded {
            Ok(plugin_tools) => plugin_tools.into_iter().for_each(|tool| tools.insert(tool)),
            Err(e) => eprintln!("hx: {e}"),
        }
    }
    // Working with other agents: a shared board, a shared budget, and the
    // ability to start subagents until the depth limit.
    let lineage = options.lineage.clone().unwrap_or_else(|| Lineage {
        depth: 0,
        budget: (config.limits.tree_tokens > 0)
            .then(|| hx_multi::TreeBudget::new(config.limits.tree_tokens)),
        board: hx_multi::TaskBoard::new(
            home.join("tasks")
                .join(checkpoint_dir(home, cwd).file_name().unwrap_or_default())
                .with_extension("json"),
        ),
    });
    for tool in hx_multi::board_tools(&lineage.board) {
        tools.insert(tool);
    }
    if let Some(budget) = &lineage.budget {
        hooks.push(Arc::new(hx_multi::BudgetHook::new(Arc::clone(budget))));
    }
    if lineage.depth < config.limits.subagent_depth {
        let (child_home, parent_options, parent_lineage) =
            (home.to_path_buf(), options.clone(), lineage.clone());
        let factory: hx_multi::ChildFactory = Arc::new(move |spec: hx_multi::ChildSpec| {
            let (home, mut options) = (child_home.clone(), parent_options.clone());
            options.lineage = Some(Lineage {
                depth: spec.depth,
                ..parent_lineage.clone()
            });
            if spec.read_only {
                options.sandbox = Some(Mode::ReadOnly);
            }
            Box::pin(async move {
                let mut runtime = build(&home, &spec.cwd, &options).await?;
                if spec.read_only {
                    runtime
                        .agent
                        .tools
                        .retain(|tool| tool.effect() != Effect::Write);
                }
                Ok(hx_multi::Child {
                    agent: runtime.agent,
                    keep_alive: Box::new(runtime.proxy),
                })
            })
        });
        tools.insert(Arc::new(hx_multi::SubagentTool {
            factory,
            store: session_store(home),
            cwd: cwd.to_path_buf(),
            worktrees: home.join("worktrees"),
            depth: lineage.depth,
        }));
    }
    if options.plan {
        tools.retain(|tool| tool.effect() != Effect::Write);
        system.push_str(PLAN_MODE);
    }

    let agent = Agent {
        llm,
        tools,
        pipeline,
        tool_context: ToolContext::new(cwd),
        runner,
        checkpointer,
        hooks,
        system,
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

/// The agent's model, then the one that writes summaries.
type Models = (Arc<dyn Llm>, Arc<dyn Llm>);

fn llm(config: &Config, secrets: &Secrets, options: &Options) -> Result<Models, String> {
    // Replays recorded responses instead of calling a provider; for tests
    // and evals.
    if let Some(script) = std::env::var_os("HX_SCRIPT") {
        let delay = std::env::var("HX_SCRIPT_DELAY_MS")
            .ok()
            .and_then(|ms| ms.parse().ok())
            .unwrap_or(0);
        let recording = Recording::load(&script).map_err(|e| e.to_string())?;
        let scripted: Arc<dyn Llm> =
            Arc::new(ScriptedLlm::new(recording).with_delay(Duration::from_millis(delay)));
        return Ok((Arc::clone(&scripted), scripted));
    }
    let env = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
    let lookup = |name: &str| env(name).or_else(|| secrets.get(name).map(String::from));

    let named = options
        .provider
        .clone()
        .or_else(|| env("HX_PROVIDER"))
        .or_else(|| config.model.provider.clone());
    let provider = match named {
        Some(name) => Provider::parse(&name)?,
        // Whichever provider there is a key for.
        None => [Provider::Deepseek, Provider::Anthropic, Provider::Openai]
            .into_iter()
            .find(|provider| {
                provider
                    .key_names()
                    .iter()
                    .any(|name| lookup(name).is_some())
            })
            .unwrap_or(Provider::Deepseek),
    };

    let key_names: Vec<&str> = match &config.model.api_key_env {
        Some(name) => vec![name.as_str()],
        None => std::iter::once("HX_API_KEY")
            .chain(provider.key_names().iter().copied())
            .collect(),
    };
    let api_key = key_names
        .iter()
        .find_map(|name| lookup(name))
        .ok_or_else(|| format!("no API key: set {}", key_names.join(" or ")))?;
    let base_url = options
        .base_url
        .clone()
        .or_else(|| env("HX_BASE_URL"))
        .or_else(|| config.model.base_url.clone())
        .unwrap_or_else(|| provider.base_url().into());
    let model = options
        .model
        .clone()
        .or_else(|| env("HX_MODEL"))
        .or_else(|| config.model.name.clone())
        .or_else(|| provider.default_model().map(String::from))
        .ok_or("no model chosen: pass --model, or set HX_MODEL or [model] name")?;
    let summary_model = config.model.summary_model.clone().unwrap_or(model.clone());

    // OpenAI's own API is spoken through Responses; a compatible server at
    // another address is assumed to offer only chat completions unless the
    // configuration says otherwise.
    let chat_only = match config.model.api.as_deref() {
        Some("chat-completions") => true,
        Some("responses") => false,
        Some(other) => {
            return Err(format!(
                "unknown [model] api {other:?}: use responses or chat-completions"
            ));
        }
        None => base_url != provider.base_url(),
    };
    let build = |model: String| -> Arc<dyn Llm> {
        match provider {
            Provider::Anthropic => Arc::new(
                hx_llm::Anthropic::new(&base_url, api_key.clone(), model).with_effort(Some(
                    config.model.effort.clone().unwrap_or_else(|| "high".into()),
                )),
            ),
            Provider::Openai if !chat_only => Arc::new(
                hx_llm::Responses::new(&base_url, api_key.clone(), model)
                    .with_effort(config.model.effort.clone()),
            ),
            Provider::Deepseek | Provider::Openai => {
                Arc::new(ChatCompletions::new(&base_url, api_key.clone(), model))
            }
        }
    };
    Ok((build(model), build(summary_model)))
}

/// Gives every question the same answer.
struct Always(Option<bool>);

impl Prompter for Always {
    fn ask<'a>(
        &'a self,
        _request: &'a ToolRequest,
        _question: String,
    ) -> BoxFuture<'a, Option<bool>> {
        Box::pin(async move { self.0 })
    }
}

/// Asks on the controlling terminal.
struct Terminal;

impl Prompter for Terminal {
    fn ask<'a>(
        &'a self,
        _request: &'a ToolRequest,
        question: String,
    ) -> BoxFuture<'a, Option<bool>> {
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

pub fn session_store(home: &Path) -> SessionStore {
    SessionStore::new(home.join("sessions"))
}

/// Opens `existing`, or creates a session rooted at `cwd`.
pub fn open_session(
    store: &SessionStore,
    existing: Option<&SessionId>,
    cwd: &Path,
) -> Result<Session, String> {
    let (records, writer) = match existing {
        Some(id) => store.open(id).map_err(|e| e.to_string())?,
        None => {
            let writer = store
                .create(CreateOptions {
                    id: None,
                    cwd: cwd.to_string_lossy().into_owned(),
                })
                .map_err(|e| e.to_string())?;
            (store.read(writer.id()).map_err(|e| e.to_string())?, writer)
        }
    };
    Ok(Session::new(records, writer))
}

/// The working directory a session was created in.
pub fn session_cwd(store: &SessionStore, id: &SessionId) -> Result<PathBuf, String> {
    let records = store.read(id).map_err(|e| e.to_string())?;
    match records.first().map(|record| &record.event) {
        Some(Event::Session(header)) => Ok(PathBuf::from(&header.cwd)),
        _ => Err(format!("session {id} has no header")),
    }
}

/// The result of [`rewind`].
pub struct Rewound {
    /// The new session, which ends just before the undone turn.
    pub session: SessionId,
    pub turn: u64,
    pub restored: Restored,
}

/// Undoes `turn` (default: the last one) and everything after it: restores
/// the workspace to the checkpoint taken as that turn began, and forks the
/// session just before it. The original session is left as it was.
pub async fn rewind(
    home: &Path,
    store: &SessionStore,
    id: &SessionId,
    turn: Option<u64>,
) -> Result<Rewound, String> {
    let records = store.read(id).map_err(|e| e.to_string())?;
    let turn = match turn {
        Some(turn) => turn,
        None => records
            .iter()
            .filter_map(|record| match record.event {
                Event::TurnStart { turn } => Some(turn),
                _ => None,
            })
            .max()
            .ok_or("the session has no turns")?,
    };
    let start = records
        .iter()
        .position(|record| record.event == Event::TurnStart { turn })
        .ok_or(format!("the session has no turn {turn}"))?;
    let commit = records[start..]
        .iter()
        .take_while(|record| !matches!(record.event, Event::TurnEnd { .. }))
        .find_map(|record| match &record.event {
            Event::Checkpoint { commit } => Some(commit.clone()),
            _ => None,
        })
        .ok_or(format!("turn {turn} has no checkpoint to rewind to"))?;
    let cwd = session_cwd(store, id)?;
    let restored = GitCheckpoints::new(checkpoint_dir(home, &cwd), &cwd)
        .restore(&commit)
        .await?;
    let options = CreateOptions {
        id: None,
        cwd: cwd.to_string_lossy().into_owned(),
    };
    let (_records, writer) = store
        .fork(id, records[start].seq - 1, options)
        .map_err(|e| e.to_string())?;
    Ok(Rewound {
        session: writer.id().clone(),
        turn,
        restored,
    })
}
