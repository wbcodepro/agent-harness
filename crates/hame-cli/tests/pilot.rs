//! The pilot task set's checkers, and `hame eval` end to end with a
//! scripted model: timing, outcomes, traces and arm comparison.
#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use hame_eval::{Recording, ScriptedResponse};
use hame_session::{Delta, StopReason, Usage};
use serde_json::{Value, json};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn python() -> bool {
    Command::new("python3")
        .arg("-c")
        .arg("import tomllib")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// The variables `hame eval` gives a task's commands, for one workspace.
struct TaskEnv {
    vars: Vec<(&'static str, PathBuf)>,
    url: Option<String>,
    token: String,
    _server: Option<std::process::Child>,
    _dir: tempfile::TempDir,
}

impl TaskEnv {
    /// Starts the task's fixture server, if it has one, as the runner does.
    fn new(task: &hame_eval::manifest::TaskSpec) -> Self {
        use std::io::BufRead;
        let dir = tempfile::tempdir().unwrap();
        let (run, home, outbox) = (
            dir.path().join("run"),
            dir.path().join("home"),
            dir.path().join("outbox"),
        );
        for path in [&run, &home, &outbox] {
            fs::create_dir_all(path).unwrap();
        }
        let token = format!("token-{}", task.id);
        let mut vars = vec![
            ("HAME_SUITE_DIR", task.hidden_root.canonicalize().unwrap()),
            ("HAME_TASK_DIR", task.dir.canonicalize().unwrap()),
            ("HAME_RUN_DIR", run),
            ("HAME_AGENT_HOME", home),
            ("HAME_OUTBOX", outbox),
        ];
        if let Some(check_dir) = &task.check_dir {
            vars.push(("HAME_CHECK_DIR", check_dir.clone()));
        }
        let (mut server, mut url) = (None, None);
        if let Some(command) = &task.server {
            let mut child = Command::new("bash")
                .arg("-c")
                .arg(command)
                .envs(vars.iter().map(|(k, v)| (*k, v)))
                .env("HAME_FIXTURE_TOKEN", &token)
                // The fixture's deliberate delays, shortened for this test.
                .env("PILOT_DELAY_SCALE", "0.05")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            let mut line = String::new();
            std::io::BufReader::new(child.stdout.take().unwrap())
                .read_line(&mut line)
                .unwrap();
            let port = line.trim().strip_prefix("PORT ").expect("PORT line");
            url = Some(format!("http://127.0.0.1:{port}"));
            server = Some(child);
        }
        Self {
            vars,
            url,
            token,
            _server: server,
            _dir: dir,
        }
    }

    fn run(&self, command: &mut Command, work: &Path) -> std::process::Output {
        command
            .current_dir(work)
            .envs(self.vars.iter().map(|(k, v)| (*k, v)))
            .env("HAME_FIXTURE_TOKEN", &self.token)
            .env("HAME_WORKSPACE", work);
        if let Some(url) = &self.url {
            command.env("HAME_FIXTURE_URL", url);
        }
        command.output().unwrap()
    }
}

impl Drop for TaskEnv {
    fn drop(&mut self) {
        if let Some(server) = &mut self._server {
            let _ = server.kill();
            let _ = server.wait();
        }
    }
}

fn check(task: &hame_eval::manifest::TaskSpec, env: &TaskEnv, work: &Path) -> (bool, String) {
    let output = env.run(Command::new("bash").arg("-c").arg(&task.check), work);
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
    )
}

fn playwright() -> bool {
    Command::new("python3")
        .args(["-c", "import playwright.sync_api"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[test]
fn every_pilot_checker_fails_the_fixture_and_passes_the_reference_solution() {
    if !python() {
        eprintln!("skipped: needs python3 with tomllib");
        return;
    }
    let set = hame_eval::manifest::load(&repo().join("evals/pilot"), None).unwrap();
    assert_eq!(set.name, "pilot");
    let ids: Vec<&str> = set.tasks.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "B01", "B02", "B03", "B04", "B05", "D01", "D03", "D04", "F01", "F02", "F03", "F04",
            "F05", "R01", "R02", "R03", "R04", "R05", "X01"
        ]
    );
    for task in &set.tasks {
        assert!(
            task.prompts.contains_key("vi"),
            "{} has a Vietnamese prompt",
            task.id
        );
        let check_dir = task.check_dir.as_ref().expect("a hidden checker");
        assert!(check_dir.join("check.py").is_file(), "{}", task.id);
        if task.category == "file" {
            assert!(!task.unchanged.is_empty());
        }
        // Desktop tasks need TextEdit and Accessibility; they are checked
        // by real runs, not here.
        if task.requires.iter().any(|r| r == "desktop") {
            assert!(task.teardown.is_some(), "{} puts the desktop back", task.id);
            continue;
        }
        // The profile check opens Chrome through Playwright.
        if task.id == "B05" && !playwright() {
            eprintln!("B05 skipped: needs Playwright for Python");
            continue;
        }
        let solution = task.solution.as_ref().expect("a reference solution");
        // Nothing hidden is inside the fixture the agent gets.
        if let Some(fixture) = &task.fixture {
            assert!(!check_dir.starts_with(fixture) && !solution.starts_with(fixture));
        }
        for attempt in ["untouched", "solved"] {
            let env = TaskEnv::new(task);
            let dir = tempfile::tempdir().unwrap();
            let work = dir.path().join(attempt);
            fs::create_dir_all(&work).unwrap();
            if let Some(fixture) = &task.fixture {
                copy_tree(fixture, &work);
            }
            if let Some(setup) = &task.setup {
                let out = env.run(Command::new("bash").arg("-c").arg(setup), &work);
                assert!(out.status.success(), "{}: setup failed: {out:?}", task.id);
            }
            if attempt == "solved" {
                let out = env.run(
                    Command::new("python3").arg(solution.join("solve.py")),
                    &work,
                );
                assert!(
                    out.status.success(),
                    "{}: the solution did not run: {out:?}",
                    task.id
                );
            }
            let (passed, said) = check(task, &env, &work);
            assert_eq!(
                passed,
                attempt == "solved",
                "{}: the checker's verdict on the {attempt} workspace: {said}",
                task.id
            );
        }
    }
}

fn bash_call(command: &str, usage: Usage) -> ScriptedResponse {
    ScriptedResponse {
        deltas: vec![
            Delta::ToolUseStart {
                id: "c1".into(),
                name: "bash".into(),
            },
            Delta::ToolInput {
                json: json!({ "command": command }).to_string(),
            },
        ],
        stop: StopReason::ToolUse,
        usage,
    }
}

const MANIFEST: &str = r#"
task_set = "e2e"
version = "test.1"

[[task]]
id = "ok"
prompt = "Make done.txt."
check = "sleep 0.3; test -f done.txt"
unchanged = ["keep.txt"]

[[task]]
id = "slow"
prompt = "Make done.txt, slowly."
check = "true"
timeout_secs = 2

[[task]]
id = "wrong"
prompt = "Make done.txt; the checker will disagree."
check = "false"

[[task]]
id = "web"
category = "office"
prompt = "Fill in a spreadsheet."
channels = ["office"]
requires = ["office"]
check = "true"
"#;

#[test]
fn eval_records_every_attempt_with_verification_in_its_time_and_compares_arms() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let suite = root.join("suite");
    fs::create_dir_all(suite.join("ok/files")).unwrap();
    fs::create_dir_all(suite.join("slow/files")).unwrap();
    fs::write(suite.join("manifest.toml"), MANIFEST).unwrap();
    fs::write(suite.join("ok/files/keep.txt"), "keep me").unwrap();
    fs::write(suite.join("slow/files/slow.marker"), "").unwrap();

    // Every task's agent replays the same two responses: one shell call,
    // then an answer. The slow task's fixture makes the call hang.
    let script = root.join("script.jsonl");
    Recording {
        responses: vec![
            bash_call(
                "if [ -f slow.marker ]; then sleep 30; fi; echo ok > done.txt",
                Usage {
                    input_tokens: 100,
                    output_tokens: 7,
                    cache_read_tokens: 20,
                    cache_write_tokens: 5,
                },
            ),
            ScriptedResponse {
                deltas: vec![Delta::Text {
                    text: "Done: done.txt is written.".into(),
                }],
                stop: StopReason::EndTurn,
                usage: Usage {
                    input_tokens: 50,
                    output_tokens: 3,
                    ..Usage::default()
                },
            },
        ],
    }
    .save(&script)
    .unwrap();

    let report = root.join("out/report.json");
    let output = Command::new(env!("CARGO_BIN_EXE_hame"))
        .arg("--home")
        .arg(root.join("home"))
        .arg("eval")
        .arg(&suite)
        .args(["--arm", "base", "--arm", "lean"])
        .args(["--set", "sandbox.mode=danger-full-access"])
        .args(["--set", "lean:limits.max_steps=5"])
        .args(["--seed", "3", "--bootstrap", "200"])
        .arg("--json")
        .arg(&report)
        .current_dir(&root)
        .env("HAME_SCRIPT", &script)
        .env_remove("HAME_TRACE")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "not every task passed");
    let report: Value = serde_json::from_str(
        &fs::read_to_string(&report).unwrap_or_else(|e| panic!("{e}\n{stdout}\n{stderr}")),
    )
    .unwrap();
    assert_eq!(report["schema"], "hame.eval.v2");
    assert_eq!(report["task_set_version"], "test.1");
    let records = report["records"].as_array().unwrap();
    assert_eq!(records.len(), 8, "4 tasks x 2 arms\n{stderr}");
    let find = |arm: &str, task: &str| {
        records
            .iter()
            .find(|r| r["arm"] == arm && r["task_id"] == task)
            .unwrap_or_else(|| panic!("no record for {arm}/{task}"))
    };

    for arm in ["base", "lean"] {
        let ok = find(arm, "ok");
        assert_eq!(ok["outcome"], "pass", "{ok:#}\n{stderr}");
        let (agent, verify, total) = (
            ok["agent_ms"].as_f64().unwrap(),
            ok["verification_ms"].as_f64().unwrap(),
            ok["total_ms"].as_f64().unwrap(),
        );
        assert!(verify >= 300.0, "the checker's time is measured: {verify}");
        assert!(
            (total - (agent + verify)).abs() < 0.01,
            "total includes verification"
        );
        assert!(ok["setup_ms"].as_f64().unwrap() >= 0.0);
        // Usage from the trace equals the sum of the requests.
        assert_eq!(ok["model_calls"], 2);
        assert_eq!(ok["attempts"], 2);
        assert_eq!(ok["uncached_input"], 150);
        assert_eq!(ok["cached_input"], 20);
        assert_eq!(ok["cache_write"], 5);
        assert_eq!(ok["output"], 10);
        assert_eq!(ok["usage_complete"], true);
        assert_eq!(ok["tool_calls"], 1);
        assert!(ok["billed_cost"].is_null(), "a scripted model has no price");
        assert!(ok["request_bytes"].is_null(), "unknown, not zero");
        assert!(ok["artifact_hashes"]["done.txt"].is_string());
        assert_eq!(ok["forbidden_effects"], json!([]));
        assert!(
            ok["initial_state_id"]
                .as_str()
                .unwrap()
                .starts_with("sha256:")
        );
        assert!(
            ok["prompt_version"]
                .as_str()
                .unwrap()
                .starts_with("sha256:")
        );
        assert!(ok["stage_spans"]["turn"].as_f64().unwrap() > 0.0);
        assert!(ok["stage_spans"]["model_request"].as_f64().is_some());
        let trace = ok["trace_reference"].as_str().unwrap();
        let spans = hame_trace::read_trace(Path::new(trace)).unwrap();
        assert!(spans.len() > 4);

        let slow = find(arm, "slow");
        assert_eq!(slow["outcome"], "timeout", "{slow:#}");
        assert_eq!(slow["failure_category"], "timeout");
        let agent = slow["agent_ms"].as_f64().unwrap();
        assert!((2000.0..20_000.0).contains(&agent), "{agent}");

        let wrong = find(arm, "wrong");
        assert_eq!(wrong["outcome"], "false_success", "{wrong:#}");
        assert_eq!(wrong["claimed_success"], true);
        assert_eq!(wrong["check_passed"], false);

        assert_eq!(find(arm, "web")["outcome"], "unsupported");
    }
    assert_eq!(
        find("lean", "ok")["enabled_features"]["limits.max_steps"],
        5
    );

    let summaries = report["summaries"].as_array().unwrap();
    assert_eq!(summaries.len(), 2);
    for summary in summaries {
        assert_eq!(summary["attempts"], 3, "unsupported is not counted");
        assert_eq!(summary["passes"], 1);
        assert_eq!(summary["false_successes"], 1);
        assert_eq!(summary["timeouts"], 1);
        assert_eq!(summary["unsupported"], 1);
        assert!(summary["cost_usd"].is_null());
        assert!(summary["cost_per_success"].is_null());
        assert!(summary["verified_latency_ms_p50"].as_f64().unwrap() >= 300.0);
    }
    let comparisons = report["comparisons"].as_array().unwrap();
    let success = comparisons
        .iter()
        .find(|c| c["metric"] == "strict_success")
        .unwrap();
    assert_eq!(success["baseline"], "base");
    assert_eq!(success["arm"], "lean");
    assert_eq!(success["tasks"], 3);
    assert_eq!(success["mean_diff"], 0.0);
    // The first report format is still there.
    assert_eq!(report["total"], 6);
    assert_eq!(report["passed"], 2);
    assert!(stdout.contains("strict completion 1/3"), "{stdout}");
}

