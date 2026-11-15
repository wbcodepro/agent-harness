//! What the user allows: configuration, permission rules, approval, and
//! keeping secrets out of the model's context.

mod config;
mod gitguard;
mod layers;
mod readfirst;
mod secrets;
mod shell;

pub use config::{
    ApprovalConfig, CheckpointConfig, CompatConfig, ComputerConfig, Config, ConfigError,
    ContextConfig, EditFormat, HookSource, LimitsConfig, ModelConfig, NetworkConfig, NetworkMode,
    Observation, ProjectConfig, Rule, RuleAction, SandboxConfig, SecretsConfig, ToolProfile,
    VerifyConfig,
};
pub use gitguard::{GIT_GUARD_PREFIX, GitGuard};
pub use layers::{ApprovalLayer, AskControl, DENIED_BY_USER, Prompter, RedactLayer, glob_match};
pub use readfirst::ReadFirst;
pub use secrets::Secrets;
pub use shell::shell_commands;
