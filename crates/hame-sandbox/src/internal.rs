//! The internal commands hame runs its own executable for: `tool-worker`
//! (one confined tool call) and, on Linux and Windows, `sandbox-exec` (a
//! command under a sandbox policy). Every hame binary that runs agents (the
//! `hame` command, the desktop app) answers them with these.

use std::ffi::OsString;

/// Body of `<exe> tool-worker`: runs one tool call read from standard
/// input with every built-in tool. Returns the exit code; on Windows it
/// exits the process itself, since a program the tool left running still
/// holds the pipes its output was read from.
pub async fn tool_worker() -> u8 {
    let code = crate::worker_main(&hame_tools::ToolSet::every()).await;
    #[cfg(windows)]
    std::process::exit(code.into());
    #[cfg(not(windows))]
    code
}

/// Reads the arguments after `sandbox-exec`: `--policy <json> -- <program>
/// [args…]`.
pub fn parse_sandbox_exec(args: Vec<OsString>) -> Result<(String, Vec<OsString>), String> {
    let mut args = args.into_iter();
    if args.next().as_deref() != Some("--policy".as_ref()) {
        return Err("sandbox-exec: expected --policy".into());
    }
    let policy = args
        .next()
        .and_then(|policy| policy.into_string().ok())
        .ok_or("sandbox-exec: --policy needs a value")?;
    if args.next().as_deref() != Some("--".as_ref()) {
        return Err("sandbox-exec: expected -- before the command".into());
    }
    let command: Vec<OsString> = args.collect();
    if command.is_empty() {
        return Err("sandbox-exec: no command".into());
    }
    Ok((policy, command))
}

/// Body of `<exe> sandbox-exec`: applies `policy` (JSON) to this process,
/// then runs `command` (on Linux in place of this process). Returns the
/// command's exit code where it does not replace the process.
#[cfg(target_os = "linux")]
pub fn sandbox_exec(policy: &str, command: &[OsString]) -> Result<u8, String> {
    use std::os::unix::process::CommandExt;
    let policy: crate::SandboxPolicy =
        serde_json::from_str(policy).map_err(|e| format!("invalid sandbox policy: {e}"))?;
    crate::apply_to_current_process(&policy).map_err(|e| e.to_string())?;
    let error = std::process::Command::new(&command[0])
        .args(&command[1..])
        .exec();
    Err(format!(
        "cannot execute {}: {error}",
        command[0].to_string_lossy()
    ))
}

/// Windows has no `exec`: lower this process, run the program as a child
/// (which inherits the lowered level), and pass its exit status on.
#[cfg(windows)]
pub fn sandbox_exec(policy: &str, command: &[OsString]) -> Result<u8, String> {
    let policy: crate::SandboxPolicy =
        serde_json::from_str(policy).map_err(|e| format!("invalid sandbox policy: {e}"))?;
    crate::apply_to_current_process(&policy).map_err(|e| e.to_string())?;
    // The program gets copies of the standard handles; the originals must
    // not travel along as well, or what it starts holds the caller's pipes.
    crate::keep_stdio_private();
    let status = std::process::Command::new(&command[0])
        .args(&command[1..])
        .status()
        .map_err(|e| format!("cannot execute {}: {e}", command[0].to_string_lossy()))?;
    Ok(status.code().unwrap_or(1).clamp(0, 255) as u8)
}

/// macOS confines with the system's `sandbox-exec` instead.
#[cfg(not(any(target_os = "linux", windows)))]
pub fn sandbox_exec(_policy: &str, _command: &[OsString]) -> Result<u8, String> {
    Err("sandbox-exec is not used on this platform".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn sandbox_exec_arguments() {
        let (policy, command) =
            parse_sandbox_exec(args(&["--policy", "{}", "--", "/bin/echo", "hi"])).unwrap();
        assert_eq!(policy, "{}");
        assert_eq!(command, args(&["/bin/echo", "hi"]));
        for bad in [
            &[][..],
            &["--policy"][..],
            &["--policy", "{}"][..],
            &["--policy", "{}", "--"][..],
            &["--other", "{}", "--", "x"][..],
        ] {
            assert!(parse_sandbox_exec(args(bad)).is_err(), "{bad:?}");
        }
    }
}
