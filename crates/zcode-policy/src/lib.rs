//! What the user allows: configuration, permission rules, approval, and
//! keeping secrets out of the model's context.

mod config;
mod gitguard;
mod layers;
mod secrets;
mod shell;

pub use config::{
    ApprovalConfig, CheckpointConfig, Config, ConfigError, ContextConfig, EditFormat, LimitsConfig,
    ModelConfig, NetworkConfig, NetworkMode, ProjectConfig, Rule, RuleAction, SandboxConfig,
    SecretsConfig, ToolProfile, VerifyConfig,
};
pub use gitguard::{GIT_GUARD_PREFIX, GitGuard};
pub use layers::{ApprovalLayer, Prompter, RedactLayer, glob_match};
pub use secrets::Secrets;
pub use shell::shell_commands;
