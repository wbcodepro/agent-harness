//! Confinement for everything a tool does.
//!
//! Tools do not run in the harness process. Each call is shipped to a
//! short-lived worker process ([`SandboxedRunner`]) that the operating
//! system confines according to a [`SandboxPolicy`]: where it may write,
//! what it may not read, and whether it may reach the network. Commands the
//! tool starts inherit the confinement. If the platform cannot enforce the
//! policy, the call is refused rather than run unconfined.

mod intercept;
mod policy;
mod proxy;
mod worker;

#[cfg(target_os = "linux")]
mod landlock;
#[cfg(target_os = "macos")]
mod seatbelt;
#[cfg(windows)]
mod windows;

pub use intercept::Substitution;
pub use policy::{Mode, Network, SandboxError, SandboxPolicy, Wrapped, default_unreadable, wrap};
pub use proxy::{Allowlist, Decision, EgressProxy};
pub use worker::{SandboxedRunner, worker_main};

#[cfg(target_os = "linux")]
pub use landlock::apply_to_current_process;
#[cfg(windows)]
pub use windows::{apply_to_current_process, keep_stdio_private, low_temp_dir};

/// Gets the machine ready to enforce `policy`, before any tool runs. Only
/// Windows needs this: its writable roots must be labelled once.
pub fn prepare(policy: &SandboxPolicy) -> Result<(), SandboxError> {
    #[cfg(windows)]
    return windows::prepare(policy);
    #[cfg(not(windows))]
    {
        let _ = policy;
        Ok(())
    }
}
