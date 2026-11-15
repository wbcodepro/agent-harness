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

fn check(task: &hame_eval::manifest::TaskSpec, work: &Path) -> (bool, String) {
    let output = Command::new("bash")
        .arg("-c")
        .arg(&task.check)
        .current_dir(work)
        .env("HAME_CHECK_DIR", task.check_dir.as_ref().unwrap())
        .output()
        .unwrap();
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
    )
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
    assert_eq!(ids, ["F01", "F02", "F03", "F04", "F05"]);
    for task in &set.tasks {
        assert!(
            task.prompts.contains_key("vi"),
            "{} has a Vietnamese prompt",
            task.id
        );
        assert!(!task.unchanged.is_empty());
        let check_dir = task.check_dir.as_ref().expect("a hidden checker");
        let solution = task.solution.as_ref().expect("a reference solution");
        // Nothing hidden is inside the fixture the agent gets.
        let fixture = task.fixture.as_ref().unwrap();
        assert!(!check_dir.starts_with(fixture) && !solution.starts_with(fixture));

        let dir = tempfile::tempdir().unwrap();
        let untouched = dir.path().join("untouched");
        copy_tree(fixture, &untouched);
        let (passed, said) = check(task, &untouched);
        assert!(
            !passed,
            "{}: the checker passes the unmodified fixture: {said}",
            task.id
        );

        let solved = dir.path().join("solved");
        copy_tree(fixture, &solved);
        let status = Command::new("python3")
            .arg(solution.join("solve.py"))
            .current_dir(&solved)
            .status()
            .unwrap();
        assert!(status.success(), "{}: the solution did not run", task.id);
        let (passed, said) = check(task, &solved);
        assert!(
            passed,
            "{}: the checker rejects the reference solution: {said}",
            task.id
        );
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
category = "browser"
prompt = "Fill in a form."
channels = ["browser"]
requires = ["browser"]
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
