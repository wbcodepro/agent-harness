//! Outcome verification: checks a task's acceptance predicates against the
//! authoritative state, independently of what the acting model claims.
//!
//! File predicates are checked here directly. Browser and desktop
//! predicates need a live adapter; the caller passes a [`StateProbe`] that
//! asks one. A predicate that cannot be checked fails with the reason as
//! evidence: "could not check" is never reported as success.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use hame_agent::TurnHook;
use hame_kernel::BoxFuture;
use hame_session::contract::{Predicate, PredicateResult, TaskContract, VerificationReport};
use hame_session::{Block, Event, Record};
use serde_json::Value;

/// Starts every message the outcome gate sends to the model.
pub const OUTCOME_PREFIX: &str = "[hame outcome]";

/// How long a `command` predicate may run.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(120);

/// What a browser shows, for browser predicates.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BrowserSnapshot {
    pub url: String,
    pub text: String,
    pub profile: Option<String>,
}

/// What a native application shows, for desktop predicates.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DesktopSnapshot {
    pub window_titles: Vec<String>,
    /// Values of text elements in the application's windows.
    pub values: Vec<String>,
}

/// A command line rewritten to run confined.
#[derive(Debug, Clone)]
pub struct ConfinedCommand {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    /// The whole environment of the command; `None` inherits hame's.
    pub env: Option<Vec<(OsString, OsString)>>,
}

/// Rewrites a command line (`program`, `args`) so that it runs confined;
/// `Err` refuses to run it. `command` predicates run through it: the agent
/// can change the files such a command executes (a test, a Makefile), so the
/// check must not run them with more access than the agent's own tools have.
pub type Confinement =
    Arc<dyn Fn(&Path, &[OsString]) -> Result<ConfinedCommand, String> + Send + Sync>;

/// Reads live application state for predicates that need it.
pub trait StateProbe: Send + Sync {
    fn browser<'a>(
        &'a self,
        profile: Option<&'a str>,
    ) -> BoxFuture<'a, Result<BrowserSnapshot, String>>;
    fn desktop<'a>(&'a self, app: &'a str) -> BoxFuture<'a, Result<DesktopSnapshot, String>>;
}

fn resolve(cwd: &Path, path: &str) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

fn clip(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// Splits CSV text into records of fields, honouring double quotes: a
/// quoted field may hold commas and line breaks, and `""` inside it is a
/// quote. Empty physical lines are dropped, but quoted empty fields and
/// whitespace remain records. A quote that is never closed, or one in the
/// middle of an unquoted field, is an error.
fn csv_records(text: &str) -> Result<Vec<Vec<String>>, String> {
    let mut records = Vec::new();
    let mut record = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    // The current field began with a quote (and so may only end with one).
    let mut was_quoted = false;
    let mut record_started = false;
    let mut line = 1;
    let mut chars = text.chars().peekable();
    let mut end_record = |record: &mut Vec<String>, field: &mut String, started: &mut bool| {
        record.push(std::mem::take(field));
        let fields = std::mem::take(record);
        if std::mem::take(started) {
            records.push(fields);
        }
    };
    while let Some(c) = chars.next() {
        record_started |= quoted || !matches!(c, '\r' | '\n');
        match (c, quoted) {
            ('"', true) if chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            ('"', true) => {
                quoted = false;
                if !matches!(chars.peek(), None | Some(',' | '\n' | '\r')) {
                    return Err(format!("line {line}: text after a closing quote"));
                }
            }
            ('"', false) if field.is_empty() && !was_quoted => {
                quoted = true;
                was_quoted = true;
            }
            ('"', false) => return Err(format!("line {line}: a quote inside an unquoted field")),
            (',', false) => {
                record.push(std::mem::take(&mut field));
                was_quoted = false;
            }
            ('\r', false) if chars.peek() == Some(&'\n') => {}
            ('\n' | '\r', false) => {
                end_record(&mut record, &mut field, &mut record_started);
                was_quoted = false;
                line += 1;
            }
            (c, _) => {
                if c == '\n' {
                    line += 1;
                }
                field.push(c);
            }
        }
    }
    if quoted {
        return Err(format!("line {line}: a quoted field is never closed"));
    }
    end_record(&mut record, &mut field, &mut record_started);
    Ok(records)
}

/// Reads a file a predicate names. In a confined session it is read
/// through the confinement, so a path the agent's tools cannot read (or a
/// link the agent made to one) is not read for them either and its content
/// cannot reach the model as evidence.
async fn read_checked(
    cwd: &Path,
    path: &str,
    confine: Option<&Confinement>,
) -> Result<Vec<u8>, String> {
    let full = resolve(cwd, path);
    let Some(confine) = confine.filter(|_| cfg!(unix)) else {
        return std::fs::read(&full).map_err(|e| e.to_string());
    };
    let confined = confine(
        Path::new("/bin/cat"),
        &[OsString::from("--"), full.into_os_string()],
    )?;
    let mut cmd = tokio::process::Command::new(confined.program);
    cmd.args(confined.args);
    if let Some(env) = confined.env {
        cmd.env_clear().envs(env);
    }
    cmd.current_dir(cwd).kill_on_drop(true);
    match tokio::time::timeout(COMMAND_TIMEOUT, cmd.output()).await {
        Ok(Ok(output)) if output.status.success() => Ok(output.stdout),
        Ok(Ok(output)) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let reason = stderr.trim().rsplit(": ").next().unwrap_or("").trim();
            Err(if reason.is_empty() {
                "not readable".into()
            } else {
                reason.to_string()
            })
        }
        Ok(Err(e)) => Err(format!("cannot read confined: {e}")),
        Err(_) => Err(format!("timed out after {COMMAND_TIMEOUT:?}")),
    }
}

