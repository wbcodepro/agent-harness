//! Drives the real `zcode` binary with scripted model responses.
#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use zcode_eval::{Recording, ScriptedResponse};
use zcode_session::{
    AttemptReason, Block, Delta, Event, INTERRUPTED_RUNNING, Record, SessionId, SessionStore,
    StopReason, TurnEndReason, Usage, plan_recovery,
};

struct Fixture {
    _dir: tempfile::TempDir,
    home: PathBuf,
    work: PathBuf,
    scripts: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let fixture = Self {
            home: root.join("home"),
            work: root.join("work"),
            scripts: root.join("scripts"),
            _dir: dir,
        };
        fs::create_dir_all(&fixture.work).unwrap();
        fs::create_dir_all(&fixture.scripts).unwrap();
        fixture
    }

    fn script(&self, name: &str, responses: Vec<ScriptedResponse>) -> PathBuf {
        let path = self.scripts.join(name);
        Recording { responses }.save(&path).unwrap();
        path
    }

    fn zcode(&self, script: &Path, delay_ms: u64, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_zcode"));
        command
            .arg("--home")
            .arg(&self.home)
            .arg("run")
            .args(args)
            .current_dir(&self.work)
            .env("ZCODE_SCRIPT", script)
            .env("ZCODE_SCRIPT_DELAY_MS", delay_ms.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    fn store(&self) -> SessionStore {
        SessionStore::new(self.home.join("sessions"))
    }

    fn only_session(&self) -> Option<SessionId> {
        let mut headers = self.store().list().ok()?;
        (headers.len() == 1).then(|| headers.remove(0).id)
    }

    /// Polls the only session's log until `ready` holds.
    fn wait_for(&self, what: &str, ready: impl Fn(&[Record]) -> bool) -> SessionId {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(id) = self.only_session()
                && let Ok(records) = self.store().read(&id)
                && ready(&records)
            {
                return id;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

fn tool_call(id: &str, name: &str, input: Value) -> ScriptedResponse {
    ScriptedResponse {
        deltas: vec![
            Delta::ToolUseStart {
                id: id.into(),
                name: name.into(),
            },
            Delta::ToolInput {
                json: input.to_string(),
            },
        ],
        stop: StopReason::ToolUse,
        usage: Usage::default(),
    }
}

fn text(output: &[u8]) -> String {
    String::from_utf8_lossy(output).into_owned()
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "zcode failed\nstdout: {}\nstderr: {}",
        text(&output.stdout),
        text(&output.stderr)
    );
}

fn events(records: &[Record]) -> Vec<&Event> {
    records.iter().map(|record| &record.event).collect()
}

#[test]
fn a_scripted_run_edits_the_workspace_and_prints_the_answer() {
    let fx = Fixture::new();
    let script = fx.script(
        "ok.jsonl",
        vec![
            tool_call("c1", "write", json!({"path": "out.txt", "content": "hi\n"})),
            tool_call("c2", "bash", json!({"command": "cat out.txt"})),
            ScriptedResponse::text("Wrote out.txt and checked it."),
        ],
    );

    let output = fx
        .zcode(&script, 0, &["--yes", "Make out.txt"])
        .output()
        .unwrap();

    assert_success(&output);
    assert_eq!(text(&output.stdout), "Wrote out.txt and checked it.\n");
    assert_eq!(fs::read_to_string(fx.work.join("out.txt")).unwrap(), "hi\n");
    let stderr = text(&output.stderr);
    assert!(stderr.contains("> bash cat out.txt"), "{stderr}");
    assert!(stderr.contains("< ok: hi"), "{stderr}");

    let id = fx.only_session().unwrap();
    assert!(stderr.contains(&format!("session: {id}")));
    let records = fx.store().read(&id).unwrap();
    assert!(matches!(
        records.last().unwrap().event,
        Event::TurnEnd {
            reason: TurnEndReason::Completed,
            ..
        }
    ));
}

#[test]
fn programs_can_read_the_outcome_as_json() {
    let fx = Fixture::new();
    let responses = || {
        vec![
            tool_call("c1", "write", json!({"path": "out.txt", "content": "hi\n"})),
            ScriptedResponse::text("Wrote out.txt."),
        ]
    };

    let script = fx.script("json.jsonl", responses());
    let output = fx
        .zcode(&script, 0, &["--yes", "--output", "json", "Make out.txt"])
        .output()
        .unwrap();
    assert_success(&output);
    let result: Value = serde_json::from_str(text(&output.stdout).trim()).unwrap();
    assert_eq!(result["type"], "result");
    assert_eq!(result["status"], "completed");
    assert_eq!(result["text"], "Wrote out.txt.");
    assert_eq!(result["requests"], 2);
    assert_eq!(result["tool_calls"], 1);
    assert_eq!(result["error"], Value::Null);
    let id = result["session"].as_str().unwrap().to_string();
    assert!(fx.store().read(&SessionId::parse(&id).unwrap()).is_ok());
    assert!(
        !text(&output.stderr).contains("> write"),
        "no trace for programs"
    );

    // The stream is the session's own records, then the same result.
    let script = fx.script("stream.jsonl", responses());
    let output = fx
        .zcode(
            &script,
            0,
            &[
                "--yes",
                "--output",
                "stream-json",
                "--session",
                &id,
                "Again",
            ],
        )
        .output()
        .unwrap();
    assert_success(&output);
    let lines: Vec<Value> = text(&output.stdout)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let types: Vec<&str> = lines
        .iter()
        .filter_map(|line| line["type"].as_str())
        .collect();
    assert_eq!(types.first(), Some(&"turn/start"), "{types:?}");
    assert!(
        types.contains(&"tool/call") && types.contains(&"tool/result"),
        "{types:?}"
    );
    assert!(!types.contains(&"assistant/delta"), "{types:?}");
    assert_eq!(types.last(), Some(&"result"));
    assert_eq!(lines.last().unwrap()["session"], id.as_str());

    // A failed turn is still one parseable object, with a failing status.
    let script = fx.script("empty.jsonl", Vec::new());
    let output = fx
        .zcode(
            &script,
            0,
            &["--yes", "--output", "json", "Nothing scripted"],
        )
        .output()
        .unwrap();
    assert!(!output.status.success());
    let result: Value = serde_json::from_str(text(&output.stdout).trim()).unwrap();
    assert_eq!(result["status"], "error");
    assert!(result["error"].is_string());
}

#[test]
fn unconfined_tools_need_approval_and_nobody_is_there_to_give_it() {
    let fx = Fixture::new();
    fs::write(fx.work.join("in.txt"), "data\n").unwrap();
    let script = fx.script(
        "denied.jsonl",
        vec![
            tool_call("c1", "read", json!({"path": "in.txt"})),
            tool_call("c2", "write", json!({"path": "out.txt", "content": "x"})),
            tool_call("c3", "bash", json!({"command": "touch ran"})),
            ScriptedResponse::text("I was not allowed to change anything."),
        ],
    );

    // Full access, no --yes, and stdin is not a terminal.
    let output = fx
        .zcode(
            &script,
            0,
            &["--sandbox", "danger-full-access", "Change things"],
        )
        .output()
        .unwrap();

    assert_success(&output);
    assert!(!fx.work.join("out.txt").exists());
    assert!(!fx.work.join("ran").exists());
    let stderr = text(&output.stderr);
    assert!(stderr.contains("< ok: 1\tdata"), "{stderr}");
    assert_eq!(stderr.matches("< error: Denied").count(), 2, "{stderr}");
}

#[test]
fn killed_mid_stream_the_session_resumes_with_the_partial_answer_kept_aside() {
    let fx = Fixture::new();
    let words: Vec<Delta> = (0..200)
        .map(|n| Delta::Text {
            text: format!("word{n} "),
        })
        .collect();
    let slow = fx.script(
        "slow.jsonl",
        vec![ScriptedResponse {
            deltas: words,
            stop: StopReason::EndTurn,
            usage: Usage::default(),
        }],
    );

    let mut child = fx.zcode(&slow, 20, &["Talk for a while"]).spawn().unwrap();
    let id = fx.wait_for("some streamed deltas", |records| {
        records
            .iter()
            .filter(|r| matches!(r.event, Event::AssistantDelta { .. }))
            .count()
            >= 5
    });
    child.kill().unwrap(); // SIGKILL: no chance to clean up
    child.wait().unwrap();
    let before = fx.store().read(&id).unwrap();

    let resume = fx.script("resume.jsonl", vec![ScriptedResponse::text("Picking up.")]);
    let output = fx
        .zcode(&resume, 0, &["--session", id.as_str(), "Continue"])
        .output()
        .unwrap();
    assert_success(&output);
    assert_eq!(text(&output.stdout), "Picking up.\n");

    let after = fx.store().read(&id).unwrap();
    // Nothing written before the kill was lost or altered.
    assert_eq!(after[..before.len()], before[..]);
    let recovered = events(&after[before.len()..]);
    let Event::AssistantAttempt {
        content,
        reason: AttemptReason::Interrupted,
    } = recovered[0]
    else {
        panic!("expected the partial answer first, got {:?}", recovered[0]);
    };
    assert!(matches!(&content[0], Block::Text { text } if text.starts_with("word0 word1 ")));
    assert_eq!(recovered[1], &Event::StepEnd { turn: 1, step: 1 });
    assert_eq!(
        recovered[2],
        &Event::TurnEnd {
            turn: 1,
            reason: TurnEndReason::Interrupted
        }
    );
    assert_eq!(recovered[3], &Event::TurnStart { turn: 2 });
    assert!(matches!(
        after.last().unwrap().event,
        Event::TurnEnd {
            turn: 2,
            reason: TurnEndReason::Completed
        }
    ));
    assert!(plan_recovery(&after).is_empty());
}

#[test]
fn killed_mid_tool_the_model_is_told_the_call_may_have_taken_effect() {
    let fx = Fixture::new();
    let script = fx.script(
        "tool.jsonl",
        vec![tool_call(
            "c1",
            "bash",
            json!({"command": "touch started; sleep 3"}),
        )],
    );

    let mut child = fx.zcode(&script, 0, &["--yes", "Run it"]).spawn().unwrap();
    let id = fx.wait_for("the tool call to be logged", |records| {
        records
            .iter()
            .any(|r| matches!(r.event, Event::ToolCall { .. }))
    });
    // The call is logged before it runs; wait until it has really started.
    let deadline = Instant::now() + Duration::from_secs(10);
    while !fx.work.join("started").exists() {
        assert!(Instant::now() < deadline, "the command never started");
        std::thread::sleep(Duration::from_millis(10));
    }
    child.kill().unwrap();
    child.wait().unwrap();

    let resume = fx.script("resume.jsonl", vec![ScriptedResponse::text("Checked.")]);
    let output = fx
        .zcode(&resume, 0, &["--session", id.as_str(), "What happened?"])
        .output()
        .unwrap();
    assert_success(&output);

    let records = fx.store().read(&id).unwrap();
    assert!(records.iter().any(|record| record.event
        == Event::ToolResult {
            call_id: "c1".into(),
            content: vec![Block::text(INTERRUPTED_RUNNING)],
            is_error: true,
        }));
    assert!(plan_recovery(&records).is_empty());
}

#[test]
fn a_missing_api_key_is_reported_before_any_session_is_created() {
    let fx = Fixture::new();
    let output = Command::new(env!("CARGO_BIN_EXE_zcode"))
        .arg("--home")
        .arg(&fx.home)
        .args(["run", "Hello"])
        .current_dir(&fx.work)
        .env_remove("ZCODE_SCRIPT")
        .env_remove("ZCODE_API_KEY")
        .env_remove("DEEPSEEK_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("CLAUDE_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .stdin(Stdio::null())
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(text(&output.stderr).contains("no API key"));
    assert!(fx.store().list().unwrap().is_empty());
}
