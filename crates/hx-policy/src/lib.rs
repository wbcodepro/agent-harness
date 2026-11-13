//! What the user allows: configuration, permission rules, approval, and
//! keeping secrets out of the model's context.

mod config;
mod layers;
mod secrets;

pub use config::{
    ApprovalConfig, CheckpointConfig, Config, ConfigError, ModelConfig, NetworkConfig, NetworkMode,
    Rule, RuleAction, SandboxConfig, SecretsConfig,
};
pub use layers::{ApprovalLayer, Prompter, RedactLayer, glob_match};
pub use secrets::Secrets;
