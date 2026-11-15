//! Scripted end-to-end tests of the `browser` tool against a local fixture
//! app, with headless Chrome and temporary profiles. No model is involved.
//!
//! They need Python with Playwright and Google Chrome. Where those are
//! missing (CI on Linux or Windows, usually) every test prints why and
//! passes without running.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use hame_agent::{ToolPipeline, ToolRequest};
use hame_browser::{BrowserConfig, BrowserTool, requirements};
use hame_kernel::{BoxFuture, Layer, Next};
use hame_session::artifact::sha256_hex;
use hame_session::contract::{
    ACTION_JOURNAL_FILE, ActionJournal, ActionReceipt, ActionStatus, Authorization, Capability,
    Channel, Grant,
};
use hame_session::{ArtifactStore, Block};
use hame_tools::{Tool, ToolContext, ToolOutput};
use serde_json::{Value, json};

const VIETNAMESE: &str = "Tiếng Việt: đặt hàng ưu tiên";

/// At most this many browsers at once: each test starts its own.
static SLOTS: (Mutex<usize>, Condvar) = (Mutex::new(0), Condvar::new());
const MAX_BROWSERS: usize = 3;

struct Slot;

impl Slot {
    fn take() -> Self {
        let (lock, cvar) = &SLOTS;
        let mut used = lock.lock().unwrap_or_else(|e| e.into_inner());
        while *used >= MAX_BROWSERS {
            used = cvar.wait(used).unwrap_or_else(|e| e.into_inner());
        }
        *used += 1;
        Slot
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let (lock, cvar) = &SLOTS;
        *lock.lock().unwrap_or_else(|e| e.into_inner()) -= 1;
        cvar.notify_one();
    }
}

struct Fixture {
    child: Child,
    base: String,
}

