//! Escape attempts and policy checks, through the real `hame` binary with the
//! default (confined) settings.
#![cfg(any(target_os = "macos", target_os = "linux"))]

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use hame_eval::{Recording, ScriptedResponse};
use hame_session::{Block, Delta, Event, SessionStore, StopReason, Usage};
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

        let output = Command::new(env!("CARGO_BIN_EXE_hame"))
            .arg("--home")
            .arg(&self.home)
            .arg("run")
            .args(args)
            .arg("go")
            .current_dir(&self.work)
            .env("HAME_SCRIPT", &script)
            .env("HARNESS_ONLY_TOKEN", "hame-private-token-123456")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "hame failed: {}",
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
                "cat {0}/.env || cat {0}/config.toml || ls {0}/sessions",
                fx.home.display()
            )),
            bash("echo token=${HARNESS_ONLY_TOKEN:-unset}"),
        ],
        &[],
    );

    let ok: Vec<bool> = results.iter().map(|(_, is_error)| !is_error).collect();
    // Landlock cannot hide files by name, so on Linux a workspace `.env`
    // stays readable; its values are still redacted (checked below).
    let env_file = cfg!(target_os = "linux");
    assert_eq!(
        ok,
        [
            true, true, false, false, false, false, false, false, env_file, env_file, false, true
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

    let output = Command::new(env!("CARGO_BIN_EXE_hame"))
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
        hame_session::SessionId::parse(String::from_utf8_lossy(&output.stdout).trim()).unwrap();
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

#[test]
fn checks_hold_the_turn_open_until_they_pass() {
    let fx = Fixture::new("");
    fs::create_dir(fx.work.join(".hame")).unwrap();
    fs::write(
        fx.work.join(".hame/config.toml"),
        "[verify]\ncommands = [\"grep -q good out.txt\"]\n",
    )
    .unwrap();
    fs::write(fx.work.join("AGENTS.md"), "Always write lowercase.\n").unwrap();

    // The model writes the wrong content and says it is done; the gate
    // sends it back; it fixes the file and is then allowed to stop.
    let mut responses = vec![
        ScriptedResponse {
            deltas: vec![
                Delta::ToolUseStart {
                    id: "c0".into(),
                    name: "write".into(),
                },
                Delta::ToolInput {
                    json: json!({"path": "out.txt", "content": "bad"}).to_string(),
                },
            ],
            stop: StopReason::ToolUse,
            usage: Usage::default(),
        },
        ScriptedResponse::text("Done."),
    ];
    responses.push(ScriptedResponse {
        deltas: vec![
            Delta::ToolUseStart {
                id: "c1".into(),
                name: "write".into(),
            },
            Delta::ToolInput {
                json: json!({"path": "out.txt", "content": "good"}).to_string(),
            },
        ],
        stop: StopReason::ToolUse,
        usage: Usage::default(),
    });
    responses.push(ScriptedResponse::text("Fixed."));
    let script = fx.root.join("verify.jsonl");
    Recording { responses }.save(&script).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_hame"))
        .arg("--home")
        .arg(&fx.home)
        .args(["run", "write out.txt"])
        .current_dir(&fx.work)
        .env("HAME_SCRIPT", &script)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "Done.\nFixed.\n");
    assert_eq!(
        stderr.matches("verify: checks failed").count(),
        1,
        "{stderr}"
    );
    assert_eq!(fs::read_to_string(fx.work.join("out.txt")).unwrap(), "good");

    // The project's instructions became part of the logged system prompt.
    let store = SessionStore::new(fx.home.join("sessions"));
    let id = store.list().unwrap().pop().unwrap().id;
    let system = store
        .read(&id)
        .unwrap()
        .into_iter()
        .find_map(|record| match record.event {
            Event::SystemMessage { text } => Some(text),
            _ => None,
        })
        .unwrap();
    assert!(system.contains("Always write lowercase."), "{system}");
}

#[test]
fn large_outputs_are_spilled_to_a_file_the_tools_can_read() {
    let fx = Fixture::new("[context]\nspill_at_bytes = 2000\n");
    let (_, results) = fx.run(vec![bash("seq 1 5000")], &[]);
    let (text, is_error) = &results[0];
    assert!(!is_error && text.len() < 4000, "{}", text.len());
    assert!(text.starts_with("1\n2\n") && text.contains("bytes omitted"));

    // The path it names is readable from inside the sandbox.
    let path = text
        .split("saved at ")
        .nth(1)
        .and_then(|rest| rest.split(". Use read").next())
        .unwrap()
        .to_string();
    let (_, results) = fx.run(
        vec![("grep", json!({"pattern": "^4321$", "path": path}))],
        &[],
    );
    assert!(results[0].0.ends_with(":4321:4321\n"), "{results:#?}");
}

#[test]
fn skills_hooks_mcp_servers_and_plugins_extend_one_agent() {
    let fx = Fixture::new("");
    // A skill in the project.
    fs::create_dir_all(fx.work.join(".hame/skills/greet")).unwrap();
    fs::write(
        fx.work.join(".hame/skills/greet/SKILL.md"),
        "---\nname: greet\ndescription: How to greet\n---\nAlways say xin chao.\n",
    )
    .unwrap();
    // An MCP server with one tool.
    let server = fx.root.join("server.py");
    fs::write(
        &server,
        r#"
import json, sys
def send(m): sys.stdout.write(json.dumps(m) + "\n"); sys.stdout.flush()
for line in sys.stdin:
    m = json.loads(line); i = m.get("id"); method = m.get("method")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": i, "result": {"protocolVersion": "2025-06-18", "capabilities": {}, "serverInfo": {"name": "t", "version": "0"}}})
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": i, "result": {"tools": [{"name": "add", "description": "Add.", "inputSchema": {"type": "object"}}]}})
    elif method == "tools/call":
        a = m["params"]["arguments"]
        send({"jsonrpc": "2.0", "id": i, "result": {"content": [{"type": "text", "text": str(a["a"] + a["b"])}]}})
"#,
    )
    .unwrap();
    // A plugin granted one directory.
    let plugin = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/wordcount");
    let built = Command::new(env!("CARGO"))
        .args(["build", "--release", "--target", "wasm32-unknown-unknown"])
        .current_dir(&plugin)
        .env_remove("CARGO_TARGET_DIR")
        .status()
        .unwrap();
    assert!(built.success());
    fs::create_dir(fx.work.join("docs")).unwrap();
    fs::write(fx.work.join("docs/a.txt"), "one two three\n").unwrap();
    fs::write(
        fx.home.join("config.toml"),
        format!(
            "[sandbox]\nwritable_tmp = false\n\n[mcp.calc]\ncommand = \"python3\"\nargs = [{server:?}]\n\n\
             [plugin.wc]\npath = {wasm:?}\nread = [\"docs\"]\n\n\
             [[hook]]\nevent = \"pre_tool\"\ntool = \"bash\"\n\
             command = \"grep -q forbidden && {{ echo not-that >&2; exit 2; }}; exit 0\"\n",
            server = server.display().to_string(),
            wasm = plugin
                .join("target/wasm32-unknown-unknown/release/hame_plugin_wordcount.wasm")
                .display()
                .to_string(),
        ),
    )
    .unwrap();

    let (output, results) = fx.run(
        vec![
            ("skill", json!({"name": "greet"})),
            ("mcp__calc__add", json!({"a": 2, "b": 40})),
            ("wc__file_stats", json!({"path": "docs/a.txt"})),
            ("wc__file_stats", json!({"path": "../home/config.toml"})),
            bash("echo forbidden"),
            bash("echo fine"),
        ],
        &[],
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        results[0].0.starts_with("Always say xin chao."),
        "{stderr}\n{results:#?}"
    );
    assert_eq!(results[1], ("42".to_string(), false), "{stderr}");
    assert_eq!(
        results[2],
        ("1 lines, 3 words, 14 characters".to_string(), false)
    );
    assert!(results[3].1 && results[3].0.contains("not been granted"));
    assert_eq!(
        results[4],
        (
            "Blocked by the user's pre-tool hook: not-that".to_string(),
            true
        )
    );
    assert_eq!(results[5], ("fine\n".to_string(), false));

    // The model was told about the skill and offered every extension tool.
    let store = SessionStore::new(fx.home.join("sessions"));
    let records = store
        .read(&store.list().unwrap().pop().unwrap().id)
        .unwrap();
    let system = records
        .iter()
        .find_map(|r| match &r.event {
            Event::SystemMessage { text } => Some(text.clone()),
            _ => None,
        })
        .unwrap();
    assert!(system.contains("- greet: How to greet"));
    let offered: Vec<String> = records
        .iter()
        .find_map(|r| match &r.event {
            Event::ToolsDeclared { tools } => Some(tools.iter().map(|t| t.name.clone()).collect()),
            _ => None,
        })
        .unwrap();
    for name in [
        "skill",
        "mcp__calc__add",
        "wc__text_stats",
        "wc__file_stats",
    ] {
        assert!(offered.contains(&name.to_string()), "{offered:?}");
    }

    // Past the configured count, extension tools are not declared at all:
    // the model searches for one and calls it through `tool_call`.
    let config = fs::read_to_string(fx.home.join("config.toml")).unwrap();
    fs::write(
        fx.home.join("config.toml"),
        format!("[context]\ndefer_tools_over = 2\n\n{config}"),
    )
    .unwrap();
    let (output, results) = fx.run(
        vec![
            ("tool_search", json!({"query": "add numbers"})),
            (
                "tool_call",
                json!({"name": "mcp__calc__add", "input": {"a": 2, "b": 40}}),
            ),
            (
                "tool_call",
                json!({"name": "wc__file_stats", "input": {"path": "../home/config.toml"}}),
            ),
            // A script chains calls in the sandbox and returns what it
            // prints; it cannot write outside the workspace either.
            (
                "script",
                json!({"code": "values = [call('mcp__calc__add', a=n, b=n) for n in (1, 20)]\nprint(sum(int(v) for v in values))"}),
            ),
            (
                "script",
                json!({"code": "open('../escaped.txt', 'w').write('x')"}),
            ),
        ],
        &[],
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        results[0].0.starts_with("mcp__calc__add\nAdd."),
        "{stderr}\n{results:#?}"
    );
    assert_eq!(results[1], ("42".to_string(), false), "{stderr}");
    assert!(results[2].1 && results[2].0.contains("not been granted"));
    assert_eq!(results[3], ("42\n".to_string(), false), "{stderr}");
    assert!(
        results[4].1 && results[4].0.contains("PermissionError"),
        "{}",
        results[4].0
    );
    assert!(!fx.root.join("escaped.txt").exists());
    let records = store
        .read(&store.list().unwrap().pop().unwrap().id)
        .unwrap();
    let offered: Vec<String> = records
        .iter()
        .find_map(|r| match &r.event {
            Event::ToolsDeclared { tools } => Some(tools.iter().map(|t| t.name.clone()).collect()),
            _ => None,
        })
        .unwrap();
    assert!(offered.contains(&"tool_search".to_string()), "{offered:?}");
    assert!(offered.contains(&"tool_call".to_string()), "{offered:?}");
    assert!(offered.contains(&"script".to_string()), "{offered:?}");
    assert!(
        !offered
            .iter()
            .any(|name| name.starts_with("mcp__") || name.starts_with("wc__")),
        "{offered:?}"
    );
}

#[test]
fn hash_anchored_reads_and_edits_run_in_the_confined_worker() {
    let fx = Fixture::new("[model]\nedit_format = \"hashline\"\n");
    fs::write(fx.work.join("a.txt"), "hello\nbye\n").unwrap();
    let (_, results) = fx.run(
        vec![
            ("read", json!({"path": "a.txt"})),
            (
                "edit_lines",
                json!({"path": "a.txt", "edits": [{"from": "1:516", "new_text": "hi"}]}),
            ),
            // The anchor of the line just replaced no longer holds.
            (
                "edit_lines",
                json!({"path": "a.txt", "edits": [{"from": "1:516", "new_text": "ho"}]}),
            ),
        ],
        &[],
    );
    // The worker finds the `read` that shows anchors, not the plain one.
    assert!(
        results[0].0.starts_with("1:516|hello\n2:") && results[0].0.ends_with("|bye\n"),
        "{}",
        results[0].0
    );
    assert!(!results[0].1);
    assert!(!results[1].1, "{}", results[1].0);
    assert!(results[2].1 && results[2].0.contains("is stale"));
    assert_eq!(
        fs::read_to_string(fx.work.join("a.txt")).unwrap(),
        "hi\nbye\n"
    );
}
