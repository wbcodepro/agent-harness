use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use hx_ext::hooks::Hook;
use hx_ext::lsp::LspServerConfig;
use hx_ext::mcp::McpServerConfig;
use hx_ext::wasm::PluginConfig;

use hx_sandbox::Mode;
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

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
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
    /// A cheaper model, on the same provider, for summarizing history.
    pub summary_model: Option<String>,
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
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            mode: Mode::WorkspaceWrite,
            writable: Vec::new(),
            writable_tmp: true,
            unreadable: Vec::new(),
            hide_env_files: true,
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
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct SecretsConfig {
    /// Names of secrets passed into the tool environment. Everything else
    /// the harness knows stays out of reach of tools.
    pub grant: Vec<String>,
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
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            compact_at_tokens: 100_000,
            spill_at_bytes: 16_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct VerifyConfig {
    /// Commands that must succeed before a turn that changed files ends.
    pub commands: Vec<String>,
    /// Times per turn a failure is sent back to the model.
    pub max_rounds: usize,
    pub timeout_ms: u64,
}

impl Default for VerifyConfig {
    fn default() -> Self {
        Self {
            commands: Vec::new(),
            max_rounds: 2,
            timeout_ms: 300_000,
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

/// What a project may configure for itself in `.hx/config.toml`. A
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
