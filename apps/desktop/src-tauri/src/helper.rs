//! The internal commands hame runs its own executable for.
//!
//! hame confines every tool call in a fresh `<exe> tool-worker` process
//! and, on Linux and Windows, wraps commands in `<exe> sandbox-exec`. In the
//! desktop app `<exe>` is this binary, so it answers those two before
//! anything of the app starts (no window, no tray, no Dock icon). This
//! mirrors `hame tool-worker` and `hame sandbox-exec` in `crates/hame-cli`.

use std::ffi::OsString;
use std::process::ExitCode;

/// Runs an internal command when the arguments name one.
pub fn internal_command() -> Option<ExitCode> {
    let mut args = std::env::args_os().skip(1);
    let command = args.next()?;
    match command.to_str()? {
        "tool-worker" => Some(tool_worker()),
        "sandbox-exec" => Some(match parse_sandbox_exec(args.collect()) {
            Ok((policy, command)) => report(sandbox_exec(&policy, &command)),
            Err(message) => report(Err(message)),
        }),
        _ => None,
    }
}

fn report(result: Result<ExitCode, String>) -> ExitCode {
    result.unwrap_or_else(|message| {
        eprintln!("hame: {message}");
        ExitCode::FAILURE
    })
}

fn tool_worker() -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => return report(Err(format!("cannot start the runtime: {e}"))),
    };
    let code = runtime.block_on(hame_sandbox::worker_main(&hame_tools::ToolSet::every()));
    // On Windows a program the tool left running still holds the pipes its
    // output was read from; the answer is written, so leave.
    #[cfg(windows)]
    std::process::exit(code.into());
    #[cfg(not(windows))]
    ExitCode::from(code)
}

/// `--policy <json> -- <program> [args…]`.
fn parse_sandbox_exec(args: Vec<OsString>) -> Result<(String, Vec<OsString>), String> {
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

#[cfg(target_os = "linux")]
fn sandbox_exec(policy: &str, command: &[OsString]) -> Result<ExitCode, String> {
    use std::os::unix::process::CommandExt;
    let policy: hame_sandbox::SandboxPolicy =
        serde_json::from_str(policy).map_err(|e| format!("invalid sandbox policy: {e}"))?;
    hame_sandbox::apply_to_current_process(&policy).map_err(|e| e.to_string())?;
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
fn sandbox_exec(policy: &str, command: &[OsString]) -> Result<ExitCode, String> {
    let policy: hame_sandbox::SandboxPolicy =
        serde_json::from_str(policy).map_err(|e| format!("invalid sandbox policy: {e}"))?;
    hame_sandbox::apply_to_current_process(&policy).map_err(|e| e.to_string())?;
    hame_sandbox::keep_stdio_private();
    let status = std::process::Command::new(&command[0])
        .args(&command[1..])
        .status()
        .map_err(|e| format!("cannot execute {}: {e}", command[0].to_string_lossy()))?;
    Ok(ExitCode::from(
        status.code().unwrap_or(1).clamp(0, 255) as u8
    ))
}

#[cfg(not(any(target_os = "linux", windows)))]
fn sandbox_exec(_policy: &str, _command: &[OsString]) -> Result<ExitCode, String> {
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
