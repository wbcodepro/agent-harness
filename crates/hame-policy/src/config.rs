use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use hame_ext::hooks::Hook;
use hame_ext::lsp::LspServerConfig;
use hame_ext::mcp::McpServerConfig;
use hame_ext::wasm::PluginConfig;

use hame_sandbox::Mode;
use serde::Deserialize;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path} is not valid configuration: {reason}")]
    Invalid { path: PathBuf, reason: String },
}

/// The user's `config.toml`. Every table and key is optional; unknown keys
/// are rejected so a typo cannot silently fall back to a default.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    pub model: ModelConfig,
    pub sandbox: SandboxConfig,
    pub network: NetworkConfig,
    pub approval: ApprovalConfig,
    pub secrets: SecretsConfig,
    pub checkpoints: CheckpointConfig,
    pub context: ContextConfig,
    pub verify: VerifyConfig,
    pub limits: LimitsConfig,
    pub compat: CompatConfig,
    /// Permission rules, first match wins.
    #[serde(rename = "rule")]
    pub rules: Vec<Rule>,
    /// Model Context Protocol servers, by name.
    pub mcp: BTreeMap<String, McpServerConfig>,
    /// Language servers for the `diagnostics` tool, by name.
    pub lsp: BTreeMap<String, LspServerConfig>,
    /// Commands run before and after tool calls.
    #[serde(rename = "hook")]
    pub hooks: Vec<Hook>,
    /// WebAssembly plugins, by name.
    #[serde(rename = "plugin")]
    pub plugins: BTreeMap<String, PluginConfig>,
}

impl Config {
    /// Loads `path`, or the defaults when the file does not exist.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => {
                return Err(ConfigError::Read {
                    path: path.to_path_buf(),
                    source,
                });
            }
        };
        Self::parse(&text).map_err(|reason| ConfigError::Invalid {
            path: path.to_path_buf(),
            reason,
        })
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ModelConfig {
    /// `deepseek`, `openai`, `anthropic`, or empty to pick by available key.
    pub provider: Option<String>,
    pub name: Option<String>,
    pub base_url: Option<String>,
    /// Name of the environment variable or secret holding the API key.
    pub api_key_env: Option<String>,
    /// Reasoning effort for providers that take one: `low`, `medium`,
    /// `high`, `xhigh` or `max`. Empty leaves the model's default.
    pub effort: Option<String>,
    /// Wire protocol for the `openai` provider: `responses` (default) or
    /// `chat-completions` for OpenAI-compatible servers that lack it.
    pub api: Option<String>,
    /// A cheaper model, on the same provider, for summarizing history.
    pub summary_model: Option<String>,
    /// Sampling temperature, for servers spoken to through chat
    /// completions (DeepSeek, compatible and local servers). Small models
    /// often follow tool formats more reliably near 0. Empty leaves the
    /// server's default.
    pub temperature: Option<f32>,
    /// Most tool calls run from one model response.
    pub max_tool_calls: usize,
    /// Let `edit`, `write` and `apply_patch` take a `then_run` command that
    /// runs right after the change, so a change and its check cost one
    /// request instead of two.
    pub fuse_actions: bool,
    /// How many tools the model is offered.
    pub tools: ToolProfile,
    /// How the model changes files.
    pub edit_format: EditFormat,
    /// A stronger model, on the same provider, that takes over the rest of
    /// a turn in which this one keeps failing.
    pub escalate_to: Option<String>,
    /// The provider of the stronger model, when it is not this one's:
    /// `deepseek`, `openai` or `anthropic`, reached at its own address with
    /// its own key. This is how a small local model hands over to a hosted
    /// one. Without `escalate_to`, the provider's default model is used.
    pub escalate_provider: Option<String>,
    /// Reasoning effort for the stronger model. Empty keeps `effort`. Set
    /// without `escalate_to`, it is the same model that thinks harder once
    /// a turn is stuck, so `effort` can be set low for everyday work.
    pub escalate_effort: Option<String>,
    /// How many failed checks or repeated calls in one turn it takes.
    pub escalate_after: usize,
}

/// How many tools the model is offered. Small models choose better among
/// few.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToolProfile {
    #[default]
    Full,
    /// Only reading, changing and searching files and running commands,
    /// plus tools the configuration adds by name (MCP servers, plugins).
    Minimal,
}