async fn check_one(
    predicate: &Predicate,
    cwd: &Path,
    probe: Option<&dyn StateProbe>,
    confine: Option<&Confinement>,
) -> (bool, String) {
    if let Err(e) = predicate.validate() {
        return (false, format!("invalid condition: {e}"));
    }
    match predicate {
        Predicate::FileExists { path } => {
            let full = resolve(cwd, path);
            match std::fs::metadata(&full) {
                Ok(meta) => (true, format!("exists, {} bytes", meta.len())),
                Err(e) => (false, format!("{}: {e}", full.display())),
            }
        }
        Predicate::FileAbsent { path } => {
            let full = resolve(cwd, path);
            // Only "not found" is absence; a path that cannot be looked at
            // is not known to be absent. A link counts as present.
            match std::fs::symlink_metadata(&full) {
                Ok(_) => (false, format!("{} exists", full.display())),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (true, "absent".into()),
                Err(e) => (false, format!("cannot check {}: {e}", full.display())),
            }
        }
        Predicate::FileContains { path, text } => match read_checked(cwd, path, confine).await {
            Ok(bytes) => {
                let content = String::from_utf8_lossy(&bytes);
                if content.contains(text.as_str()) {
                    (true, format!("found {:?}", clip(text, 80)))
                } else {
                    (
                        false,
                        format!(
                            "{:?} not found; file starts {:?}",
                            clip(text, 80),
                            clip(&content, 160)
                        ),
                    )
                }
            }
            Err(e) => (false, format!("cannot read {path}: {e}")),
        },
        Predicate::FileSha256 { path, sha256 } => match read_checked(cwd, path, confine).await {
            Ok(bytes) => {
                let actual = hame_session::artifact::sha256_hex(&bytes);
                (
                    actual.eq_ignore_ascii_case(sha256),
                    format!("sha256 {actual}"),
                )
            }
            Err(e) => (false, format!("cannot read {path}: {e}")),
        },
        Predicate::JsonValue {
            path,
            pointer,
            equals,
        } => {
            let parsed = read_checked(cwd, path, confine).await.and_then(|bytes| {
                serde_json::from_slice::<Value>(&bytes).map_err(|e| e.to_string())
            });
            match parsed {
                Ok(value) => match value.pointer(pointer) {
                    Some(found) => (found == equals, format!("{pointer} = {found}")),
                    None => (false, format!("{pointer} is absent")),
                },
                Err(e) => (false, format!("cannot parse {path}: {e}")),
            }
        }
        Predicate::CsvShape {
            path,
            rows,
            columns,
        } => match read_checked(cwd, path, confine)
            .await
            .and_then(|bytes| String::from_utf8(bytes).map_err(|_| "not UTF-8 text".to_string()))
        {
            Ok(text) => {
                let records = match csv_records(&text) {
                    Ok(records) => records,
                    Err(e) => return (false, format!("{path} is not valid CSV: {e}")),
                };
                let mut records = records.into_iter();
                let header = records.next().unwrap_or_default();
                let records: Vec<Vec<String>> = records.collect();
                let count = records.len();
                // Every record must have the header's fields: a short or
                // long row is not the declared shape.
                if let Some((i, bad)) = records
                    .iter()
                    .enumerate()
                    .find(|(_, r)| r.len() != header.len())
                {
                    return (
                        false,
                        format!(
                            "data record {} has {} fields, the header {}",
                            i + 1,
                            bad.len(),
                            header.len()
                        ),
                    );
                }
                let columns_ok = columns.is_empty()
                    || columns
                        .iter()
                        .all(|c| header.iter().any(|h| h.trim() == c.as_str()));
                let rows_ok = rows.is_none_or(|n| n == count);
                (
                    columns_ok && rows_ok,
                    format!("{count} data rows; header {header:?}"),
                )
            }
            Err(e) => (false, format!("cannot read {path}: {e}")),
        },
        Predicate::Command { command, exit } => {
            let (program, args): (PathBuf, Vec<OsString>) = if cfg!(windows) {
                ("cmd".into(), vec!["/C".into(), command.into()])
            } else {
                ("/bin/sh".into(), vec!["-c".into(), command.into()])
            };
            let confined = match confine {
                Some(confine) => match confine(&program, &args) {
                    Ok(confined) => confined,
                    Err(e) => return (false, format!("cannot run confined: {e}")),
                },
                None => ConfinedCommand {
                    program,
                    args,
                    env: None,
                },
            };
            let mut cmd = tokio::process::Command::new(confined.program);
            cmd.args(confined.args);
            if let Some(env) = confined.env {
                cmd.env_clear().envs(env);
            }
            cmd.current_dir(cwd).kill_on_drop(true);
            match tokio::time::timeout(COMMAND_TIMEOUT, cmd.output()).await {
                Ok(Ok(output)) => {
                    let code = output.status.code().unwrap_or(-1);
                    let mut evidence = format!("exit {code}");
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    let tail = if stderr.trim().is_empty() {
                        stdout
                    } else {
                        stderr
                    };
                    if code != *exit && !tail.trim().is_empty() {
                        evidence.push_str(&format!(": {}", clip(tail.trim(), 400)));
                    }
                    (code == *exit, evidence)
                }
                Ok(Err(e)) => (false, format!("cannot run: {e}")),
                Err(_) => (false, format!("timed out after {COMMAND_TIMEOUT:?}")),
            }
        }
        Predicate::BrowserState {
            url_contains,
            text_contains,
            profile,
        } => {
            let Some(probe) = probe else {
                return (false, "no browser available to check this".into());
            };
            match probe.browser(profile.as_deref()).await {
                Ok(snapshot) => {
                    let mut ok = true;
                    let mut evidence = format!("url {}", snapshot.url);
                    if let Some(want) = profile {
                        match &snapshot.profile {
                            Some(have) if have == want => {}
                            Some(have) => {
                                ok = false;
                                evidence.push_str(&format!("; profile {have}, expected {want}"));
                            }
                            // A required profile that cannot be confirmed
                            // is not confirmed.
                            None => {
                                ok = false;
                                evidence
                                    .push_str(&format!("; no profile reported, expected {want}"));
                            }
                        }
                    }
                    if let Some(part) = url_contains {
                        ok &= snapshot.url.contains(part.as_str());
                    }
                    if let Some(part) = text_contains {
                        let found = snapshot.text.contains(part.as_str());
                        ok &= found;
                        evidence.push_str(&format!(
                            "; text {} {:?}",
                            if found { "contains" } else { "lacks" },
                            clip(part, 80)
                        ));
                    }
                    (ok, evidence)
                }
                Err(e) => (false, format!("browser check failed: {e}")),
            }
        }
        Predicate::DesktopState {
            app,
            window_title_contains,
            value_contains,
        } => {
            let Some(probe) = probe else {
                return (false, "no desktop adapter available to check this".into());
            };
            match probe.desktop(app).await {
                Ok(snapshot) => {
                    let mut ok = true;
                    if let Some(part) = window_title_contains {
                        ok &= snapshot
                            .window_titles
                            .iter()
                            .any(|title| title.contains(part.as_str()));
                    }
                    if let Some(part) = value_contains {
                        ok &= snapshot
                            .values
                            .iter()
                            .any(|value| value.contains(part.as_str()));
                    }
                    (ok, format!("windows {:?}", snapshot.window_titles))
                }
                Err(e) => (false, format!("desktop check failed: {e}")),
            }
        }
    }
}

