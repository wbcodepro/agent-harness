use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    /// Nothing can be written.
    ReadOnly,
    /// Only the listed roots can be written.
    WorkspaceWrite,
    /// No confinement at all.
    DangerFullAccess,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Network {
    Blocked,
    /// Only this machine is reachable; everything else must go through the
    /// egress proxy listening on `port`.
    Proxy {
        port: u16,
    },
    Open,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxPolicy {
    pub mode: Mode,
    /// Directories that can be written in `WorkspaceWrite` mode.
    pub writable: Vec<PathBuf>,
    /// Paths that cannot be read (or written) in any confined mode.
    pub unreadable: Vec<PathBuf>,
    /// Also hide files named `.env` or `.env.*`, wherever they are.
    pub hide_env_files: bool,
    pub network: Network,
}

impl SandboxPolicy {
    /// Writes confined to `cwd` and the temp directories, well-known
    /// credential locations hidden, no network.
    pub fn workspace(cwd: &Path) -> Self {
        let mut writable = vec![real(cwd)];
        for tmp in [std::env::temp_dir(), PathBuf::from("/tmp")] {
            let tmp = real(&tmp);
            if !writable.contains(&tmp) {
                writable.push(tmp);
            }
        }
        Self {
            mode: Mode::WorkspaceWrite,
            writable,
            unreadable: std::env::home_dir()
                .map(|home| default_unreadable(&home))
                .unwrap_or_default(),
            hide_env_files: true,
            network: Network::Blocked,
        }
    }

    pub fn read_only() -> Self {
        Self {
            mode: Mode::ReadOnly,
            writable: Vec::new(),
            ..Self::workspace(Path::new("/nonexistent"))
        }
    }

    pub fn full_access() -> Self {
        Self {
            mode: Mode::DangerFullAccess,
            writable: Vec::new(),
            unreadable: Vec::new(),
            hide_env_files: false,
            network: Network::Open,
        }
    }

    pub fn is_confined(&self) -> bool {
        self.mode != Mode::DangerFullAccess
    }
}

/// Where credentials usually live under a home directory.
pub fn default_unreadable(home: &Path) -> Vec<PathBuf> {
    [
        ".ssh",
        ".aws",
        ".azure",
        ".gnupg",
        ".kube",
        ".docker",
        ".netrc",
        ".npmrc",
        ".pypirc",
        ".git-credentials",
        ".config/gh",
        ".config/gcloud",
        ".cargo/credentials.toml",
        ".hx",
    ]
    .iter()
    .map(|name| real(&home.join(name)))
    .collect()
}

/// The path with symlinks resolved, when it exists. Platform rules match
/// real paths, and e.g. `/tmp` is a symlink on macOS.
pub(crate) fn real(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error("no sandbox is available to enforce the policy: {0}")]
    Unavailable(String),
}

/// A command line that runs the original program under confinement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wrapped {
    pub program: PathBuf,
    pub args: Vec<OsString>,
}

/// Rewrites `program args...` so that it runs confined by `policy`.
///
/// `helper` is the `hx` executable; on Linux it re-executes itself to apply
/// the policy before starting the program. Fails when the platform has no
/// way to enforce a confined policy: nothing ever silently runs unconfined.
pub fn wrap(
    policy: &SandboxPolicy,
    helper: &Path,
    program: &Path,
    args: &[OsString],
) -> Result<Wrapped, SandboxError> {
    if !policy.is_confined() {
        return Ok(Wrapped {
            program: program.to_path_buf(),
            args: args.to_vec(),
        });
    }
    platform_wrap(policy, helper, program, args)
}

#[cfg(target_os = "macos")]
fn platform_wrap(
    policy: &SandboxPolicy,
    _helper: &Path,
    program: &Path,
    args: &[OsString],
) -> Result<Wrapped, SandboxError> {
    let launcher = Path::new("/usr/bin/sandbox-exec");
    if !launcher.exists() {
        return Err(SandboxError::Unavailable(
            "/usr/bin/sandbox-exec is missing".into(),
        ));
    }
    let mut wrapped = vec![
        OsString::from("-p"),
        OsString::from(crate::seatbelt::profile(policy)),
        OsString::from("--"),
        program.as_os_str().to_os_string(),
    ];
    wrapped.extend_from_slice(args);
    Ok(Wrapped {
        program: launcher.to_path_buf(),
        args: wrapped,
    })
}

#[cfg(target_os = "linux")]
fn platform_wrap(
    policy: &SandboxPolicy,
    helper: &Path,
    program: &Path,
    args: &[OsString],
) -> Result<Wrapped, SandboxError> {
    let encoded = serde_json::to_string(policy)
        .map_err(|e| SandboxError::Unavailable(format!("cannot encode the policy: {e}")))?;
    let mut wrapped = vec![
        OsString::from("sandbox-exec"),
        OsString::from("--policy"),
        OsString::from(encoded),
        OsString::from("--"),
        program.as_os_str().to_os_string(),
    ];
    wrapped.extend_from_slice(args);
    Ok(Wrapped {
        program: helper.to_path_buf(),
        args: wrapped,
    })
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn platform_wrap(
    _policy: &SandboxPolicy,
    _helper: &Path,
    _program: &Path,
    _args: &[OsString],
) -> Result<Wrapped, SandboxError> {
    Err(SandboxError::Unavailable(
        "this platform has no sandbox backend yet".into(),
    ))
}
