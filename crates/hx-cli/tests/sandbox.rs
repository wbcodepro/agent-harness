//! Escape attempts and policy checks, through the real `hx` binary with the
//! default (confined) settings.
#![cfg(target_os = "macos")]

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use hx_eval::{Recording, ScriptedResponse};
use hx_session::{Block, Delta, Event, SessionStore, StopReason, Usage};
use serde_json::{Value, json};

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    work: PathBuf,
}

impl Fixture {
    fn new(config: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let fixture = Self {
            home: root.join("home"),
            work: root.join("work"),
            root,
            _dir: dir,
        };
        fs::create_dir_all(&fixture.work).unwrap();
        fs::create_dir_all(&fixture.home).unwrap();
        // The fixture lives under the temp directory, which is writable by
        // default; turn that off so "outside the workspace" means something.
        fs::write(
            fixture.home.join("config.toml"),
            format!("[sandbox]\nwritable_tmp = false\n{config}"),
        )
        .unwrap();
        fixture
    }

    /// Runs one turn in which the model issues `calls` one step at a time,
    /// and returns each call's (text, is_error).
    fn run(&self, calls: Vec<(&str, Value)>, args: &[&str]) -> (Output, Vec<(String, bool)>) {
        let mut responses: Vec<ScriptedResponse> = calls
            .iter()
            .enumerate()
            .map(|(n, (name, input))| ScriptedResponse {
                deltas: vec![
                    Delta::ToolUseStart {
                        id: format!("c{n}"),
                        name: (*name).into(),
                    },
                    Delta::ToolInput {
                        json: input.to_string(),
                    },
                ],
                stop: StopReason::ToolUse,
                usage: Usage::default(),
            })
            .collect();
        responses.push(ScriptedResponse::text("done"));
        let script = self.root.join("script.jsonl");
        Recording { responses }.save(&script).unwrap();

        let output = Command::new(env!("CARGO_BIN_EXE_hx"))
            .arg("--home")
            .arg(&self.home)
            .arg("run")
            .args(args)
            .arg("go")
            .current_dir(&self.work)
            .env("HX_SCRIPT", &script)
            .env("HARNESS_ONLY_TOKEN", "hx-private-token-123456")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "hx failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let store = SessionStore::new(self.home.join("sessions"));
        let newest = store.list().unwrap().pop().unwrap().id;
        let results = store
            .read(&newest)
            .unwrap()
            .into_iter()
            .filter_map(|record| match record.event {
                Event::ToolResult {
                    content, is_error, ..
                } => match content.into_iter().next() {
                    Some(Block::Text { text }) => Some((text, is_error)),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        (output, results)
    }
}

fn bash(command: impl Into<String>) -> (&'static str, Value) {
    ("bash", json!({ "command": command.into() }))
}

#[test]
fn confined_tools_cannot_write_outside_the_workspace_or_read_secrets() {
    let fx = Fixture::new("");
    let outside = fx.root.join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("public"), "readable").unwrap();
    fs::write(fx.work.join(".env"), "TOKEN=from-dotenv-123456").unwrap();
    fs::write(fx.home.join(".env"), "HOME_KEY=from-home-123456").unwrap();
    std::os::unix::fs::symlink(&outside, fx.work.join("link")).unwrap();
    let out = |name: &str| outside.join(name).display().to_string();

    let (_, results) = fx.run(
        vec![
            // Allowed.
            ("write", json!({"path": "a.txt", "content": "inside"})),
            bash("cat a.txt && cat ../outside/public"),
            // Writes outside: file tool, absolute, traversal, symlink, shell.
            ("write", json!({"path": out("w1"), "content": "x"})),
            ("write", json!({"path": "../outside/w2", "content": "x"})),
            ("write", json!({"path": "link/w3", "content": "x"})),
            (
                "edit",
                json!({"path": out("public"), "old_string": "readable", "new_string": "x"}),
            ),
            bash(format!("echo x > {}", out("w4"))),
            bash("rm ../outside/public"),
            // Secrets: file tool and shell, the harness's own data, env.
            ("read", json!({"path": ".env"})),
            bash("cat .env"),
            bash(format!(
                "cat {}/.env; ls {}",
                fx.home.display(),
                fx.home.display()
            )),
            bash("echo token=${HARNESS_ONLY_TOKEN:-unset}"),
        ],
        &[],
    );

    let ok: Vec<bool> = results.iter().map(|(_, is_error)| !is_error).collect();
    assert_eq!(
        ok,
        [
            true, true, false, false, false, false, false, false, false, false, false, true
        ],
        "{results:#?}"
    );
    assert_eq!(results[1].0, "insidereadable");
    // The harness's environment did not reach the tool.
    assert_eq!(results[11].0, "token=unset\n");
    // Nothing outside changed.
    assert_eq!(
        fs::read_to_string(outside.join("public")).unwrap(),
        "readable"
    );
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 1);
    // No secret value reached the log.
    let all: String = results.iter().map(|(text, _)| text.as_str()).collect();
    assert!(
        !all.contains("from-dotenv") && !all.contains("from-home"),
        "{all}"
    );
}

#[test]
fn the_network_is_limited_to_the_allowlist() {
    let fx = Fixture::new("[network]\nmode = \"allowlist\"\nallow = [\"allowed.invalid\"]\n");
    let (output, results) = fx.run(
        vec![
            // Through the proxy, to a host that is not allowed.
            bash("curl -sS -m 10 -o /dev/null -w '%{http_code}' http://example.com/"),
            // Around the proxy.
            bash("curl -sS -m 10 --noproxy '*' -o /dev/null http://example.com/"),
        ],
        &[],
    );
    assert_eq!(results[0].0.trim(), "403", "{results:#?}");
    assert!(results[1].1, "a direct connection must fail: {results:#?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("network: blocked example.com:80"),
        "{stderr}"
    );

    let (_, results) = fx.run(
        vec![bash("curl -sS -m 10 -o /dev/null http://example.com/")],
        &["--network", "off"],
    );
    assert!(results[0].1, "{results:#?}");
}

#[test]
fn granted_secrets_reach_tools_but_never_the_log() {
    let fx = Fixture::new("[secrets]\ngrant = [\"DEPLOY_TOKEN\"]\n");
    fs::write(
        fx.home.join(".env"),
        "DEPLOY_TOKEN=tok-granted-abcdef\nOTHER_TOKEN=tok-withheld-abcdef\n",
    )
    .unwrap();
    let (_, results) = fx.run(
        vec![bash(
            "echo deploy=$DEPLOY_TOKEN other=${OTHER_TOKEN:-unset}; test -n \"$DEPLOY_TOKEN\"",
        )],
        &[],
    );
    assert_eq!(
        results,
        [(
            "deploy=[secret:DEPLOY_TOKEN] other=unset\n".to_string(),
            false
        )]
    );
}

#[test]
fn read_only_mode_and_rules_are_enforced() {
    let fx =
        Fixture::new("[[rule]]\ntool = \"bash\"\npattern = \"git push*\"\naction = \"deny\"\n");
    fs::write(fx.work.join("a.txt"), "1").unwrap();
    let (_, results) = fx.run(
        vec![
            ("read", json!({"path": "a.txt"})),
            ("write", json!({"path": "a.txt", "content": "2"})),
            bash("echo 3 > a.txt"),
        ],
        &["--sandbox", "read-only"],
    );
    let ok: Vec<bool> = results.iter().map(|(_, is_error)| !is_error).collect();
    assert_eq!(ok, [true, false, false], "{results:#?}");
    assert_eq!(fs::read_to_string(fx.work.join("a.txt")).unwrap(), "1");

    let (_, results) = fx.run(vec![bash("git push origin main"), bash("echo fine")], &[]);
    assert!(results[0].1 && results[0].0.contains("permission rules"));
    assert_eq!(results[1], ("fine\n".to_string(), false));
}

#[test]
fn rewind_restores_the_workspace_and_forks_the_session() {
    let fx = Fixture::new("");
    fs::write(fx.work.join("keep.txt"), "original").unwrap();

    fx.run(
        vec![("write", json!({"path": "first.txt", "content": "turn 1"}))],
        &[],
    );
    let store = SessionStore::new(fx.home.join("sessions"));
    let id = store.list().unwrap().pop().unwrap().id;
    fx.run(
        vec![
            ("write", json!({"path": "keep.txt", "content": "changed"})),
            ("write", json!({"path": "second.txt", "content": "turn 2"})),
            bash("rm first.txt"),
        ],
        &["--session", id.as_str()],
    );
    assert!(!fx.work.join("first.txt").exists());

    let output = Command::new(env!("CARGO_BIN_EXE_hx"))
        .arg("--home")
        .arg(&fx.home)
        .args(["rewind", id.as_str()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    // The workspace is back to the end of turn 1.
    assert_eq!(
        fs::read_to_string(fx.work.join("keep.txt")).unwrap(),
        "original"
    );
    assert_eq!(
        fs::read_to_string(fx.work.join("first.txt")).unwrap(),
        "turn 1"
    );
    assert!(!fx.work.join("second.txt").exists());

    // And the fork holds turn 1 only, leaving the original intact.
    let fork =
        hx_session::SessionId::parse(String::from_utf8_lossy(&output.stdout).trim()).unwrap();
    let turns = |id| {
        store
            .read(id)
            .unwrap()
            .iter()
            .filter(|record| matches!(record.event, Event::TurnStart { .. }))
            .count()
    };
    assert_eq!((turns(&fork), turns(&id)), (1, 2));
}