/// Checks every acceptance predicate of `task`. With no predicates the
/// report fails: a task with nothing to check cannot be verified. An
/// invalid predicate (see [`Predicate::validate`]) fails. `command`
/// predicates run through `confine` when given, unconfined otherwise.
pub async fn verify(
    task: &TaskContract,
    cwd: &Path,
    probe: Option<&dyn StateProbe>,
    confine: Option<&Confinement>,
) -> VerificationReport {
    let started = Instant::now();
    let mut results = Vec::with_capacity(task.acceptance.len());
    for predicate in &task.acceptance {
        let (passed, evidence) = check_one(predicate, cwd, probe, confine).await;
        results.push(PredicateResult {
            predicate: predicate.clone(),
            passed,
            evidence,
        });
    }
    VerificationReport {
        task_id: task.id.clone(),
        passed: !results.is_empty() && results.iter().all(|r| r.passed),
        results,
        ms: started.elapsed().as_millis() as u64,
    }
}

/// The task contract in force for a session: the last `task/start`.
pub fn current_task(records: &[Record]) -> Option<TaskContract> {
    records.iter().rev().find_map(|record| match &record.event {
        Event::TaskStart { contract } => Some((**contract).clone()),
        _ => None,
    })
}

/// Renders a report for the model or the user.
pub fn describe(report: &VerificationReport) -> String {
    let mut out = format!(
        "Task {}: {} ({} of {} checks passed, {} ms)",
        report.task_id,
        if report.passed {
            "verified"
        } else {
            "NOT verified"
        },
        report.results.iter().filter(|r| r.passed).count(),
        report.results.len(),
        report.ms
    );
    for result in &report.results {
        let predicate = serde_json::to_string(&result.predicate).unwrap_or_default();
        out.push_str(&format!(
            "\n- [{}] {predicate} — {}",
            if result.passed { "pass" } else { "FAIL" },
            result.evidence
        ));
    }
    out
}

