use std::path::Path;

use crate::policy::{Mode, Network, SandboxPolicy};

/// Builds the Seatbelt profile for `policy`.
///
/// The profile starts permissive and then takes away: all writes except the
/// listed roots, reads of credential locations, and the network. In SBPL the
/// last matching rule wins, so the order below is the precedence.
pub(crate) fn profile(policy: &SandboxPolicy) -> String {
    let mut out = String::from("(version 1)\n(allow default)\n");

    out.push_str("(deny file-write*)\n");
    out.push_str(
        "(allow file-write* (literal \"/dev/null\") (literal \"/dev/zero\") \
         (literal \"/dev/tty\") (literal \"/dev/dtracehelper\") \
         (regex #\"^/dev/ttys[0-9]+$\") (regex #\"^/dev/fd/\"))\n",
    );
    if policy.mode == Mode::WorkspaceWrite {
        for root in &policy.writable {
            out.push_str(&format!("(allow file-write* (subpath {}))\n", quote(root)));
        }
    }

    for path in &policy.unreadable {
        out.push_str(&format!(
            "(deny file-read* file-write* (subpath {}))\n",
            quote(path)
        ));
    }
    if policy.hide_env_files {
        out.push_str("(deny file-read* file-write* (regex #\"/\\.env(\\.[^/]+)?$\"))\n");
    }

    match policy.network {
        Network::Open => {}
        Network::Blocked => out.push_str("(deny network*)\n"),
        Network::Proxy { .. } => {
            out.push_str("(deny network*)\n");
            out.push_str("(allow network* (local ip \"localhost:*\"))\n");
            out.push_str("(allow network* (remote ip \"localhost:*\"))\n");
        }
    }
    out
}

fn quote(path: &Path) -> String {
    let raw = path.to_string_lossy();
    format!("\"{}\"", raw.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::wrap;
    use std::ffi::OsString;
    use std::fs;
    use std::process::Command;

    fn sh(policy: &SandboxPolicy, cwd: &Path, script: &str) -> (bool, String) {
        let wrapped = wrap(
            policy,
            Path::new("/unused"),
            Path::new("/bin/sh"),
            &[OsString::from("-c"), OsString::from(script)],
        )
        .unwrap();
        let output = Command::new(wrapped.program)
            .args(wrapped.args)
            .current_dir(cwd)
            .output()
            .unwrap();
        (
            output.status.success(),
            String::from_utf8_lossy(&output.stdout).into_owned()
                + &String::from_utf8_lossy(&output.stderr),
        )
    }

    #[test]
    fn workspace_mode_confines_writes_and_hides_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let work = root.join("work");
        let outside = root.join("outside");
        let vault = root.join("vault");
        for d in [&work, &outside, &vault] {
            fs::create_dir(d).unwrap();
        }
        fs::write(vault.join("key"), "s3cret").unwrap();
        fs::write(work.join(".env"), "TOKEN=s3cret").unwrap();
        fs::write(outside.join("readable"), "public").unwrap();
        std::os::unix::fs::symlink(&outside, work.join("link")).unwrap();

        let policy = SandboxPolicy {
            mode: Mode::WorkspaceWrite,
            writable: vec![work.clone()],
            unreadable: vec![vault.clone()],
            hide_env_files: true,
            network: Network::Blocked,
        };
        let run = |script: &str| sh(&policy, &work, script);

        assert!(run("echo hi > inside && mkdir -p a/b && echo x > a/b/c").0);
        assert!(run("cat ../outside/readable").1.contains("public"));

        // Writes outside the workspace: directly, by traversal, via symlink.
        assert!(!run(&format!("echo x > {}/f", outside.display())).0);
        assert!(!run("echo x > ../outside/f").0);
        assert!(!run("echo x > link/f").0);
        assert!(!run("rm ../outside/readable").0);
        assert!(!run("mv ../outside/readable inside2").0);
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 1);

        // Secrets cannot be read, listed or copied out.
        assert!(!run(&format!("cat {}/key", vault.display())).0);
        assert!(!run(&format!("ls {}", vault.display())).0);
        assert!(!run(&format!("cp {}/key stolen", vault.display())).0);
        assert!(!run("cat .env").0);
        assert!(!run("echo OVERWRITE > .env").0);
        assert!(!work.join("stolen").exists());

        // No network.
        let (ok, out) = run("curl -sS -m 5 -o /dev/null https://example.com");
        assert!(!ok, "{out}");
    }

    #[test]
    fn read_only_mode_cannot_write_even_in_the_working_directory() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().canonicalize().unwrap();
        fs::write(work.join("a"), "1").unwrap();
        let mut policy = SandboxPolicy::read_only();
        policy.unreadable.clear();

        assert!(sh(&policy, &work, "cat a").1.contains('1'));
        assert!(!sh(&policy, &work, "echo 2 > a").0);
        assert!(!sh(&policy, &work, "touch b").0);
        assert!(!sh(&policy, &work, "echo x > /tmp/hx-ro-escape").0);
        assert_eq!(fs::read_to_string(work.join("a")).unwrap(), "1");
    }

    #[test]
    fn paths_are_quoted_for_the_profile_language() {
        assert_eq!(quote(Path::new("/a b/c\"d\\e")), "\"/a b/c\\\"d\\\\e\"");
    }
}
