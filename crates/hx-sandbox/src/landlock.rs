use std::path::{Path, PathBuf};

use landlock::{
    ABI, Access, AccessFs, AccessNet, NetPort, PathBeneath, PathFd, Ruleset, RulesetAttr,
    RulesetCreatedAttr, RulesetStatus,
};

use crate::policy::{Mode, Network, SandboxError, SandboxPolicy, real};

/// Confines the calling process, and everything it later executes, with
/// Landlock. Called by the `hx sandbox-exec` helper right before it executes
/// the target program.
///
/// Landlock can only grant access beneath paths, never take a subpath away,
/// so "unreadable" is enforced by granting read access to everything except
/// the top-level entries that contain an unreadable path, then descending
/// into those. Files matching `hide_env_files` cannot be expressed and stay
/// readable on Linux.
///
/// Network: with `Proxy`, TCP connections are limited to the proxy's port
/// number (on any address). UDP is not restricted by Landlock.
pub fn apply_to_current_process(policy: &SandboxPolicy) -> Result<(), SandboxError> {
    if !policy.is_confined() {
        return Ok(());
    }
    let unavailable = |what: &str, e: &dyn std::fmt::Display| {
        SandboxError::Unavailable(format!("landlock: {what}: {e}"))
    };
    let abi = ABI::V4;
    let read = AccessFs::from_read(abi);
    let all = AccessFs::from_all(abi);

    let mut ruleset = Ruleset::default()
        .handle_access(all)
        .map_err(|e| unavailable("cannot handle filesystem access", &e))?;
    if policy.network != Network::Open {
        ruleset = ruleset
            .handle_access(AccessNet::BindTcp | AccessNet::ConnectTcp)
            .map_err(|e| unavailable("cannot handle network access", &e))?;
    }
    let mut created = ruleset
        .create()
        .map_err(|e| unavailable("cannot create the ruleset", &e))?;

    // Rights that only make sense for directories must not be requested on
    // a file: the kernel interface rejects them, and a rule that had to be
    // trimmed would make the whole policy count as partially enforced.
    let fit = |path: &Path, wanted| {
        if path.is_dir() {
            wanted
        } else {
            wanted & AccessFs::from_file(abi)
        }
    };

    let unreadable: Vec<PathBuf> = policy.unreadable.iter().map(|p| real(p)).collect();
    for path in readable_roots(Path::new("/"), &unreadable) {
        if let Ok(fd) = PathFd::new(&path) {
            created = created
                .add_rule(PathBeneath::new(fd, fit(&path, read)))
                .map_err(|e| unavailable("cannot add a read rule", &e))?;
        }
    }
    if policy.mode == Mode::WorkspaceWrite {
        for path in &policy.writable {
            let path = real(path);
            if let Ok(fd) = PathFd::new(&path) {
                created = created
                    .add_rule(PathBeneath::new(fd, fit(&path, all)))
                    .map_err(|e| unavailable("cannot add a write rule", &e))?;
            }
        }
    }
    for device in ["/dev/null", "/dev/zero", "/dev/tty", "/dev/urandom"] {
        if let Ok(fd) = PathFd::new(device) {
            created = created
                .add_rule(PathBeneath::new(fd, AccessFs::from_file(abi)))
                .map_err(|e| unavailable("cannot add a device rule", &e))?;
        }
    }
    if let Network::Proxy { port } = policy.network {
        created = created
            .add_rule(NetPort::new(port, AccessNet::ConnectTcp))
            .map_err(|e| unavailable("cannot add the proxy rule", &e))?;
    }

    let status = created
        .restrict_self()
        .map_err(|e| unavailable("cannot restrict the process", &e))?;
    match status.ruleset {
        RulesetStatus::FullyEnforced => Ok(()),
        // An older kernel that silently drops part of the policy is not
        // confinement we can vouch for.
        RulesetStatus::PartiallyEnforced => Err(SandboxError::Unavailable(
            "landlock: this kernel enforces only part of the policy (needs Landlock ABI 4, Linux 6.7+)"
                .into(),
        )),
        RulesetStatus::NotEnforced => Err(SandboxError::Unavailable(
            "landlock: the kernel did not apply the policy (Landlock is disabled, or blocked by a seccomp filter such as a container runtime's)"
                .into(),
        )),
    }
}

/// Every path under `dir` that can be granted for reading without exposing
/// any of `unreadable`: whole entries where possible, descending only into
/// the ones that contain something unreadable.
fn readable_roots(dir: &Path, unreadable: &[PathBuf]) -> Vec<PathBuf> {
    if !unreadable.iter().any(|hidden| hidden.starts_with(dir)) {
        return vec![dir.to_path_buf()];
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut roots = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if unreadable.contains(&path) {
            continue;
        }
        // Do not follow symlinks while descending; grant the link's entry
        // itself only when nothing hidden lies textually beneath it.
        let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
        if is_dir {
            roots.extend(readable_roots(&path, unreadable));
        } else if !unreadable.iter().any(|hidden| hidden.starts_with(&path)) {
            roots.push(path);
        }
    }
    roots
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readable_roots_cover_everything_but_the_hidden_paths() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        for d in ["home/u/.ssh", "home/u/code", "home/v", "usr/bin"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::write(root.join("home/u/.netrc"), "x").unwrap();
        std::fs::write(root.join("home/u/notes"), "x").unwrap();

        let hidden = vec![root.join("home/u/.ssh"), root.join("home/u/.netrc")];
        let mut roots = readable_roots(&root, &hidden);
        roots.sort();
        assert_eq!(
            roots,
            vec![
                root.join("home/u/code"),
                root.join("home/u/notes"),
                root.join("home/v"),
                root.join("usr"),
            ]
        );
        assert_eq!(readable_roots(&root, &[]), vec![root]);
    }
}