/// Holds a turn open until the session's task passes its acceptance
/// checks, up to `max_rounds` times; then lets it end, and the final
/// report (appended by the caller) records the failure.
pub struct OutcomeGate {
    pub cwd: PathBuf,
    pub probe: Option<Arc<dyn StateProbe>>,
    /// Confines `command` predicates; see [`Confinement`].
    pub confine: Option<Confinement>,
    pub max_rounds: usize,
}

impl TurnHook for OutcomeGate {
    fn gates_completion(&self) -> bool {
        true
    }

    fn before_stop<'a>(
        &'a self,
        records: &'a [Record],
        _changed: bool,
    ) -> BoxFuture<'a, Option<Vec<Block>>> {
        Box::pin(async move {
            let task = current_task(records)?;
            let rounds = records
                .iter()
                .rev()
                .take_while(|record| !matches!(record.event, Event::TurnStart { .. }))
                .filter(|record| {
                    matches!(&record.event, Event::UserMessage { content }
                        if matches!(content.first(), Some(Block::Text { text }) if text.starts_with(OUTCOME_PREFIX)))
                })
                .count();
            if rounds >= self.max_rounds {
                return None;
            }
            let report = verify(
                &task,
                &self.cwd,
                self.probe.as_deref(),
                self.confine.as_ref(),
            )
            .await;
            if report.passed {
                return None;
            }
            Some(vec![Block::text(format!(
                "{OUTCOME_PREFIX} The task's acceptance checks do not pass yet. These checks \
                 come from the user's request and are evaluated against the real state, not \
                 your description of it.\n{}\n\nFix what fails and finish again. If something \
                 cannot be done, say exactly what blocks it instead of ending as if it were done.",
                describe(&report)
            ))])
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hame_session::contract::{Authorization, Budget, CONTRACT_VERSION};
    use serde_json::json;

    fn task(acceptance: Vec<Predicate>) -> TaskContract {
        TaskContract {
            version: CONTRACT_VERSION,
            id: "t1".into(),
            outcome: "o".into(),
            constraints: vec![],
            acceptance,
            authorization: Authorization::default(),
            budget: Budget::default(),
        }
    }

    struct FakeProbe;

    impl StateProbe for FakeProbe {
        fn browser<'a>(
            &'a self,
            profile: Option<&'a str>,
        ) -> BoxFuture<'a, Result<BrowserSnapshot, String>> {
            Box::pin(async move {
                Ok(BrowserSnapshot {
                    url: "http://127.0.0.1/prefs".into(),
                    text: "Theme: dark".into(),
                    profile: profile.map(|_| "a".into()),
                })
            })
        }

        fn desktop<'a>(&'a self, app: &'a str) -> BoxFuture<'a, Result<DesktopSnapshot, String>> {
            Box::pin(async move {
                if app == "TextEdit" {
                    Ok(DesktopSnapshot {
                        window_titles: vec!["report.rtf".into()],
                        values: vec!["Tiếng Việt".into()],
                    })
                } else {
                    Err("not running".into())
                }
            })
        }
    }

    #[tokio::test]
    async fn file_predicates_check_the_real_state() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("out.txt"), "Tổng: 42\n").unwrap();
        std::fs::write(dir.path().join("data.json"), r#"{"total": 42}"#).unwrap();
        std::fs::write(dir.path().join("t.csv"), "name,\"amount, usd\"\na,1\nb,2\n").unwrap();
        let sha = hame_session::artifact::sha256_hex("Tổng: 42\n".as_bytes());
        let ok = task(vec![
            Predicate::FileExists {
                path: "out.txt".into(),
            },
            Predicate::FileAbsent {
                path: "nope".into(),
            },
            Predicate::FileContains {
                path: "out.txt".into(),
                text: "Tổng: 42".into(),
            },
            Predicate::FileSha256 {
                path: "out.txt".into(),
                sha256: sha,
            },
            Predicate::JsonValue {
                path: "data.json".into(),
                pointer: "/total".into(),
                equals: json!(42),
            },
            Predicate::CsvShape {
                path: "t.csv".into(),
                rows: Some(2),
                columns: vec!["name".into(), "amount, usd".into()],
            },
            Predicate::Command {
                command: "test -f out.txt".into(),
                exit: 0,
            },
        ]);
        let report = verify(&ok, dir.path(), None, None).await;
        assert!(report.passed, "{}", describe(&report));

        let bad = task(vec![
            Predicate::FileContains {
                path: "out.txt".into(),
                text: "Tổng: 43".into(),
            },
            Predicate::Command {
                command: "exit 3".into(),
                exit: 0,
            },
        ]);
        let report = verify(&bad, dir.path(), None, None).await;
        assert!(!report.passed);
        assert!(report.results.iter().all(|r| !r.passed));
        assert!(report.results[1].evidence.starts_with("exit 3"));
        // Nothing to check is not success.
        assert!(!verify(&task(vec![]), dir.path(), None, None).await.passed);
    }

    #[tokio::test]
    async fn live_state_predicates_need_a_probe_and_never_pass_without_one() {
        let dir = tempfile::tempdir().unwrap();
        let t = task(vec![
            Predicate::BrowserState {
                url_contains: Some("/prefs".into()),
                text_contains: Some("Theme: dark".into()),
                profile: Some("a".into()),
            },
            Predicate::DesktopState {
                app: "TextEdit".into(),
                window_title_contains: Some("report".into()),
                value_contains: Some("Tiếng Việt".into()),
            },
        ]);
        assert!(!verify(&t, dir.path(), None, None).await.passed);
        let report = verify(&t, dir.path(), Some(&FakeProbe), None).await;
        assert!(report.passed, "{}", describe(&report));
        let wrong_profile = task(vec![Predicate::BrowserState {
            url_contains: None,
            text_contains: Some("Theme".into()),
            profile: Some("b".into()),
        }]);
        assert!(
            !verify(&wrong_profile, dir.path(), Some(&FakeProbe), None)
                .await
                .passed
        );
        let not_running = task(vec![Predicate::DesktopState {
            app: "Pages".into(),
            window_title_contains: Some("report".into()),
            value_contains: None,
        }]);
        assert!(
            !verify(&not_running, dir.path(), Some(&FakeProbe), None)
                .await
                .passed
        );
    }

    /// A probe whose browser reports no profile.
    struct NoProfile;

    impl StateProbe for NoProfile {
        fn browser<'a>(
            &'a self,
            _profile: Option<&'a str>,
        ) -> BoxFuture<'a, Result<BrowserSnapshot, String>> {
            Box::pin(async move {
                Ok(BrowserSnapshot {
                    url: "http://127.0.0.1/prefs".into(),
                    text: "Theme: dark".into(),
                    profile: None,
                })
            })
        }

        fn desktop<'a>(&'a self, _app: &'a str) -> BoxFuture<'a, Result<DesktopSnapshot, String>> {
            Box::pin(async move {
                Ok(DesktopSnapshot {
                    window_titles: vec!["anything".into()],
                    values: vec!["anything".into()],
                })
            })
        }
    }

    #[tokio::test]
    async fn conditions_that_check_nothing_never_pass() {
        let dir = tempfile::tempdir().unwrap();
        // Nothing to compare: the probe answering is not the outcome.
        for vacuous in [
            Predicate::BrowserState {
                url_contains: None,
                text_contains: None,
                profile: None,
            },
            Predicate::BrowserState {
                url_contains: None,
                text_contains: Some(String::new()),
                profile: None,
            },
            Predicate::DesktopState {
                app: "TextEdit".into(),
                window_title_contains: None,
                value_contains: None,
            },
            Predicate::FileContains {
                path: "x".into(),
                text: String::new(),
            },
            Predicate::Command {
                command: " ".into(),
                exit: 0,
            },
        ] {
            assert!(vacuous.validate().is_err(), "{vacuous:?}");
            let report = verify(&task(vec![vacuous]), dir.path(), Some(&NoProfile), None).await;
            assert!(!report.passed);
            assert!(report.results[0].evidence.starts_with("invalid condition"));
        }
        // A required profile the browser does not report is not confirmed.
        let wants_profile = task(vec![Predicate::BrowserState {
            url_contains: None,
            text_contains: Some("Theme".into()),
            profile: Some("work".into()),
        }]);
        let report = verify(&wants_profile, dir.path(), Some(&NoProfile), None).await;
        assert!(!report.passed);
        assert!(report.results[0].evidence.contains("no profile reported"));
        // A misspelt field is refused rather than ignored.
        let typo: Result<Predicate, _> = serde_json::from_value(json!({
            "kind": "browser_state", "text_contians": "Payment received"
        }));
        assert!(typo.is_err());
    }

    #[test]
    fn csv_records_keep_quoted_line_breaks_in_one_record() {
        let text = "name,note\r\n\"Ann\",\"line one\nline two, still\"\r\n\r\n";
        let records = csv_records(text).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[1], vec!["Ann", "line one\nline two, still"]);
        let quotes = csv_records("a\n\"say \"\"hi\"\"\"\n").unwrap();
        assert_eq!(quotes[1], vec!["say \"hi\""]);
        assert!(csv_records("a,b\n\"open,1\n").is_err());
        assert!(csv_records("a,b\nx\"y,1\n").is_err());
        assert!(csv_records("a,b\n\"x\"y,1\n").is_err());
    }

    #[tokio::test]
    async fn csv_shape_fails_on_ragged_or_malformed_records() {
        let dir = tempfile::tempdir().unwrap();
        let shape = || {
            task(vec![Predicate::CsvShape {
                path: "t.csv".into(),
                rows: Some(1),
                columns: vec!["a".into(), "b".into()],
            }])
        };
        for bad in ["a,b\n1\n", "a,b\n1,2,3\n", "a,b\n\"1,2\n"] {
            std::fs::write(dir.path().join("t.csv"), bad).unwrap();
            assert!(
                !verify(&shape(), dir.path(), None, None).await.passed,
                "{bad:?}"
            );
        }
        std::fs::write(dir.path().join("t.csv"), "a,b\r\n1,2\r\n").unwrap();
        assert!(verify(&shape(), dir.path(), None, None).await.passed);
    }

    #[tokio::test]
    async fn a_csv_record_with_a_line_break_counts_once() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("t.csv"), "a,b\n1,\"x\ny\"\n").unwrap();
        let shape = |rows| {
            task(vec![Predicate::CsvShape {
                path: "t.csv".into(),
                rows: Some(rows),
                columns: vec!["a".into(), "b".into()],
            }])
        };
        assert!(verify(&shape(1), dir.path(), None, None).await.passed);
        assert!(!verify(&shape(2), dir.path(), None, None).await.passed);
    }

    #[tokio::test]
    async fn csv_shape_counts_empty_quoted_and_whitespace_records() {
        let dir = tempfile::tempdir().unwrap();
        for text in [
            "note\n\"\"",
            "note\r\n\"\"\r\n",
            "note\n\" \"\n",
            "note\n \n",
        ] {
            std::fs::write(dir.path().join("t.csv"), text).unwrap();
            for rows in [0, 1] {
                let t = task(vec![Predicate::CsvShape {
                    path: "t.csv".into(),
                    rows: Some(rows),
                    columns: vec!["note".into()],
                }]);
                let report = verify(&t, dir.path(), None, None).await;
                assert_eq!(report.passed, rows == 1, "{text:?}: {}", describe(&report));
            }
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_path_that_cannot_be_looked_at_is_not_absent() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let locked = dir.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::write(locked.join("f"), "x").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let blocked = std::fs::symlink_metadata(locked.join("f")).is_err();
        let report = verify(
            &task(vec![Predicate::FileAbsent {
                path: "locked/f".into(),
            }]),
            dir.path(),
            None,
            None,
        )
        .await;
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        // Root can look anyway; then the file is found, which also fails.
        assert!(!report.passed, "{blocked}");
        // A link that points at itself is present, not absent.
        std::os::unix::fs::symlink(dir.path().join("loop"), dir.path().join("loop")).unwrap();
        assert!(
            !verify(
                &task(vec![Predicate::FileAbsent {
                    path: "loop".into()
                }]),
                dir.path(),
                None,
                None
            )
            .await
            .passed
        );
        // A missing path is absent.
        assert!(
            verify(
                &task(vec![Predicate::FileAbsent {
                    path: "never".into()
                }]),
                dir.path(),
                None,
                None
            )
            .await
            .passed
        );
        // A dangling link is present, not absent.
        std::os::unix::fs::symlink(dir.path().join("gone"), dir.path().join("link")).unwrap();
        assert!(
            !verify(
                &task(vec![Predicate::FileAbsent {
                    path: "link".into()
                }]),
                dir.path(),
                None,
                None
            )
            .await
            .passed
        );
    }

    #[tokio::test]
    async fn command_checks_run_through_the_confinement() {
        let dir = tempfile::tempdir().unwrap();
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        let confine: Confinement = Arc::new(move |program: &Path, args: &[OsString]| {
            log.lock()
                .unwrap()
                .push((program.to_path_buf(), args.to_vec()));
            Ok(ConfinedCommand {
                program: program.to_path_buf(),
                args: args.to_vec(),
                env: Some(vec![("PATH".into(), "/usr/bin:/bin".into())]),
            })
        });
        let t = task(vec![Predicate::Command {
            command: "test -z \"$SECRET_FOR_TEST\"".into(),
            exit: 0,
        }]);
        let report = verify(&t, dir.path(), None, Some(&confine)).await;
        assert!(report.passed, "{}", describe(&report));
        assert_eq!(seen.lock().unwrap().len(), 1);
        let refuse: Confinement = Arc::new(|_: &Path, _: &[OsString]| Err("no sandbox".into()));
        let report = verify(&t, dir.path(), None, Some(&refuse)).await;
        assert!(!report.passed);
        assert!(report.results[0].evidence.contains("no sandbox"));
    }
}