const LIVE_MANIFEST: &str = r#"
task_set = "live"
version = "test.1"

[[task]]
id = "crash"
prompt = "Fetch {fixture_url} and write done.txt; save copies in {outbox}."
server = "python3 \"$HAME_TASK_DIR/server.py\""
setup = "test -n \"$HAME_FIXTURE_URL\" && touch \"$HAME_RUN_DIR/setup.ran\""
# Kills the agent once it is running, as a crash would, and leaves the
# marker that asks for the session to be continued.
background = "while [ ! -f \"$HAME_RUN_DIR/agent.pid\" ]; do sleep 0.1; done; sleep 0.5; touch \"$HAME_WORKSPACE/killed\" \"$HAME_RUN_DIR/agent.killed\"; kill -9 $(cat \"$HAME_RUN_DIR/agent.pid\")"
resume = "Continue."
check = "test -f done.txt && test -f \"$HAME_RUN_DIR/setup.ran\" && python3 -c \"import os,urllib.request; assert urllib.request.urlopen(os.environ['HAME_FIXTURE_URL'] + '/?token=' + os.environ['HAME_FIXTURE_TOKEN']).read() == b'ok'\" && test -d \"$HAME_OUTBOX\""
teardown = "touch \"$PILOT_TEST_OUT/teardown.ran\""
config = { limits = { max_steps = 7 } }
"#;