/// How the model changes files. Which one a model handles best differs by
/// model family.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EditFormat {
    /// `edit` replaces exact text; `write` creates files.
    #[default]
    SearchReplace,
    /// No `edit`: the model rewrites a file in full with `write`. Costs
    /// more output but cannot fail to match.
    WholeFile,
    /// `apply_patch` replaces both `edit` and `write`.
    ApplyPatch,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            provider: None,
            name: None,
            base_url: None,
            api_key_env: None,
            effort: None,
            api: None,
            summary_model: None,
            temperature: None,
            max_tool_calls: 32,
            fuse_actions: true,
            tools: ToolProfile::Full,
            edit_format: EditFormat::SearchReplace,
            escalate_to: None,
            escalate_provider: None,
            escalate_effort: None,
            escalate_after: 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct SandboxConfig {
    pub mode: Mode,
    /// Extra directories tools may write to.
    pub writable: Vec<PathBuf>,
    /// Whether the system temp directories are writable.
    pub writable_tmp: bool,
    /// Extra paths tools may not read.
    pub unreadable: Vec<PathBuf>,
    /// Hide `.env` files from tools.
    pub hide_env_files: bool,
    /// Undo changes tools make to `.git/hooks` and `.git/config`.
    pub protect_git: bool,
    /// Windows only: accept confinement of writes alone. Windows cannot
    /// hide files from tools or limit their network access, so without
    /// this the confined modes refuse to start there.
    pub allow_partial: bool,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            mode: Mode::WorkspaceWrite,
            writable: Vec::new(),
            writable_tmp: true,
            unreadable: Vec::new(),
            hide_env_files: true,
            protect_git: true,
            allow_partial: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NetworkMode {
    Off,
    /// Only hosts in `allow`, through the egress proxy.
    Allowlist,
    Open,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct NetworkConfig {
    pub mode: NetworkMode,
    pub allow: Vec<String>,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            mode: NetworkMode::Allowlist,
            allow: [
                "github.com",
                "*.github.com",
                "*.githubusercontent.com",
                "crates.io",
                "*.crates.io",
                "pypi.org",
                "files.pythonhosted.org",
                "registry.npmjs.org",
                "proxy.golang.org",
                "sum.golang.org",
            ]
            .map(String::from)
            .to_vec(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuleAction {
    Allow,
    Ask,
    Deny,
}

/// `tool` is a tool name or `*`. `pattern`, when present, is matched against
/// the call's subject (the command for `bash`, the path for file tools) with
/// `*` standing for any run of characters.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub tool: String,
    pub pattern: Option<String>,
    pub action: RuleAction,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ApprovalConfig {
    /// What to do with a call no rule matches. Defaults to allowing calls
    /// that run confined, and asking for ones that do not.
    pub default: Option<RuleAction>,
    /// Let a model answer approval questions first: it approves calls that
    /// are plainly harmless, and everything else still goes to the user.
    /// It uses the summary model. Its verdicts are logged in
    /// `approvals.jsonl` in the data directory.
    pub reviewer: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct SecretsConfig {
    /// Names of secrets passed into the tool environment. Everything else
    /// the harness knows stays out of reach of tools.
    pub grant: Vec<String>,
    /// Secrets tools may use without holding them: the name of a secret
    /// and the hosts it may be sent to. Tools get a placeholder in the
    /// variable of that name; the egress proxy puts the real value in its
    /// place in requests to those hosts, and nowhere else. Needs the
    /// network in `allowlist` mode.
    pub substitute: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct CheckpointConfig {
    pub enabled: bool,
}

impl Default for CheckpointConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ContextConfig {
    /// Summarize older history once a request would exceed this many tokens.
    pub compact_at_tokens: u64,
    /// Tool outputs longer than this are saved to a file and previewed.
    pub spill_at_bytes: usize,
    /// Leave the lines of passing tests and build progress out of command
    /// output shown to the model; the whole output is saved to a file.
    pub condense_output: bool,
    /// With more MCP and plugin tools than this, none of them is declared
    /// to the model: it finds them with `tool_search` and runs them with
    /// `tool_call`, so they cost no prompt tokens until used.
    pub defer_tools_over: usize,
    /// When tools are deferred, also offer `script`: a Python program that
    /// calls them as functions, so a chain of calls takes one step and
    /// only its printed result enters the context.
    pub scripts: bool,
    /// Past the threshold, first clear old tool results from what the model
    /// sees, and summarize only if that is not enough. Off by default: on
    /// the one task it was measured on, a model that had not noted what it
    /// read went back and read it again, and it cost more than summarizing.
    pub clear_tool_results: bool,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            compact_at_tokens: 100_000,
            spill_at_bytes: 16_000,
            condense_output: true,
            defer_tools_over: 20,
            scripts: true,
            clear_tool_results: false,
        }
    }
}

/// Reading other tools' configuration.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct CompatConfig {
    /// Which Claude Code settings files to run hooks from. Empty (the
    /// default) runs none: hooks are commands that run outside the sandbox.
    pub claude_hooks: Vec<HookSource>,
    /// Directories of plugins packaged for Claude Code. Their skills, hooks
    /// and MCP servers are loaded; list only plugins you trust, since
    /// their hooks run outside the sandbox.
    pub claude_plugins: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HookSource {
    /// `~/.claude/settings.json`: the user's own hooks.
    User,
    /// `.claude/settings.json` and `.claude/settings.local.json` in the
    /// working directory. These come with the repository: turn this on
    /// only for repositories whose hooks you would run by hand.
    Project,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct VerifyConfig {
    /// Commands that must succeed before a turn that changed files ends.
    pub commands: Vec<String>,
    /// Times per turn a failure is sent back to the model.
    pub max_rounds: usize,
    pub timeout_ms: u64,
    /// Before a turn that changed files ends, ask the model once to go
    /// through the request point by point and confirm each is done.
    pub checklist: bool,
    /// Send the model back to work, at most twice a turn, when the last
    /// test, build or lint command it ran failed and it is stopping.
    pub stop_on_red: bool,
}

impl Default for VerifyConfig {
    fn default() -> Self {
        Self {
            commands: Vec::new(),
            max_rounds: 2,
            timeout_ms: 300_000,
            checklist: false,
            stop_on_red: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct LimitsConfig {
    /// Tokens an agent and all its subagents may spend on one run of the
    /// harness. Zero means no limit.
    pub tree_tokens: u64,
    /// How many levels of subagents may be nested. Zero turns subagents off.
    pub subagent_depth: u32,
}

impl Default for LimitsConfig {
    fn default() -> Self {
        Self {
            tree_tokens: 0,
            subagent_depth: 1,
        }
    }
}

/// What a project may configure for itself in `.hame/config.toml`. A
/// repository is not trusted to loosen the sandbox or permissions, so this
/// is deliberately a small subset of [`Config`].
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ProjectConfig {
    pub verify: Option<VerifyConfig>,
}

impl ProjectConfig {
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => {
                return Err(ConfigError::Read {
                    path: path.to_path_buf(),
                    source,
                });
            }
        };
        toml::from_str(&text).map_err(|e| ConfigError::Invalid {
            path: path.to_path_buf(),
            reason: e.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_file_gives_safe_defaults() {
        let config = Config::parse("").unwrap();
        assert_eq!(config.sandbox.mode, Mode::WorkspaceWrite);
        assert_eq!(config.network.mode, NetworkMode::Allowlist);
        assert!(config.sandbox.hide_env_files && config.checkpoints.enabled);
        assert!(config.secrets.grant.is_empty() && config.rules.is_empty());
        assert_eq!(
            Config::load(Path::new("/nonexistent/config.toml")).unwrap(),
            config
        );
    }

    #[test]
    fn settings_parse_and_typos_are_errors() {
        let config = Config::parse(
            r#"
            [model]
            provider = "openai"
            name = "some-model"
            tools = "minimal"
            edit_format = "apply-patch"

            [sandbox]
            mode = "read-only"
            writable_tmp = false

            [network]
            mode = "off"

            [secrets]
            grant = ["GITHUB_TOKEN"]

            [[rule]]
            tool = "bash"
            pattern = "git push*"
            action = "ask"

            [[rule]]
            tool = "*"
            action = "allow"
            "#,
        )
        .unwrap();
        assert_eq!(config.model.provider.as_deref(), Some("openai"));
        assert_eq!(config.model.tools, ToolProfile::Minimal);
        assert_eq!(config.model.edit_format, EditFormat::ApplyPatch);
        assert!(Config::parse("[model]\nedit_format = \"diff\"").is_err());
        assert_eq!(config.sandbox.mode, Mode::ReadOnly);
        assert!(!config.sandbox.writable_tmp);
        assert_eq!(config.network.mode, NetworkMode::Off);
        assert_eq!(config.rules.len(), 2);
        assert_eq!(config.rules[0].action, RuleAction::Ask);

        // A project file may set its checks and nothing else.
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("config.toml");
        std::fs::write(&project, "[verify]\ncommands = [\"make test\"]\n").unwrap();
        let loaded = ProjectConfig::load(&project).unwrap();
        assert_eq!(loaded.verify.unwrap().commands, ["make test"]);
        std::fs::write(&project, "[sandbox]\nmode = \"danger-full-access\"\n").unwrap();
        assert!(ProjectConfig::load(&project).is_err());

        let extended = Config::parse(
            r#"
            [mcp.docs]
            command = "docs-server"
            args = ["--stdio"]
            env = { DOCS_TOKEN = "t" }

            [[hook]]
            event = "pre_tool"
            tool = "bash"
            command = "./check.sh"

            [plugin.wc]
            path = "wc.wasm"
            read = ["docs"]
            "#,
        )
        .unwrap();
        assert!(
            extended.mcp["docs"].sandbox,
            "servers are confined by default"
        );
        assert_eq!(extended.hooks.len(), 1);
        assert_eq!(extended.plugins["wc"].read, [PathBuf::from("docs")]);
        assert!(Config::parse("[mcp.x]\ncomand = \"y\"").is_err());

        for bad in [
            "[sandbox]\nmod = \"read-only\"",
            "[sandbox]\nmode = \"yolo\"",
            "[sandox]\nmode = \"read-only\"",
            "[[rule]]\ntool = \"bash\"",
        ] {
            assert!(Config::parse(bad).is_err(), "{bad}");
        }
    }
}