impl Fixture {
    fn start() -> Self {
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture/app.py");
        let python = hame_kernel::python_program().expect("python checked by requirements");
        let mut child = Command::new(python)
            .arg("-I")
            .arg(script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("start the fixture app");
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let port: u16 = line.trim().trim_start_matches("PORT ").parse().unwrap();
        Fixture {
            child,
            base: format!("http://127.0.0.1:{port}"),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    /// What the app recorded, read directly, not through the browser.
    fn state(&self) -> Value {
        let addr = self.base.trim_start_matches("http://");
        let mut stream = TcpStream::connect(addr).unwrap();
        write!(stream, "GET /__state HTTP/1.0\r\nHost: {addr}\r\n\r\n").unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        let body = response.split_once("\r\n\r\n").unwrap().1;
        serde_json::from_str(body).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn grant(capabilities: &[Capability]) -> Authorization {
    Authorization {
        grants: vec![Grant {
            channel: Channel::Browser,
            app: None,
            origin: Some("http://127.0.0.1:*".into()),
            profile: None,
            capabilities: capabilities.to_vec(),
        }],
    }
}

fn everything() -> Authorization {
    grant(&[
        Capability::Observe,
        Capability::Interact,
        Capability::Commit,
        Capability::FileWrite,
        Capability::Screen,
    ])
}

struct Env {
    fixture: Fixture,
    dir: tempfile::TempDir,
    tool: Arc<BrowserTool>,
    ctx: ToolContext,
    _slot: Slot,
}

impl Env {
    /// A fixture, a workspace and a tool; `None` (after saying why) when the
    /// machine cannot run a browser.
    fn new(authorization: Authorization) -> Option<Self> {
        Self::with(authorization, None)
    }

    fn with(authorization: Authorization, pipeline: Option<Arc<ToolPipeline>>) -> Option<Self> {
        let dir = tempfile::tempdir().unwrap();
        let config = BrowserConfig::new(dir.path().join("profiles"), authorization);
        if let Err(why) = requirements(&config) {
            eprintln!("SKIPPED (browser tests need Python Playwright and Chrome): {why}");
            return None;
        }
        let slot = Slot::take();
        let mut tool = BrowserTool::new(config);
        if let Some(pipeline) = pipeline {
            tool = tool.with_pipeline(pipeline);
        }
        let workspace = dir.path().join("work");
        std::fs::create_dir_all(&workspace).unwrap();
        let journal = ActionJournal::open(dir.path().join(ACTION_JOURNAL_FILE)).unwrap();
        let ctx = ToolContext::new(&workspace)
            .with_artifacts(ArtifactStore::new(dir.path().join("artifacts")))
            .with_journal(Arc::new(journal))
            .with_session("session-1");
        Some(Env {
            fixture: Fixture::start(),
            dir,
            tool: Arc::new(tool),
            ctx,
            _slot: slot,
        })
    }

    fn workspace(&self) -> PathBuf {
        self.ctx.cwd.clone()
    }

    async fn call(&self, input: Value) -> ToolOutput {
        self.tool.call(&input, &self.ctx).await
    }

    /// Calls and expects success, showing the output when it fails.
    async fn ok(&self, input: Value) -> ToolOutput {
        let out = self.call(input.clone()).await;
        assert!(!out.is_error, "{input} failed:\n{}", out.text);
        out
    }

    /// Calls and expects the typed error `code`.
    async fn err(&self, input: Value, code: &str) -> ToolOutput {
        let out = self.call(input.clone()).await;
        assert!(
            out.is_error && out.text.contains(&format!("[{code}]")),
            "{input} should fail with {code}:\n{}",
            out.text
        );
        out
    }

    fn receipts(&self) -> Vec<ActionReceipt> {
        ActionJournal::read(&self.dir.path().join(ACTION_JOURNAL_FILE)).unwrap()
    }

    async fn finish(self) {
        self.tool.shutdown().await;
    }
}

/// The ref on the observation line for `role "name"`, optionally the one
/// shown `(in ctx)`.
fn find_ref(text: &str, role: &str, name: &str, ctx: Option<&str>) -> String {
    let head = format!("- {role} \"{name}\" [");
    text.lines()
        .filter(|line| line.starts_with(&head))
        .find(|line| ctx.is_none_or(|c| line.contains(&format!("(in {c})"))))
        .and_then(|line| {
            let rest = &line[head.len()..];
            rest.split(']').next().map(String::from)
        })
        .unwrap_or_else(|| panic!("no {role} \"{name}\" {ctx:?} in:\n{text}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn b01_duplicate_label_form_is_submitted_exactly_once() {
    let Some(env) = Env::new(everything()) else {
        return;
    };
    let out = env
        .ok(json!({"action": "open", "profile": "a", "url": env.fixture.url("/form")}))
        .await;
    let ship = find_ref(&out.text, "textbox", "Name", Some("Shipping"));
    let bill = find_ref(&out.text, "textbox", "Name", Some("Billing"));
    assert_ne!(ship, bill, "duplicate labels get distinct refs");
    assert!(out.text.contains("untrusted=\"true\""));

    // Naming a duplicated label is ambiguous, and the candidates come back
    // as refs with where they are.
    let amb = env
        .err(
            json!({"action": "fill", "text": "Name", "value": "x"}),
            "ambiguous",
        )
        .await;
    assert!(
        amb.text.contains(&format!("[{ship}] (in Shipping)")),
        "{}",
        amb.text
    );
    assert!(
        amb.text.contains(&format!("[{bill}] (in Billing)")),
        "{}",
        amb.text
    );

    let ship_city = find_ref(&out.text, "textbox", "City", Some("Shipping"));
    let bill_city = find_ref(&out.text, "textbox", "City", Some("Billing"));
    let country = find_ref(&out.text, "combobox", "Country", None);
    let express = find_ref(&out.text, "checkbox", "Express delivery", None);
    let place = find_ref(&out.text, "button", "Place order", None);
    let batch = env
        .ok(json!({"action": "batch", "steps": [
            {"action": "fill", "ref": ship, "value": "Ana"},
            {"action": "fill", "ref": ship_city, "value": "Hanoi"},
            {"action": "fill", "ref": bill, "value": "Ben"},
            {"action": "fill", "ref": bill_city, "value": "Berlin"},
            {"action": "select", "ref": country, "value": "Germany"},
            {"action": "check", "ref": express, "checked": true},
            {"action": "click", "ref": place, "navigates": true, "expect": {"text": "Order #1 received"}},
        ]}))
        .await;
    assert!(
        batch.text.starts_with("batch: 7 of 7 steps completed."),
        "{}",
        batch.text
    );
    assert!(batch.text.contains("Thank you"), "{}", batch.text);

    let subs = env.fixture.state()["submissions"].clone();
    assert_eq!(subs.as_array().unwrap().len(), 1, "{subs}");
    assert_eq!(
        subs[0],
        json!({"ship_name": "Ana", "ship_city": "Hanoi", "bill_name": "Ben", "bill_city": "Berlin",
               "country": "de", "express": "yes", "note": ""})
    );

    // The same order again, on a fresh form, is a duplicate commit.
    let again = env
        .ok(json!({"action": "navigate", "url": env.fixture.url("/form")}))
        .await;
    let steps: Vec<Value> = [
        ("Shipping", "Name", "Ana"),
        ("Shipping", "City", "Hanoi"),
        ("Billing", "Name", "Ben"),
        ("Billing", "City", "Berlin"),
    ]
    .iter()
    .map(|(ctx, name, value)| {
        json!({"action": "fill", "ref": find_ref(&again.text, "textbox", name, Some(ctx)), "value": value})
    })
    .chain([
        json!({"action": "select", "ref": find_ref(&again.text, "combobox", "Country", None), "value": "Germany"}),
        json!({"action": "check", "ref": find_ref(&again.text, "checkbox", "Express delivery", None)}),
    ])
    .collect();
    env.ok(json!({"action": "batch", "steps": steps})).await;
    let dup = env
        .err(
            json!({"action": "click", "text": "Place order"}),
            "duplicate_commit",
        )
        .await;
    assert!(dup.text.contains("completed"), "{}", dup.text);
    assert_eq!(
        env.fixture.state()["submissions"].as_array().unwrap().len(),
        1
    );

    // The commit was journalled before dispatch and settled after.
    let receipts = env.receipts();
    let commit: Vec<&ActionReceipt> = receipts
        .iter()
        .filter(|r| r.capability == Capability::Commit)
        .collect();
    let statuses: Vec<ActionStatus> = commit.iter().map(|r| r.status).collect();
    assert_eq!(
        statuses,
        [
            ActionStatus::NotStarted,
            ActionStatus::Dispatched,
            ActionStatus::Completed
        ]
    );
    assert!(
        commit[0]
            .idempotency_key
            .as_deref()
            .unwrap()
            .starts_with("browser:")
    );
    assert_eq!(commit[0].target.profile.as_deref(), Some("a"));
    env.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn b02_filtered_paginated_extraction_returns_all_and_only_matching_rows() {
    // A GET filter form is not a commit: no Commit grant is needed.
    let Some(env) = Env::new(grant(&[Capability::Observe, Capability::Interact])) else {
        return;
    };
    let out = env
        .ok(json!({"action": "open", "url": env.fixture.url("/table")}))
        .await;
    let category = find_ref(&out.text, "combobox", "Category", None);
    env.ok(json!({"action": "select", "ref": category, "value": "Tool"}))
        .await;
    let filtered = env.ok(json!({"action": "click", "text": "Filter"})).await;
    assert!(filtered.text.contains("navigated"), "{}", filtered.text);
    let table = find_ref(&filtered.text, "table", "Items", None);
    let out = env
        .ok(json!({"action": "extract", "ref": table, "next": "Next", "max_pages": 10}))
        .await;
    let Some(Block::Data { value }) = out.parts.first() else {
        panic!("no data block: {out:?}");
    };
    assert_eq!(value["complete"], json!(true), "{}", out.text);
    assert_eq!(value["pages"], json!(2));
    let mut got: Vec<i64> = value["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["ID"].as_str().unwrap().parse().unwrap())
        .collect();
    got.sort();
    let mut want: Vec<i64> = env.fixture.state()["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|it| it["category"] == "tool")
        .map(|it| it["id"].as_i64().unwrap())
        .collect();
    want.sort();
    assert_eq!(got, want);
    assert!(
        value["rows"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["Category"] == "tool")
    );

    // A bounded observation says it is incomplete; a scope expands one part.
    let small = env.ok(json!({"action": "observe", "max_nodes": 5})).await;
    assert!(
        small.text.contains("Incomplete: showed 5 of 9"),
        "{}",
        small.text
    );
    let form = env
        .ok(json!({"action": "observe", "scope": "#filter"}))
        .await;
    assert!(form.text.contains("button \"Filter\""), "{}", form.text);
    assert!(!form.text.contains("table \"Items\""), "{}", form.text);
    // Back returns to the first page of results.
    let back = env.ok(json!({"action": "back"})).await;
    assert!(
        back.text.contains("Page 1 of 2") && !back.text.contains("page="),
        "{}",
        back.text
    );
    env.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn b03_the_right_file_is_uploaded_to_the_right_record() {
    let Some(env) = Env::new(everything()) else {
        return;
    };
    let bytes: Vec<u8> = (0..4096u32).map(|i| (i * 31 % 251) as u8).collect();
    std::fs::create_dir_all(env.workspace().join("docs")).unwrap();
    std::fs::write(env.workspace().join("docs/invoice-march.pdf"), &bytes).unwrap();
    std::fs::write(env.dir.path().join("secret.txt"), b"outside").unwrap();

    let out = env
        .ok(json!({"action": "open", "url": env.fixture.url("/records")}))
        .await;
    let input = find_ref(&out.text, "file", "Attachment", Some("R-102"));
    let upload = find_ref(&out.text, "button", "Upload", Some("R-102"));
    assert_ne!(
        input,
        find_ref(&out.text, "file", "Attachment", Some("R-101"))
    );

    // Only workspace files.
    env.err(
        json!({"action": "upload", "ref": input, "path": "../secret.txt"}),
        "unauthorized",
    )
    .await;
    env.ok(json!({"action": "upload", "ref": input, "path": "docs/invoice-march.pdf"}))
        .await;
    let sent = env.ok(json!({"action": "click", "ref": upload})).await;
    assert!(sent.text.contains("as a commit"), "{}", sent.text);
    assert!(sent.text.contains("Uploaded to R-102"), "{}", sent.text);

    let uploads = env.fixture.state()["uploads"].clone();
    assert_eq!(
        uploads,
        json!([{"record": "R-102", "filename": "invoice-march.pdf", "sha256": sha256_hex(&bytes),
                "size": 4096}])
    );
    env.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn b04_a_delayed_date_range_csv_download_completes_with_the_right_content() {
    let Some(env) = Env::new(everything()) else {
        return;
    };
    let out = env
        .ok(json!({"action": "open", "url": env.fixture.url("/export")}))
        .await;
    let from = find_ref(&out.text, "textbox", "From", None);
    let to = find_ref(&out.text, "textbox", "To", None);
    env.ok(json!({"action": "fill", "ref": from, "value": "2026-01-10"}))
        .await;
    env.ok(json!({"action": "fill", "ref": to, "value": "2026-01-20"}))
        .await;
    let started = Instant::now();
    let out = env
        .ok(json!({"action": "download", "text": "Export CSV", "path": "exports/"}))
        .await;
    assert!(started.elapsed() >= Duration::from_millis(1500));
    let path = env
        .workspace()
        .join("exports/ledger-2026-01-10-2026-01-20.csv");
    let csv = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{e}: {}", out.text));
    let ledger = env.fixture.state()["ledger"].clone();
    let mut want = String::from("date,amount\n");
    for row in ledger.as_array().unwrap() {
        let date = row["date"].as_str().unwrap();
        if ("2026-01-10"..="2026-01-20").contains(&date) {
            want.push_str(&format!("{date},{}\n", row["amount"].as_str().unwrap()));
        }
    }
    assert_eq!(csv, want);
    assert_eq!(csv.lines().count(), 12);
    let Some(Block::Artifact { artifact }) = out.parts.first() else {
        panic!("no artifact: {out:?}");
    };
    assert_eq!(artifact.sha256, sha256_hex(csv.as_bytes()));
    assert_eq!(artifact.mime, "text/csv");
    let store = env.ctx.artifacts.as_ref().unwrap();
    assert_eq!(store.get(artifact).unwrap(), csv.as_bytes());
    // A download started by a plain click (a GET form) is waited for.
    env.ok(json!({"action": "click", "text": "Export CSV"}))
        .await;
    let waited = env
        .ok(json!({"action": "wait", "download": true, "path": "exports/again.csv", "timeout_ms": 15000}))
        .await;
    assert!(waited.text.contains("Download finished"), "{}", waited.text);
    assert_eq!(
        std::fs::read_to_string(env.workspace().join("exports/again.csv")).unwrap(),
        want
    );
    // Downloads are FileWrite, journalled.
    assert!(
        env.receipts()
            .iter()
            .any(|r| r.capability == Capability::FileWrite
                && r.status == ActionStatus::Completed
                && r.action == "browser.download")
    );
    env.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn b05_a_preference_changes_in_one_profile_only() {
    let Some(env) = Env::new(everything()) else {
        return;
    };
    let a = env
        .ok(json!({"action": "open", "profile": "a", "url": env.fixture.url("/prefs")}))
        .await;
    assert!(a.text.contains("Current theme: light"), "{}", a.text);
    let theme = find_ref(&a.text, "combobox", "Theme", None);
    let save = find_ref(&a.text, "button", "Save preferences", None);
    env.ok(json!({"action": "select", "ref": theme, "value": "Dark"}))
        .await;
    env.ok(json!({"action": "click", "ref": save})).await;
    env.ok(json!({"action": "wait", "text": "Saved"})).await;

    let b = env
        .ok(json!({"action": "open", "profile": "b", "url": env.fixture.url("/prefs")}))
        .await;
    assert!(b.text.contains("Current theme: light"), "{}", b.text);
    let b_tab = b
        .text
        .split("Opened tab ")
        .nth(1)
        .unwrap()
        .split(' ')
        .next()
        .unwrap()
        .to_string();

    // A ref from profile a is refused when the action names profile b or
    // b's tab.
    env.err(
        json!({"action": "click", "ref": save, "profile": "b"}),
        "target_mismatch",
    )
    .await;
    env.err(
        json!({"action": "click", "ref": save, "tab": b_tab}),
        "target_mismatch",
    )
    .await;

    // Both survive a restart of the browser: profiles are persistent.
    env.tool.shutdown().await;
    let a = env
        .ok(json!({"action": "open", "profile": "a", "url": env.fixture.url("/prefs")}))
        .await;
    assert!(a.text.contains("Current theme: dark"), "{}", a.text);
    let b = env
        .ok(json!({"action": "open", "profile": "b", "url": env.fixture.url("/prefs")}))
        .await;
    assert!(b.text.contains("Current theme: light"), "{}", b.text);
    let saved = env.fixture.state()["prefs_saved"].clone();
    assert_eq!(saved.as_array().unwrap().len(), 1, "{saved}");
    assert_eq!(saved[0]["theme"], "dark");
    assert!(
        env.dir.path().join("profiles/a").is_dir() && env.dir.path().join("profiles/b").is_dir()
    );

    let tabs = env.ok(json!({"action": "tabs"})).await;
    assert!(
        tabs.text.contains("profile=a") && tabs.text.contains("profile=b"),
        "{}",
        tabs.text
    );
    env.ok(json!({"action": "close", "profile": "b"})).await;
    let tabs = env.ok(json!({"action": "tabs"})).await;
    assert!(!tabs.text.contains("profile=b"), "{}", tabs.text);
    env.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_ref_is_stale_after_the_dom_changes_or_the_page_navigates() {
    let Some(env) = Env::new(everything()) else {
        return;
    };
    let out = env
        .ok(json!({"action": "open", "url": env.fixture.url("/mutating")}))
        .await;
    let alpha = find_ref(&out.text, "button", "Edit", Some("Alpha"));
    assert_ne!(alpha, find_ref(&out.text, "button", "Edit", Some("Beta")));
    env.ok(json!({"action": "click", "text": "Shuffle"})).await;
    // The list was re-rendered: same labels, new elements.
    let stale = env
        .err(json!({"action": "click", "ref": alpha}), "stale_ref")
        .await;
    assert!(stale.text.contains("removed"), "{}", stale.text);
    assert!(stale.text.contains("observe again"), "{}", stale.text);

    // A change to the element itself.
    let out = env.ok(json!({"action": "observe"})).await;
    assert!(!out.text.contains("Editing"));
    let gamma = find_ref(&out.text, "button", "Edit", Some("Gamma"));
    env.ok(json!({"action": "click", "text": "Rename first"}))
        .await;
    let changed = env
        .err(json!({"action": "click", "ref": gamma}), "stale_ref")
        .await;
    assert!(changed.text.contains("Modify"), "{}", changed.text);

    // Navigation.
    let out = env.ok(json!({"action": "observe"})).await;
    let beta = find_ref(&out.text, "button", "Edit", Some("Beta"));
    env.ok(json!({"action": "navigate", "url": env.fixture.url("/mutating")}))
        .await;
    let nav = env
        .err(json!({"action": "click", "ref": beta}), "stale_ref")
        .await;
    assert!(nav.text.contains("navigated"), "{}", nav.text);
    // Nothing stale was ever clicked.
    let out = env.ok(json!({"action": "observe"})).await;
    assert!(!out.text.contains("Editing"), "{}", out.text);
    env.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn hidden_covered_or_disabled_controls_are_not_actionable() {
    let Some(env) = Env::new(everything()) else {
        return;
    };
    let out = env
        .ok(json!({"action": "open", "url": env.fixture.url("/hidden")}))
        .await;
    let covered = find_ref(&out.text, "button", "Covered action", None);
    let disabled = find_ref(&out.text, "button", "Disabled action", None);
    let vanish = find_ref(&out.text, "button", "Vanishing action", None);
    assert!(out.text.contains("disabled"));
    let c = env
        .err(
            json!({"action": "click", "ref": covered, "timeout_ms": 1000}),
            "not_actionable",
        )
        .await;
    assert!(c.text.contains("intercepts pointer events"), "{}", c.text);
    let d = env
        .err(
            json!({"action": "click", "ref": disabled, "timeout_ms": 1000}),
            "not_actionable",
        )
        .await;
    assert!(d.text.contains("enabled"), "{}", d.text);
    env.ok(json!({"action": "click", "text": "Hide it"})).await;
    let v = env
        .err(
            json!({"action": "click", "ref": vanish, "timeout_ms": 1000}),
            "not_actionable",
        )
        .await;
    assert!(v.text.contains("visible"), "{}", v.text);
    let out = env.ok(json!({"action": "observe"})).await;
    assert!(!out.text.contains("clicked"), "{}", out.text);
    // Refused before dispatch: nothing but the one real click was journalled.
    let clicks: Vec<_> = env
        .receipts()
        .into_iter()
        .filter(|r| r.status == ActionStatus::Dispatched)
        .collect();
    assert_eq!(clicks.len(), 1, "{clicks:?}");
    env.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unexpected_dialog_stops_a_batch_and_reports_the_completed_prefix() {
    let Some(env) = Env::new(everything()) else {
        return;
    };
    let out = env
        .ok(json!({"action": "open", "url": env.fixture.url("/dialog")}))
        .await;
    let first = find_ref(&out.text, "textbox", "First", None);
    let second = find_ref(&out.text, "textbox", "Second", None);
    let third = find_ref(&out.text, "textbox", "Third", None);
    let next = find_ref(&out.text, "button", "Next step", None);
    let batch = env
        .call(json!({"action": "batch", "steps": [
            {"action": "fill", "ref": first, "value": "1"},
            {"action": "fill", "ref": second, "value": "2"},
            {"action": "click", "ref": next},
            {"action": "fill", "ref": third, "value": "3"},
        ]}))
        .await;
    assert!(batch.is_error, "{}", batch.text);
    assert!(
        batch
            .text
            .starts_with("batch: 2 of 4 steps completed; stopped: step 3"),
        "{}",
        batch.text
    );
    assert!(batch.text.contains("[dialog_open]"), "{}", batch.text);
    assert!(batch.text.contains("Discard your draft?"), "{}", batch.text);
    assert!(!batch.text.contains("4. fill"), "{}", batch.text);
    let dismissed = env.ok(json!({"action": "dialog", "accept": false})).await;
    assert!(
        dismissed.text.contains("Dismissed the confirm dialog"),
        "{}",
        dismissed.text
    );
    let out = env.ok(json!({"action": "observe"})).await;
    assert!(out.text.contains("Draft kept"), "{}", out.text);
    assert!(!out.text.contains("value=\"3\""), "{}", out.text);

    // An in-page modal stops a batch too.
    let batch = env
        .call(json!({"action": "batch", "steps": [
            {"action": "fill", "ref": first, "value": "a"},
            {"action": "click", "text": "Show terms"},
            {"action": "fill", "ref": third, "value": "never"},
        ]}))
        .await;
    assert!(
        batch.text.starts_with(
            "batch: 1 of 3 steps completed; stopped: step 2 opened an unexpected dialog"
        ),
        "{}",
        batch.text
    );
    assert!(batch.text.contains("Terms changed"), "{}", batch.text);
    env.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_crash_after_a_submit_leaves_the_outcome_unknown_and_blocks_a_blind_retry() {
    let Some(env) = Env::new(everything()) else {
        return;
    };
    let out = env
        .ok(json!({"action": "open", "profile": "a", "url": env.fixture.url("/form?slow=1")}))
        .await;
    let name = find_ref(&out.text, "textbox", "Name", Some("Shipping"));
    env.ok(json!({"action": "fill", "ref": name, "value": "Crash Test"}))
        .await;
    let place = find_ref(&out.text, "button", "Place order", None);

    let tool = Arc::clone(&env.tool);
    let ctx = env.ctx.clone();
    let click = tokio::spawn(async move {
        tool.call(&json!({"action": "click", "ref": place}), &ctx)
            .await
    });
    // Kill the helper once the server has the submission and is still
    // answering it.
    let deadline = Instant::now() + Duration::from_secs(15);
    while env.fixture.state()["submissions"]
        .as_array()
        .unwrap()
        .is_empty()
    {
        assert!(Instant::now() < deadline, "the submission never arrived");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let pid = env.tool.helper_pid().expect("helper running");
    let pid = pid.to_string();
    let (program, args) = if cfg!(windows) {
        ("taskkill", vec!["/F", "/PID", pid.as_str()])
    } else {
        ("kill", vec!["-9", pid.as_str()])
    };
    Command::new(program).args(args).status().unwrap();
    let out = click.await.unwrap();
    assert!(out.is_error, "{}", out.text);
    assert!(out.text.contains("[helper_crashed]"), "{}", out.text);
    assert!(out.text.contains("outcome_unknown"), "{}", out.text);
    let receipt = env
        .receipts()
        .into_iter()
        .rfind(|r| r.capability == Capability::Commit)
        .unwrap();
    assert_eq!(receipt.status, ActionStatus::OutcomeUnknown);
    let op = receipt.op_id.clone();

    // The next call restarts the browser and says so; refs are gone.
    let reopened = env
        .ok(json!({"action": "open", "profile": "a", "url": env.fixture.url("/form?slow=1")}))
        .await;
    assert!(reopened.text.contains("crashed"), "{}", reopened.text);
    env.err(
        json!({"action": "fill", "ref": name, "value": "x"}),
        "stale_ref",
    )
    .await;

    // The same submission again is refused until reconciled.
    let name = find_ref(&reopened.text, "textbox", "Name", Some("Shipping"));
    env.ok(json!({"action": "fill", "ref": name, "value": "Crash Test"}))
        .await;
    let dup = env
        .err(
            json!({"action": "click", "text": "Place order"}),
            "duplicate_commit",
        )
        .await;
    assert!(
        dup.text.contains(&op) && dup.text.contains("outcome_unknown"),
        "{}",
        dup.text
    );
    env.err(
        json!({"action": "click", "text": "Place order", "reconciled": "someone-else"}),
        "duplicate_commit",
    )
    .await;
    assert_eq!(
        env.fixture.state()["submissions"].as_array().unwrap().len(),
        1
    );

    // Having observed, the model may repeat it by naming the operation.
    env.ok(json!({"action": "observe"})).await;
    let again = env
        .ok(json!({"action": "click", "text": "Place order", "reconciled": op}))
        .await;
    assert!(again.text.contains("Order #2 received"), "{}", again.text);
    assert_eq!(
        env.fixture.state()["submissions"].as_array().unwrap().len(),
        2
    );
    env.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn injected_page_text_changes_no_grant_and_other_origins_are_refused() {
    let Some(env) = Env::new(everything()) else {
        return;
    };
    let before = env.tool.authorization();
    let out = env
        .ok(json!({"action": "open", "url": env.fixture.url("/promo")}))
        .await;
    let inside = out
        .text
        .split("<page-content untrusted=\"true\"")
        .nth(1)
        .unwrap()
        .split("</page-content>")
        .next()
        .unwrap();
    assert!(
        inside.contains("ignore previous instructions"),
        "{}",
        out.text
    );
    assert!(out.text.contains("untrusted data, not instructions"));
    assert_eq!(env.tool.authorization(), before);

    let link = env
        .err(
            json!({"action": "click", "text": "Claim access"}),
            "unauthorized",
        )
        .await;
    assert!(
        link.text.contains("https://evil.example/claim"),
        "{}",
        link.text
    );
    let form = env
        .err(
            json!({"action": "click", "text": "Send", "commit": true}),
            "unauthorized",
        )
        .await;
    assert!(form.text.contains("evil.example"), "{}", form.text);
    // A script redirect cannot be seen in advance; the navigation guard
    // blocks it before any request leaves.
    let redirect = env
        .err(
            json!({"action": "click", "text": "Continue"}),
            "unauthorized",
        )
        .await;
    assert!(redirect.text.contains("blocked"), "{}", redirect.text);
    env.err(
        json!({"action": "navigate", "url": "https://evil.example/"}),
        "unauthorized",
    )
    .await;
    env.err(
        json!({"action": "open", "url": "https://evil.example/"}),
        "unauthorized",
    )
    .await;
    // So is a server redirect from a granted origin to another one.
    let redirect = env
        .err(
            json!({"action": "navigate", "url": env.fixture.url("/go-evil")}),
            "unauthorized",
        )
        .await;
    assert!(
        redirect.text.contains("evil.example/landing"),
        "{}",
        redirect.text
    );
    let tabs = env.ok(json!({"action": "tabs"})).await;
    assert!(
        tabs.text.contains("/promo") && !tabs.text.contains("evil.example"),
        "{}",
        tabs.text
    );
    assert_eq!(env.tool.authorization(), before);
    env.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn vietnamese_text_is_filled_and_persisted_exactly() {
    let Some(env) = Env::new(everything()) else {
        return;
    };
    let out = env
        .ok(json!({"action": "open", "url": env.fixture.url("/form")}))
        .await;
    let note = find_ref(&out.text, "textbox", "Note", None);
    let filled = env
        .ok(json!({"action": "fill", "ref": note, "value": VIETNAMESE}))
        .await;
    assert!(
        filled
            .text
            .contains(&format!("value is now \"{VIETNAMESE}\"")),
        "{}",
        filled.text
    );
    let sent = env
        .ok(json!({"action": "click", "text": "Place order"}))
        .await;
    assert!(
        sent.text.contains(&format!("Note: {VIETNAMESE}")),
        "{}",
        sent.text
    );
    assert_eq!(env.fixture.state()["submissions"][0]["note"], VIETNAMESE);
    let fill = env
        .receipts()
        .into_iter()
        .find(|r| r.action == "browser.fill")
        .unwrap();
    assert_eq!(fill.emitted, [VIETNAMESE]);
    env.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_submit_needs_a_commit_grant_and_a_screenshot_needs_screen() {
    let Some(env) = Env::new(grant(&[Capability::Observe, Capability::Interact])) else {
        return;
    };
    let out = env
        .ok(json!({"action": "open", "url": env.fixture.url("/form")}))
        .await;
    let name = find_ref(&out.text, "textbox", "Name", Some("Shipping"));
    env.ok(json!({"action": "fill", "ref": name, "value": "Ana"}))
        .await;
    let refused = env
        .err(
            json!({"action": "click", "text": "Place order"}),
            "unauthorized",
        )
        .await;
    assert!(refused.text.contains("Commit"), "{}", refused.text);
    env.err(json!({"action": "screenshot"}), "unauthorized")
        .await;
    assert!(
        env.fixture.state()["submissions"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    // With Screen granted, an element screenshot carries its CSS crop.
    env.tool.set_authorization(everything());
    let place = find_ref(&out.text, "button", "Place order", None);
    let shot = env.ok(json!({"action": "screenshot", "ref": place})).await;
    let Some(Block::Image { artifact }) = shot.parts.first() else {
        panic!("no image: {shot:?}");
    };
    let capture = artifact.capture.as_ref().unwrap();
    assert_eq!(capture.space, "css-pixels");
    assert!(
        capture.crop[2] > 10.0 && capture.crop[3] > 5.0,
        "{capture:?}"
    );
    let expected_width = (capture.crop[2] * capture.scale).round() as i64;
    assert!(
        (i64::from(artifact.width.unwrap()) - expected_width).abs() <= 2,
        "{artifact:?}"
    );
    let full = env.ok(json!({"action": "screenshot"})).await;
    let Some(Block::Image { artifact }) = full.parts.first() else {
        panic!("no image");
    };
    assert_eq!(
        artifact.capture.as_ref().unwrap().crop,
        [0.0, 0.0, 1280.0, 900.0]
    );
    env.finish().await;
}

/// Denies clicks, the way a permission rule would.
struct NoClicks;

impl Layer<ToolRequest, ToolOutput> for NoClicks {
    fn call<'a>(
        &'a self,
        request: ToolRequest,
        next: Next<'a, ToolRequest, ToolOutput>,
    ) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            if request.input["action"] == "click" {
                ToolOutput::error("Denied by the user's permission rules.")
            } else {
                next.run(request).await
            }
        })
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn each_batch_step_goes_through_the_policy_pipeline() {
    let pipeline = Arc::new(ToolPipeline::new());
    pipeline.push(NoClicks);
    let Some(env) = Env::with(everything(), Some(pipeline)) else {
        return;
    };
    let out = env
        .ok(json!({"action": "open", "url": env.fixture.url("/form")}))
        .await;
    let name = find_ref(&out.text, "textbox", "Name", Some("Shipping"));
    let batch = env
        .call(json!({"action": "batch", "steps": [
            {"action": "fill", "ref": name, "value": "Ana"},
            {"action": "click", "text": "Place order"},
            {"action": "fill", "ref": name, "value": "never"},
        ]}))
        .await;
    assert!(
        batch
            .text
            .starts_with("batch: 1 of 3 steps completed; stopped: step 2"),
        "{}",
        batch.text
    );
    assert!(
        batch
            .text
            .contains("Denied by the user's permission rules."),
        "{}",
        batch.text
    );
    assert!(batch.text.contains("value=\"Ana\""), "{}", batch.text);
    assert!(
        env.fixture.state()["submissions"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    // A failed expect stops the batch before the next step.
    let batch = env
        .call(json!({"action": "batch", "steps": [
            {"action": "fill", "ref": name, "value": "Bo", "expect": {"ref": name, "value": "Ana"}},
            {"action": "fill", "ref": name, "value": "never"},
        ]}))
        .await;
    assert!(
        batch
            .text
            .starts_with("batch: 0 of 2 steps completed; stopped: step 1's expect failed"),
        "{}",
        batch.text
    );
    assert!(batch.text.contains("expected 'Ana'"), "{}", batch.text);
    assert!(batch.text.contains("value=\"Bo\""), "{}", batch.text);
    env.finish().await;
}