const SERVER: &str = r#"
import os, sys, threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
class H(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_GET(self):
        body = b"ok" if os.environ["HAME_FIXTURE_TOKEN"] in self.path else b"no"
        self.send_response(200); self.send_header("Content-Length", str(len(body))); self.end_headers()
        self.wfile.write(body)
s = ThreadingHTTPServer(("127.0.0.1", 0), H)
print(f"PORT {s.server_address[1]}", flush=True)
threading.Thread(target=s.serve_forever, daemon=True).start()
sys.stdin.read()
open(os.path.join(os.environ["PILOT_TEST_OUT"], "server.stopped"), "w").write("1")
"#;

#[test]
fn live_fixtures_faults_and_resumption_run_around_the_agent() {
    if !python() {
        eprintln!("skipped: needs python3");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let suite = root.join("suite");
    let out = root.join("out");
    fs::create_dir_all(suite.join("crash")).unwrap();
    fs::create_dir_all(&out).unwrap();
    fs::write(suite.join("manifest.toml"), LIVE_MANIFEST).unwrap();
    fs::write(suite.join("crash/server.py"), SERVER).unwrap();
    // The first agent hangs until it is killed; the continued session sees
    // the marker and finishes.
    let script = root.join("script.jsonl");
    Recording {
        responses: vec![
            bash_call(
                "if [ ! -f killed ]; then sleep 30; fi; echo ok > done.txt",
                Usage::default(),
            ),
            ScriptedResponse {
                deltas: vec![Delta::Text {
                    text: "Done.".into(),
                }],
                stop: StopReason::EndTurn,
                usage: Usage::default(),
            },
        ],
    }
    .save(&script)
    .unwrap();
    let report = root.join("report.json");
    let output = Command::new(env!("CARGO_BIN_EXE_hame"))
        .arg("--home")
        .arg(root.join("home"))
        .arg("eval")
        .arg(&suite)
        .args(["--set", "sandbox.mode=danger-full-access"])
        .arg("--json")
        .arg(&report)
        .current_dir(&root)
        .env("HAME_SCRIPT", &script)
        .env("PILOT_TEST_OUT", &out)
        .env_remove("HAME_TRACE")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report: Value = serde_json::from_str(
        &fs::read_to_string(&report).unwrap_or_else(|e| panic!("{e}\n{stderr}")),
    )
    .unwrap();
    let record = &report["records"][0];
    assert_eq!(record["outcome"], "pass", "{record:#}\n{stderr}");
    assert_eq!(
        record["human_interventions"], 1,
        "the session was continued"
    );
    assert!(
        record["note"].as_str().unwrap().contains("resumed"),
        "{record:#}"
    );
    assert_eq!(record["network_profile"], "loopback");
    assert!(out.join("teardown.ran").is_file(), "teardown ran");
    assert!(
        out.join("server.stopped").is_file(),
        "the server was stopped"
    );
}
