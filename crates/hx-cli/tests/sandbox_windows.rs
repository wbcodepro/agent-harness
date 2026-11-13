//! The Windows sandbox, through the real binary: writes are confined to
//! the workspace by lowering the tool process's integrity level.
#![cfg(windows)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use hx_eval::{Recording, ScriptedResponse};
use hx_session::{Block, Delta, Event, SessionStore, StopReason, Usage};
use serde_json::{Value, json};

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    work: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = hx_kernel::real_path(dir.path()).unwrap();
        let work = root.join("work");
        fs::create_dir_all(&work).unwrap();
        Self {
            _dir: dir,
            root,
            work,
        }
    }

    fn home(&self, name: &str, config: &str) -> PathBuf {
        let home = self.root.join(name);
        fs::create_dir_all(&home).unwrap();
        fs::write(home.join("config.toml"), config).unwrap();
        home
    }

    fn run(
        &self,
        home: &Path,
        calls: Vec<(&str, Value)>,
        args: &[&str],
    ) -> (Output, Vec<(String, bool)>) {
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
            .arg(home)
            .arg("run")
            .args(args)
            .arg("go")
            .current_dir(&self.work)
            .env("HX_SCRIPT", &script)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        let store = SessionStore::new(home.join("sessions"));
        let results = store
            .list()
            .unwrap_or_default()
            .pop()
            .map(|header| store.read(&header.id).unwrap())
            .unwrap_or_default()
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

#[test]
fn confined_tools_can_write_the_workspace_and_nothing_else() {
    let fx = Fixture::new();
    let outside = fx.root.join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("public"), "readable").unwrap();
    // A file that exists before the workspace is first labelled.
    fs::write(fx.work.join("old.txt"), "old").unwrap();
    let home = fx.home("home", "[sandbox]\nallow_partial = true\n");
    let out = |name: &str| outside.join(name).display().to_string();

    let (output, results) = fx.run(
        &home,
        vec![
            ("write", json!({"path": "a.txt", "content": "inside"})),
            (
                "write",
                json!({"path": "sub/deep/b.txt", "content": "nested"}),
            ),
            (
                "edit",
                json!({"path": "old.txt", "old_string": "old", "new_string": "new"}),
            ),
            ("write", json!({"path": out("w1"), "content": "x"})),
            ("write", json!({"path": "../outside/w2", "content": "x"})),
            (
                "edit",
                json!({"path": out("public"), "old_string": "readable", "new_string": "x"}),
            ),
            // Reads are not confined on Windows; that is the stated limit.
            ("read", json!({"path": out("public")})),
        ],
        &[],
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(stderr.contains("confined for writes only"), "{stderr}");

    let ok: Vec<bool> = results.iter().map(|(_, is_error)| !is_error).collect();
    assert_eq!(
        ok,
        [true, true, true, false, false, false, true],
        "{results:#?}\n{stderr}"
    );
    assert_eq!(fs::read_to_string(fx.work.join("a.txt")).unwrap(), "inside");
    assert_eq!(
        fs::read_to_string(fx.work.join("sub/deep/b.txt")).unwrap(),
        "nested"
    );
    assert_eq!(fs::read_to_string(fx.work.join("old.txt")).unwrap(), "new");
    assert_eq!(
        fs::read_to_string(outside.join("public")).unwrap(),
        "readable"
    );
    assert_eq!(
        fs::read_dir(&outside).unwrap().count(),
        1,
        "nothing was created outside"
    );
}

#[test]
fn read_only_mode_writes_nothing_and_partial_confinement_must_be_accepted() {
    let fx = Fixture::new();
    fs::write(fx.work.join("a.txt"), "1").unwrap();
    let home = fx.home("home", "[sandbox]\nallow_partial = true\n");
    let (output, results) = fx.run(
        &home,
        vec![
            ("read", json!({"path": "a.txt"})),
            ("write", json!({"path": "a.txt", "content": "2"})),
            ("write", json!({"path": "new.txt", "content": "2"})),
        ],
        &["--sandbox", "read-only"],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let ok: Vec<bool> = results.iter().map(|(_, is_error)| !is_error).collect();
    assert_eq!(ok, [true, false, false], "{results:#?}");
    assert_eq!(fs::read_to_string(fx.work.join("a.txt")).unwrap(), "1");
    assert!(!fx.work.join("new.txt").exists());

    // Without the explicit acceptance, hx refuses to start confined rather
    // than promise protection it cannot give.
    let strict = fx.home("strict", "");
    let (output, results) = fx.run(
        &strict,
        vec![("write", json!({"path": "c.txt", "content": "x"}))],
        &[],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("allow_partial"));
    assert!(results.is_empty() && !fx.work.join("c.txt").exists());

    // Unconfined mode still works, behind approval: with nobody to ask,
    // the write is denied.
    let (output, results) = fx.run(
        &strict,
        vec![("write", json!({"path": "d.txt", "content": "x"}))],
        &["--sandbox", "danger-full-access"],
    );
    assert!(output.status.success());
    assert!(
        results[0].1 && results[0].0.contains("Denied"),
        "{results:#?}"
    );
    assert!(!fx.work.join("d.txt").exists());
}

#[test]
fn commands_started_by_the_shell_tool_inherit_the_confinement() {
    let fx = Fixture::new();
    let outside = fx.root.join("outside");
    fs::create_dir(&outside).unwrap();
    let home = fx.home("home", "[sandbox]\nallow_partial = true\n");
    // Forward slashes: the path goes through a POSIX shell.
    let target = outside
        .join("from-shell.txt")
        .display()
        .to_string()
        .replace('\\', "/");

    let (output, results) = fx.run(
        &home,
        vec![
            (
                "bash",
                json!({"command": "echo inside > shell.txt && cat shell.txt"}),
            ),
            (
                "bash",
                json!({"command": format!("echo escaped > '{target}'")}),
            ),
            // A grandchild process is confined as well.
            (
                "bash",
                json!({"command": format!("bash -c \"echo escaped > '{target}'\"")}),
            ),
        ],
        &[],
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert_eq!(results.len(), 3, "{stderr}");
    assert!(
        !results[0].1,
        "the shell must work inside the workspace: {results:#?}"
    );
    assert!(results[0].0.contains("inside"));
    assert!(
        fs::read_to_string(fx.work.join("shell.txt"))
            .unwrap()
            .contains("inside")
    );
    assert!(results[1].1 && results[2].1, "{results:#?}");
    assert_eq!(
        fs::read_dir(&outside).unwrap().count(),
        0,
        "nothing was created outside"
    );
}
