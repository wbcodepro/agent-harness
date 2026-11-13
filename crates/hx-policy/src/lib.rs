//! What the user allows: configuration, permission rules, approval, and
//! keeping secrets out of the model's context.

mod config;
mod layers;
mod secrets;

pub use config::{
    ApprovalConfig, CheckpointConfig, Config, ConfigError, ContextConfig, LimitsConfig,
    ModelConfig, NetworkConfig, NetworkMode, ProjectConfig, Rule, RuleAction, SandboxConfig,
    SecretsConfig, VerifyConfig,
};
pub use layers::{ApprovalLayer, Prompter, RedactLayer, glob_match};
pub use secrets::Secrets;
