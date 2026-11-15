//! GUI tests against real applications on this Mac.
//!
//! They drive TextEdit through the `desktop` tool exactly as a model would,
//! so they need Accessibility and Screen Recording permission for the
//! process running them, and they briefly take keyboard focus. They are
//! ignored by default; run them, one at a time, with
//!
//! ```text
//! cargo test -p hame-desktop --test gui -- --ignored --test-threads=1
//! ```
//!
//! Each test works only on windows it creates, saves only into temporary
//! directories, closes its documents without saving, and quits TextEdit
//! afterwards if TextEdit was not running before.
#![cfg(target_os = "macos")]

use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::FutureExt as _;
use hame_agent::{ToolPipeline, ToolRequest};
use hame_desktop::{DesktopConfig, DesktopTool, parse_ref, visual};
use hame_kernel::{BoxFuture, Layer, Next};
use hame_session::contract::{
    ActionJournal, ActionStatus, Authorization, Capability, Channel, Grant,
};
use hame_session::{ArtifactStore, Block};
use hame_tools::{Tool, ToolContext, ToolOutput};
use serde_json::{Value, json};
use tempfile::TempDir;

const TEXTEDIT: &str = "com.apple.TextEdit";
const VIETNAMESE: &str = "Tiếng Việt có dấu: ắ ằ ẳ ẵ ặ đ Đ ơ ư";

fn all_caps() -> Vec<Capability> {
    vec![
        Capability::Observe,
        Capability::Interact,
        Capability::Commit,
        Capability::FileWrite,
        Capability::Clipboard,
        Capability::GlobalInput,
        Capability::Screen,
    ]
}

/// Everything on TextEdit, through both channels, and the clipboard.
fn textedit_grants(caps: Vec<Capability>) -> Authorization {
    Authorization {
        grants: vec![
            Grant {
                channel: Channel::Desktop,
                app: Some(TEXTEDIT.into()),
                origin: None,
                profile: None,
                capabilities: caps.clone(),
            },
            Grant {
                channel: Channel::Visual,
                app: Some(TEXTEDIT.into()),
                origin: None,
                profile: None,
                capabilities: caps,
            },
            Grant {
                channel: Channel::Desktop,
                app: None,
                origin: None,
                profile: None,
                capabilities: vec![Capability::Clipboard],
            },
        ],
    }
}

fn helper_cache() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("hame-desktop-helpers")
}

fn config(home: &Path, auth: Authorization) -> DesktopConfig {
    let mut config = DesktopConfig::new(home, auth);
    config.helper_dir = Some(helper_cache());
    config
}

fn running(bundle: &str) -> bool {
    let name = if bundle == TEXTEDIT {
        "TextEdit"
    } else {
        bundle
    };
    Command::new("pgrep")
        .args(["-x", name])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Window ids and titles from `windows` output.
fn windows_of(text: &str) -> Vec<(u64, String)> {
    text.lines()
        .filter_map(|line| {
            let rest = line.strip_prefix("window ")?;
            let (id, rest) = rest.split_once(' ')?;
            let title = rest.split('"').nth(1).unwrap_or("").to_string();
            Some((id.parse().ok()?, title))
        })
        .collect()
}

/// The ref on the first observation line containing `needle`.
fn find_ref(observation: &str, needle: &str) -> Option<String> {
    observation
        .lines()
        .filter(|line| line.contains(needle))
        .flat_map(|line| line.split_whitespace())
        .find(|token| parse_ref(token).is_ok())
        .map(str::to_string)
}

/// The text area's value as an observation shows it (up to 120
/// characters).
fn text_area_value(observation: &str) -> String {
    let line = line_of(observation, " textarea");
    let Some((_, rest)) = line.split_once(" = ") else {
        return String::new();
    };
    serde_json::Deserializer::from_str(rest)
        .into_iter::<String>()
        .next()
        .and_then(Result::ok)
        .unwrap_or_default()
}

fn line_of<'a>(observation: &'a str, needle: &str) -> &'a str {
    observation
        .lines()
        .find(|line| line.contains(needle))
        .unwrap_or("")
}

struct Gui {
    tool: DesktopTool,
    ctx: ToolContext,
    _home: TempDir,
    journal_path: PathBuf,
    was_running: bool,
    created: Vec<u64>,
}

impl Gui {
    async fn start() -> Self {
        Self::start_with(textedit_grants(all_caps()), None).await
    }

    async fn start_with(auth: Authorization, pipeline: Option<Arc<ToolPipeline>>) -> Self {
        let home = tempfile::tempdir().unwrap();
        let journal_path = home.path().join("actions.v1.jsonl");
        let journal = Arc::new(ActionJournal::open(&journal_path).unwrap());
        let ctx = ToolContext::new(home.path())
            .with_artifacts(ArtifactStore::new(home.path().join("artifacts")))
            .with_journal(journal)
            .with_session("gui-test");
        let mut tool = DesktopTool::new(config(home.path(), auth));
        if let Some(pipeline) = pipeline {
            tool = tool.with_pipeline(pipeline);
        }
        let was_running = running(TEXTEDIT);
        if !was_running {
            let status = Command::new("open")
                .args(["-g", "-b", TEXTEDIT])
                .status()
                .unwrap();
            assert!(status.success());
        }
        let gui = Self {
            tool,
            ctx,
            _home: home,
            journal_path,
            was_running,
            created: Vec::new(),
        };
        // Wait for TextEdit to answer accessibility requests. A freshly
        // launched TextEdit answers before its menu bar exists; it is ready
        // once its launch window (the Open panel) is up.
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let out = gui
                .call(json!({"action": "windows", "app": TEXTEDIT}))
                .await;
            if !out.is_error && (gui.was_running || !windows_of(&out.text).is_empty()) {
                break;
            }
            if Instant::now() >= deadline {
                assert!(!out.is_error, "TextEdit did not start: {}", out.text);
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        gui
    }

    async fn call(&self, input: Value) -> ToolOutput {
        self.tool.call(&input, &self.ctx).await
    }

    async fn ok(&self, input: Value) -> String {
        let out = self.call(input.clone()).await;
        assert!(!out.is_error, "{input} failed: {}", out.text);
        out.text
    }

    async fn windows(&self) -> Vec<(u64, String)> {
        windows_of(&self.ok(json!({"action": "windows", "app": TEXTEDIT})).await)
    }

    async fn exists(&self, window: u64) -> bool {
        self.windows().await.iter().any(|(id, _)| *id == window)
    }

    /// A new, empty TextEdit document; its window id.
    async fn new_document(&mut self) -> u64 {
        let before = self.windows().await;
        if let Some((open, _)) = before.iter().find(|(_, title)| title == "Open") {
            self.ok(json!({"action": "press", "app": TEXTEDIT, "window": open,
                           "name": "New Document", "role": "button"}))
                .await;
        } else {
            self.ok(json!({"action": "menu", "app": TEXTEDIT, "path": "File > New"}))
                .await;
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let now = self.windows().await;
            if let Some((id, _)) = now
                .iter()
                .find(|(id, title)| title != "Open" && !before.iter().any(|(b, _)| b == id))
            {
                self.created.push(*id);
                return *id;
            }
            assert!(Instant::now() < deadline, "no new document window: {now:?}");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn observe(&self, window: u64) -> String {
        self.ok(json!({"action": "observe", "app": TEXTEDIT, "window": window, "max_nodes": 200}))
            .await
    }

    async fn text_area(&self, window: u64) -> String {
        let obs = self.observe(window).await;
        find_ref(&obs, " textarea").unwrap_or_else(|| panic!("no text area in {obs}"))
    }

    /// Observes until `needle` shows up.
    async fn observe_until(&self, window: u64, needle: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let obs = self.observe(window).await;
            if obs.contains(needle) {
                return obs;
            }
            assert!(
                Instant::now() < deadline,
                "{needle} never appeared in {obs}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Saves the document through File > Save… into `dir` as `name`,
    /// navigating the save panel with its Go To Folder sheet.
    async fn save_as(&self, window: u64, dir: &Path, name: &str) {
        self.ok(
            json!({"action": "menu", "app": TEXTEDIT, "window": window, "path": "File > Save…"}),
        )
        .await;
        self.ok(json!({"action": "wait", "app": TEXTEDIT, "window": window, "until": "dialog"}))
            .await;
        self.ok(json!({"action": "focus", "app": TEXTEDIT, "window": window}))
            .await;
        self.ok(json!({"action": "key", "app": TEXTEDIT, "window": window, "keys": "cmd+shift+g"}))
            .await;
        let obs = self.observe_until(window, "PathTextField").await;
        let path_field = find_ref(&obs, "PathTextField").unwrap();
        self.ok(json!({"action": "set_value", "ref": path_field, "value": dir.to_str().unwrap()}))
            .await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        self.ok(json!({"action": "key", "app": TEXTEDIT, "window": window, "keys": "return"}))
            .await;
        let folder = dir.file_name().unwrap().to_str().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let obs = self.observe(window).await;
            if !obs.contains("PathTextField")
                && line_of(&obs, "popupbutton \"Where:\"").contains(folder)
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the save panel did not go to {folder}: {obs}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        self.ok(
            json!({"action": "set_value", "app": TEXTEDIT, "window": window,
                       "name": "Save As:", "role": "textfield", "value": name}),
        )
        .await;
        self.ok(json!({"action": "press", "app": TEXTEDIT, "window": window,
                       "name": "Save", "role": "button"}))
            .await;
        self.ok(json!({"action": "wait", "app": TEXTEDIT, "window": window,
                       "until": "title", "contains": name.trim_end_matches(".rtf")}))
            .await;
    }

    fn receipts(&self) -> Vec<hame_session::contract::ActionReceipt> {
        ActionJournal::read(&self.journal_path).unwrap()
    }

    /// Closes every window this test created, discarding changes, and
    /// quits TextEdit if it was not running before.
    async fn cleanup(&mut self) {
        let mut stuck = Vec::new();
        for window in std::mem::take(&mut self.created) {
            for _ in 0..6 {
                if !self.exists(window).await {
                    break;
                }
                let obs = self.observe(window).await;
                if obs.contains("Dialog open") {
                    // Discard the document if asked; otherwise dismiss.
                    for name in [
                        "Delete",
                        "Don’t Save",
                        "Don't Save",
                        "Revert Changes",
                        "Cancel",
                    ] {
                        if obs.contains(&format!("button \"{name}\"")) {
                            let _ = self
                                .call(json!({"action": "press", "app": TEXTEDIT, "window": window,
                                             "name": name, "role": "button"}))
                                .await;
                            break;
                        }
                    }
                } else {
                    let _ = self
                        .call(json!({"action": "press", "app": TEXTEDIT, "window": window,
                                     "role": "AXCloseButton"}))
                        .await;
                }
                tokio::time::sleep(Duration::from_millis(400)).await;
            }
            if self.exists(window).await {
                stuck.push(window);
            }
        }
        if !self.was_running {
            let _ = self
                .call(
                    json!({"action": "menu", "app": TEXTEDIT, "path": "TextEdit > Quit TextEdit"}),
                )
                .await;
            let deadline = Instant::now() + Duration::from_secs(5);
            while running(TEXTEDIT) && Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
        assert!(stuck.is_empty(), "could not close windows {stuck:?}");
    }
}

/// Runs a test body, then cleans up even if it panicked.
macro_rules! gui_test {
    ($gui:ident, $body:expr) => {{
        let result = AssertUnwindSafe($body).catch_unwind().await;
        $gui.cleanup().await;
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }};
}

fn plain_text(path: &Path) -> String {
    let out = Command::new("textutil")
        .args(["-convert", "txt", "-stdout"])
        .arg(path)
        .output()
        .unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap()
}

#[tokio::test]
#[ignore = "drives TextEdit; run with --ignored --test-threads=1"]
async fn d01_textedit_new_document_saved_through_the_save_panel_beside_a_decoy() {
    let mut gui = Gui::start().await;
    gui_test!(gui, async {
        let dir = tempfile::tempdir().unwrap();
        let decoy = dir.path().join("hame-d01 copy.rtf");
        std::fs::write(&decoy, b"decoy bytes").unwrap();
        let text = "hame D01: saved through the save panel.";

        let doc = gui.new_document().await;
        let area = gui.text_area(doc).await;
        let set = gui
            .ok(json!({"action": "set_value", "ref": area, "value": text}))
            .await;
        assert!(set.contains("reads back exactly"), "{set}");

        // Confirming a save panel writes a file: without FileWrite it is
        // refused, even with Commit.
        let restricted_home = tempfile::tempdir().unwrap();
        let restricted = DesktopTool::new(config(
            restricted_home.path(),
            textedit_grants(vec![
                Capability::Observe,
                Capability::Interact,
                Capability::Commit,
                Capability::GlobalInput,
            ]),
        ));
        gui.ok(json!({"action": "menu", "app": TEXTEDIT, "window": doc, "path": "File > Save…"}))
            .await;
        gui.ok(json!({"action": "wait", "app": TEXTEDIT, "window": doc, "until": "dialog"}))
            .await;
        let refused = restricted
            .call(
                &json!({"action": "press", "app": TEXTEDIT, "window": doc, "name": "Save", "role": "button"}),
                &gui.ctx,
            )
            .await;
        assert!(
            refused.is_error
                && refused.text.starts_with("not_authorized")
                && refused.text.contains("FileWrite"),
            "{}",
            refused.text
        );
        gui.ok(json!({"action": "press", "app": TEXTEDIT, "window": doc, "name": "Cancel", "role": "button"}))
            .await;
        gui.ok(
            json!({"action": "wait", "app": TEXTEDIT, "window": doc, "until": "element_gone",
                      "role": "AXSheet", "name": ""}),
        )
        .await;

        gui.save_as(doc, dir.path(), "hame-d01.rtf").await;
        let saved = dir.path().join("hame-d01.rtf");
        assert!(saved.is_file());
        assert!(std::fs::read(&saved).unwrap().starts_with(b"{\\rtf"));
        assert_eq!(plain_text(&saved).trim_end(), text);
        assert_eq!(std::fs::read(&decoy).unwrap(), b"decoy bytes");
        let names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 2, "{names:?}");

        // Receipts: the confirming press was journalled as a commit with an
        // idempotency key, before and after dispatch.
        let receipts = gui.receipts();
        let press: Vec<_> = receipts
            .iter()
            .filter(|r| r.action == "desktop.press" && r.capability == Capability::Commit)
            .collect();
        let statuses: Vec<ActionStatus> = press.iter().map(|r| r.status).collect();
        assert!(
            statuses.ends_with(&[
                ActionStatus::NotStarted,
                ActionStatus::Dispatched,
                ActionStatus::Completed
            ]),
            "{statuses:?}"
        );
        assert!(press.last().unwrap().idempotency_key.is_some());
        assert_eq!(press.last().unwrap().target.app.as_deref(), Some(TEXTEDIT));

        // File > Save on the saved document is a commit (no ellipsis: no
        // dialog). Choosing it again is the same commit, refused until the
        // earlier one is reconciled against a newer observation.
        gui.ok(json!({"action": "menu", "app": TEXTEDIT, "window": doc, "path": "File > Save"}))
            .await;
        let again = gui
            .call(json!({"action": "menu", "app": TEXTEDIT, "window": doc, "path": "File > Save"}))
            .await;
        assert!(
            again.is_error && again.text.starts_with("prior_effect"),
            "{}",
            again.text
        );
        let op = again
            .text
            .split("reconciled: \"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap()
            .to_string();
        let early = gui
            .call(json!({"action": "menu", "app": TEXTEDIT, "window": doc, "path": "File > Save", "reconciled": op}))
            .await;
        assert!(
            early.is_error && early.text.contains("has not been reconciled"),
            "{}",
            early.text
        );
        gui.observe(doc).await;
        gui.ok(json!({"action": "menu", "app": TEXTEDIT, "window": doc, "path": "File > Save", "reconciled": op}))
            .await;
        assert_eq!(plain_text(&saved).trim_end(), text);

        gui.ok(json!({"action": "menu", "app": TEXTEDIT, "window": doc, "path": "File > Close"}))
            .await;
        gui.ok(json!({"action": "wait", "app": TEXTEDIT, "window": doc, "until": "window_gone"}))
            .await;
    });
}

/// Return in a save panel saves the file, so it needs Commit and FileWrite
/// like pressing Save does; a newline typed there is the same Return. In
/// the document's own text area a newline stays a line break and needs
/// only GlobalInput. (Before this, `key return` needed only GlobalInput
/// and saved without either grant.)
#[tokio::test]
#[ignore = "drives TextEdit; run with --ignored --test-threads=1"]
async fn return_in_the_save_panel_needs_commit_and_file_write() {
    let mut gui = Gui::start().await;
    gui_test!(gui, async {
        let dir = tempfile::tempdir().unwrap();
        let name = "hame-return.rtf";
        let saved = dir.path().join(name);
        let doc = gui.new_document().await;
        let area = gui.text_area(doc).await;
        gui.ok(json!({"action": "set_value", "ref": area, "value": "first line"}))
            .await;

        let typist_home = tempfile::tempdir().unwrap();
        let typist = DesktopTool::new(config(
            typist_home.path(),
            textedit_grants(vec![
                Capability::Observe,
                Capability::Interact,
                Capability::GlobalInput,
            ]),
        ));
        gui.ok(json!({"action": "focus", "ref": area})).await;
        // A digit starts the typed line: macOS text substitution capitalises
        // a typed sentence start ("second" became "Second" on save).
        let typed = typist
            .call(
                &json!({"action": "type", "app": TEXTEDIT, "window": doc, "role": "textarea",
                        "text": "\n2nd line"}),
                &gui.ctx,
            )
            .await;
        assert!(!typed.is_error, "{}", typed.text);
        gui.ok(json!({"action": "wait", "ref": area, "until": "value",
                      "value": "first line\n2nd line"}))
            .await;

        // Open the save panel and point it at the folder, with full grants.
        gui.ok(json!({"action": "menu", "app": TEXTEDIT, "window": doc, "path": "File > Save…"}))
            .await;
        gui.ok(json!({"action": "wait", "app": TEXTEDIT, "window": doc, "until": "dialog"}))
            .await;
        gui.ok(json!({"action": "focus", "app": TEXTEDIT, "window": doc}))
            .await;
        gui.ok(json!({"action": "key", "app": TEXTEDIT, "window": doc, "keys": "cmd+shift+g"}))
            .await;
        let obs = gui.observe_until(doc, "PathTextField").await;
        let path_field = find_ref(&obs, "PathTextField").unwrap();
        gui.ok(json!({"action": "set_value", "ref": path_field,
                      "value": dir.path().to_str().unwrap()}))
            .await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        gui.ok(json!({"action": "key", "app": TEXTEDIT, "window": doc, "keys": "return"}))
            .await;
        let folder = dir.path().file_name().unwrap().to_str().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let obs = gui.observe(doc).await;
            if !obs.contains("PathTextField")
                && line_of(&obs, "popupbutton \"Where:\"").contains(folder)
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the panel did not go to {folder}: {obs}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        gui.ok(
            json!({"action": "set_value", "app": TEXTEDIT, "window": doc,
                      "name": "Save As:", "role": "textfield", "value": name}),
        )
        .await;
        gui.ok(json!({"action": "focus", "app": TEXTEDIT, "window": doc,
                      "name": "Save As:", "role": "textfield"}))
            .await;

        // Focus is in the save panel: Return there, or a typed newline, is
        // refused without Commit, and without FileWrite.
        for (caps, missing) in [
            (
                vec![
                    Capability::Observe,
                    Capability::Interact,
                    Capability::GlobalInput,
                ],
                "Commit",
            ),
            (
                vec![
                    Capability::Observe,
                    Capability::Interact,
                    Capability::Commit,
                    Capability::GlobalInput,
                ],
                "FileWrite",
            ),
        ] {
            let home = tempfile::tempdir().unwrap();
            let restricted = DesktopTool::new(config(home.path(), textedit_grants(caps)));
            for input in [
                json!({"action": "key", "app": TEXTEDIT, "window": doc, "keys": "return"}),
                json!({"action": "type", "app": TEXTEDIT, "window": doc, "text": "\n"}),
            ] {
                let refused = restricted.call(&input, &gui.ctx).await;
                assert!(
                    refused.is_error
                        && refused.text.starts_with("not_authorized")
                        && refused.text.contains(missing)
                        && refused.text.contains("save panel"),
                    "{input}: {}",
                    refused.text
                );
            }
        }
        assert!(!saved.exists(), "a refused Return saved the file");

        // With both, Return saves, journalled as a commit with a key.
        gui.ok(json!({"action": "key", "app": TEXTEDIT, "window": doc, "keys": "return"}))
            .await;
        gui.ok(json!({"action": "wait", "app": TEXTEDIT, "window": doc,
                      "until": "title", "contains": "hame-return"}))
            .await;
        assert!(saved.is_file());
        assert_eq!(plain_text(&saved).trim_end(), "first line\n2nd line");
        let key = gui
            .receipts()
            .into_iter()
            .rfind(|r| r.action == "desktop.key")
            .unwrap();
        assert_eq!(key.status, ActionStatus::Completed);
        assert_eq!(key.capability, Capability::Commit);
        assert!(key.idempotency_key.is_some());
    });
}

/// The standalone Open panel (File > Open…) is drawn by the out-of-process
/// panel service: keys posted to TextEdit never reached it, so Go To
/// Folder could not be opened there (found by pilot task D03).
#[tokio::test]
#[ignore = "drives TextEdit; run with --ignored --test-threads=1"]
async fn d03_the_open_panel_takes_keys_and_opens_the_chosen_file() {
    let mut gui = Gui::start().await;
    gui_test!(gui, async {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path().canonicalize().unwrap();
        let file = dir.join("budget-2026-final.txt");
        std::fs::write(&file, "final\n").unwrap();
        std::fs::write(dir.join("budget-2026-final copy.txt"), "copy\n").unwrap();
        gui.ok(json!({"action": "menu", "app": TEXTEDIT, "path": "File > Open…"}))
            .await;
        gui.ok(json!({"action": "wait", "app": TEXTEDIT, "until": "window", "contains": "Open"}))
            .await;
        let open = gui
            .windows()
            .await
            .into_iter()
            .find(|(_, title)| title == "Open")
            .map(|(id, _)| id)
            .expect("an Open panel");
        gui.ok(json!({"action": "focus", "app": TEXTEDIT, "window": open}))
            .await;
        gui.ok(json!({"action": "key", "app": TEXTEDIT, "window": open, "keys": "cmd+shift+g"}))
            .await;
        let obs = gui.observe_until(open, "PathTextField").await;
        let path_field = find_ref(&obs, "PathTextField").unwrap();
        gui.ok(json!({"action": "set_value", "ref": path_field, "value": file.to_str().unwrap()}))
            .await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        gui.ok(json!({"action": "key", "app": TEXTEDIT, "window": open, "keys": "return"}))
            .await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        gui.ok(json!({"action": "key", "app": TEXTEDIT, "window": open, "keys": "return"}))
            .await;
        let opened = gui
            .ok(json!({"action": "wait", "app": TEXTEDIT, "until": "window",
                       "contains": "budget-2026-final.txt", "timeout_ms": 5000}))
            .await;
        assert!(!opened.contains("copy.txt\""), "{opened}");
        for (id, title) in gui.windows().await {
            if title == "budget-2026-final.txt" {
                gui.created.push(id);
            }
        }
        assert!(!gui.created.is_empty(), "the chosen file opened");
    });
}

#[tokio::test]
#[ignore = "drives TextEdit; run with --ignored --test-threads=1"]
async fn d04_vietnamese_text_persists_exactly_through_set_value_and_type() {
    let mut gui = Gui::start().await;
    gui_test!(gui, async {
        let dir = tempfile::tempdir().unwrap();

        let a = gui.new_document().await;
        let area = gui.text_area(a).await;
        gui.ok(json!({"action": "set_value", "ref": area, "value": VIETNAMESE}))
            .await;
        gui.save_as(a, dir.path(), "vi-set.rtf").await;
        assert_eq!(
            plain_text(&dir.path().join("vi-set.rtf")).trim_end(),
            VIETNAMESE
        );

        let b = gui.new_document().await;
        let area = gui.text_area(b).await;
        gui.ok(json!({"action": "focus", "ref": area})).await;
        let typed = gui
            .ok(json!({"action": "type", "ref": area, "text": VIETNAMESE}))
            .await;
        assert!(
            typed.contains(&format!("Typed {} characters", VIETNAMESE.chars().count())),
            "{typed}"
        );
        gui.ok(json!({"action": "wait", "ref": area, "until": "value", "value": VIETNAMESE}))
            .await;
        gui.save_as(b, dir.path(), "vi-typed.rtf").await;
        assert_eq!(
            plain_text(&dir.path().join("vi-typed.rtf")).trim_end(),
            VIETNAMESE
        );
        // The receipt records exactly what was typed.
        let typed: Vec<_> = gui
            .receipts()
            .into_iter()
            .filter(|r| r.action == "desktop.type" && r.status == ActionStatus::Completed)
            .collect();
        assert_eq!(typed.last().unwrap().emitted, vec![VIETNAMESE.to_string()]);
    });
}

#[tokio::test]
#[ignore = "drives TextEdit; run with --ignored --test-threads=1"]
async fn stale_refs_and_closed_windows_are_rejected() {
    let mut gui = Gui::start().await;
    gui_test!(gui, async {
        let doc = gui.new_document().await;
        let area = gui.text_area(doc).await;

        // A ref to an element that went away: the save sheet's button.
        gui.ok(json!({"action": "menu", "app": TEXTEDIT, "window": doc, "path": "File > Save…"}))
            .await;
        gui.ok(json!({"action": "wait", "app": TEXTEDIT, "window": doc, "until": "dialog"}))
            .await;
        let obs = gui.observe(doc).await;
        let save_button = find_ref(&obs, "button \"Save\"").unwrap();
        gui.ok(json!({"action": "press", "app": TEXTEDIT, "window": doc, "name": "Cancel", "role": "button"}))
            .await;
        gui.ok(
            json!({"action": "wait", "app": TEXTEDIT, "window": doc, "until": "element_gone",
                      "role": "AXSheet", "name": ""}),
        )
        .await;
        // Apps reuse their panels, so the button's accessibility element
        // outlives the sheet; it must still be refused.
        let journalled = gui.receipts().len();
        let stale = gui
            .call(json!({"action": "press", "ref": save_button}))
            .await;
        assert!(
            stale.is_error && stale.text.starts_with("stale_ref"),
            "{}",
            stale.text
        );
        assert_eq!(
            gui.receipts().len(),
            journalled,
            "a refused ref was journalled"
        );

        // A ref into a window that was closed.
        gui.ok(json!({"action": "press", "app": TEXTEDIT, "window": doc, "role": "AXCloseButton"}))
            .await;
        gui.ok(json!({"action": "wait", "app": TEXTEDIT, "window": doc, "until": "window_gone"}))
            .await;
        let journalled = gui.receipts().len();
        let gone = gui
            .call(json!({"action": "set_value", "ref": area, "value": "x"}))
            .await;
        assert!(
            gone.is_error && gone.text.starts_with("stale_window"),
            "{}",
            gone.text
        );
        assert_eq!(
            gui.receipts().len(),
            journalled,
            "a refused ref was journalled"
        );
        let gone = gui
            .call(json!({"action": "observe", "app": TEXTEDIT, "window": doc}))
            .await;
        assert!(
            gone.is_error && gone.text.starts_with("stale_window"),
            "{}",
            gone.text
        );
        // Unknown observations are stale too.
        let unknown = gui.call(json!({"action": "press", "ref": "o999:e1"})).await;
        assert!(unknown.text.starts_with("stale_ref"), "{}", unknown.text);
        // Everything journalled completed; nothing refused was dispatched.
        let latest = ActionJournal::latest(&gui.receipts());
        assert!(latest.values().all(|r| r.status == ActionStatus::Completed));
        assert!(latest.values().all(|r| r.action != "desktop.set_value"));
    });
}

#[tokio::test]
#[ignore = "drives TextEdit; run with --ignored --test-threads=1"]
async fn two_windows_with_the_same_title_are_ambiguous() {
    let mut gui = Gui::start().await;
    gui_test!(gui, async {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        for dir in [&a, &b] {
            std::fs::write(dir.path().join("same.txt"), "same name, different folder\n").unwrap();
        }
        let before = gui.windows().await;
        let status = Command::new("open")
            .args(["-g", "-b", TEXTEDIT])
            .arg(a.path().join("same.txt"))
            .arg(b.path().join("same.txt"))
            .status()
            .unwrap();
        assert!(status.success());
        let deadline = Instant::now() + Duration::from_secs(8);
        let mut ours;
        loop {
            ours = gui
                .windows()
                .await
                .into_iter()
                .filter(|(id, title)| title == "same.txt" && !before.iter().any(|(b, _)| b == id))
                .map(|(id, _)| id)
                .collect::<Vec<_>>();
            if ours.len() == 2 {
                break;
            }
            assert!(Instant::now() < deadline, "the two documents did not open");
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        gui.created.extend(&ours);

        let out = gui
            .call(json!({"action": "observe", "app": TEXTEDIT, "title": "same.txt"}))
            .await;
        assert!(
            out.is_error && out.text.starts_with("ambiguous"),
            "{}",
            out.text
        );
        for id in &ours {
            assert!(
                out.text.contains(&format!("window {id} \"same.txt\"")),
                "{}",
                out.text
            );
        }
        // Naming the window resolves it.
        let obs = gui.observe(ours[0]).await;
        assert!(obs.contains(&format!("window {} \"same.txt\"", ours[0])));
        // Several elements match a bare role: ambiguous, nothing pressed.
        let out = gui
            .call(json!({"action": "press", "app": TEXTEDIT, "window": ours[0], "role": "button"}))
            .await;
        assert!(
            out.is_error && out.text.starts_with("ambiguous"),
            "{}",
            out.text
        );
        assert!(gui.receipts().iter().all(|r| r.action != "desktop.press"));
    });
}

#[tokio::test]
#[ignore = "drives TextEdit; run with --ignored --test-threads=1"]
async fn typing_stops_when_another_window_takes_focus() {
    let mut gui = Gui::start().await;
    gui_test!(gui, async {
        let a = gui.new_document().await;
        let b = gui.new_document().await;
        let area_a = gui.text_area(a).await;
        let area_b = gui.text_area(b).await;
        gui.ok(json!({"action": "focus", "ref": area_a})).await;

        // A second, independent hame process view raises B mid-typing,
        // as a person clicking another window would.
        let other_home = tempfile::tempdir().unwrap();
        let other = DesktopTool::new(config(other_home.path(), textedit_grants(all_caps())));
        let text = "abcdefghij".repeat(30);
        let typing = gui.call(json!({"action": "type", "ref": area_a, "text": text,
                                     "chunk": 2, "delay_ms": 40}));
        let interfere = async {
            tokio::time::sleep(Duration::from_millis(900)).await;
            let out = other
                .call(
                    &json!({"action": "focus", "app": TEXTEDIT, "window": b}),
                    &gui.ctx,
                )
                .await;
            assert!(!out.is_error, "{}", out.text);
        };
        let (out, ()) = tokio::join!(typing, interfere);
        assert!(
            out.is_error && out.text.starts_with("interference"),
            "{}",
            out.text
        );
        assert!(out.text.contains("Typed before stopping"), "{}", out.text);

        let obs_a = gui.observe(a).await;
        let value_a = text_area_value(&obs_a);
        assert!(!value_a.is_empty() && value_a.len() < 100, "{value_a}");
        assert!(text.starts_with(&value_a), "{value_a}");
        let obs_b = gui.observe(b).await;
        assert!(
            !line_of(&obs_b, " textarea").contains(" = "),
            "text leaked into B: {obs_b}"
        );
        let _ = area_b;
        // The receipt says the outcome needs reconciling, with exactly what
        // was emitted.
        let receipt = gui
            .receipts()
            .into_iter()
            .rfind(|r| r.action == "desktop.type")
            .unwrap();
        assert_eq!(receipt.status, ActionStatus::OutcomeUnknown);
        assert_eq!(receipt.emitted, vec![value_a.clone()]);
    });
}

/// Records every request the pipeline sees.
struct Recorder(Arc<Mutex<Vec<String>>>);

impl Layer<ToolRequest, ToolOutput> for Recorder {
    fn call<'a>(
        &'a self,
        request: ToolRequest,
        next: Next<'a, ToolRequest, ToolOutput>,
    ) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            self.0.lock().unwrap().push(format!(
                "{}:{}",
                request.call_id,
                request.input["action"].as_str().unwrap_or("")
            ));
            next.run(request).await
        })
    }
}

#[tokio::test]
#[ignore = "drives TextEdit; run with --ignored --test-threads=1"]
async fn a_batch_is_cancelled_between_chunks_and_reports_its_prefix() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let pipeline = Arc::new(ToolPipeline::new());
    pipeline.push(Recorder(Arc::clone(&seen)));
    let mut gui = Gui::start_with(textedit_grants(all_caps()), Some(pipeline)).await;
    gui_test!(gui, async {
        let doc = gui.new_document().await;
        let area = gui.text_area(doc).await;
        let text = "0123456789".repeat(30);
        let handle = gui.tool.cancel_handle();
        let batch = gui.call(json!({"action": "batch", "steps": [
            {"action": "focus", "ref": area},
            {"action": "type", "ref": area, "text": text, "chunk": 2, "delay_ms": 40},
            {"action": "set_value", "ref": area, "value": "SHOULD NOT HAPPEN"},
        ]}));
        let cancel = async {
            tokio::time::sleep(Duration::from_millis(1200)).await;
            handle.cancel();
        };
        let (out, ()) = tokio::join!(batch, cancel);
        assert!(out.is_error, "{}", out.text);
        assert!(
            out.text.contains("Batch: 1 of 3 steps completed"),
            "{}",
            out.text
        );
        assert!(out.text.contains("Stopped (cancelled)"), "{}", out.text);
        assert!(out.text.contains("Typed before stopping"), "{}", out.text);
        assert!(out.text.contains("Fresh observation:"), "{}", out.text);
        let obs = gui.observe(doc).await;
        let line = line_of(&obs, " textarea");
        assert!(!line.contains("SHOULD NOT"), "{line}");
        assert!(line.contains(" = \"0123"), "{line}");
        // Each step went through the pipeline as its own request.
        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen, vec!["desktop-batch-1:focus", "desktop-batch-2:type"]);
    });
}

#[tokio::test]
#[ignore = "drives TextEdit; run with --ignored --test-threads=1"]
async fn a_batch_stops_at_an_unexpected_save_sheet() {
    let mut gui = Gui::start().await;
    gui_test!(gui, async {
        let doc = gui.new_document().await;
        let area = gui.text_area(doc).await;
        let out = gui
            .call(json!({"action": "batch", "steps": [
                {"action": "set_value", "ref": area, "value": "unsaved work",
                 "expect": {"value": "unsaved work"}},
                {"action": "menu", "app": TEXTEDIT, "window": doc, "path": "File > Close"},
                {"action": "set_value", "ref": area, "value": "must not run"},
            ]}))
            .await;
        assert!(out.is_error, "{}", out.text);
        assert!(
            out.text.contains("Batch: 2 of 3 steps completed"),
            "{}",
            out.text
        );
        assert!(
            out.text.contains("Stopped (unexpected_dialog)"),
            "{}",
            out.text
        );
        assert!(
            out.text.contains("Fresh observation:") && out.text.contains("Dialog open: sheet"),
            "{}",
            out.text
        );
        assert!(!out.text.contains("must not run"));
        // A failed expectation stops a batch too.
        let doc2 = gui.new_document().await;
        let area2 = gui.text_area(doc2).await;
        let out = gui
            .call(json!({"action": "batch", "steps": [
                {"action": "set_value", "ref": area2, "value": "one", "expect": {"value": "two"}},
                {"action": "set_value", "ref": area2, "value": "three"},
            ]}))
            .await;
        assert!(
            out.text.contains("Stopped (assertion_failed)"),
            "{}",
            out.text
        );
        assert!(
            out.text.contains("Batch: 1 of 2 steps completed"),
            "{}",
            out.text
        );
    });
}

fn image_of(out: &ToolOutput) -> hame_session::ArtifactRef {
    out.parts
        .iter()
        .find_map(|p| match p {
            Block::Image { artifact } => Some(artifact.clone()),
            _ => None,
        })
        .expect("a screenshot returns an image part")
}

#[tokio::test]
#[ignore = "drives TextEdit; run with --ignored --test-threads=1"]
async fn screenshots_are_scaled_correctly_and_click_at_maps_into_the_window() {
    let mut gui = Gui::start().await;
    gui_test!(gui, async {
        let doc = gui.new_document().await;
        let area = gui.text_area(doc).await;
        gui.ok(
            json!({"action": "set_value", "ref": area, "value": "Screenshot me.\nSecond line."}),
        )
        .await;
        gui.ok(json!({"action": "focus", "app": TEXTEDIT, "window": doc}))
            .await;
        tokio::time::sleep(Duration::from_millis(300)).await;

        let full = gui
            .call(json!({"action": "screenshot", "app": TEXTEDIT, "window": doc}))
            .await;
        assert!(!full.is_error, "{}", full.text);
        let image = image_of(&full);
        let capture = image.capture.clone().unwrap();
        assert_eq!(capture.space, "screen-points");
        assert!(capture.scale >= 1.0);
        let (w, h) = (image.width.unwrap(), image.height.unwrap());
        assert!(
            (f64::from(w) - capture.crop[2] * capture.scale).abs() <= 1.0
                && (f64::from(h) - capture.crop[3] * capture.scale).abs() <= 1.0,
            "{w}x{h} for {:?} at {}",
            capture.crop,
            capture.scale
        );
        let bytes = gui.ctx.artifacts.as_ref().unwrap().get(&image).unwrap();
        assert!(bytes.starts_with(b"\x89PNG"));

        let small = gui
            .call(json!({"action": "screenshot", "app": TEXTEDIT, "window": doc, "max_edge": 600}))
            .await;
        assert!(!small.is_error, "{}", small.text);
        let small_image = image_of(&small);
        assert_eq!(
            small_image.width.unwrap().max(small_image.height.unwrap()),
            600
        );
        let small_capture = small_image.capture.clone().unwrap();
        assert_eq!(small_capture.crop, capture.crop);

        // Click the Bold button through the downscaled screenshot's
        // coordinates; the click must land on it and toggle it.
        let obs = gui
            .ok(json!({"action": "observe", "app": TEXTEDIT, "window": doc, "frames": true}))
            .await;
        let bold = line_of(&obs, "checkbox/segment \"bold\"");
        assert!(bold.contains("= \"0\""), "{bold}");
        let frame: Vec<f64> = bold
            .split('@')
            .nth(1)
            .unwrap()
            .split([',', ' ', 'x'])
            .filter_map(|n| n.parse().ok())
            .collect();
        let target = (frame[0] + frame[2] / 2.0, frame[1] + frame[3] / 2.0);
        let sent = (small_image.width.unwrap(), small_image.height.unwrap());
        let (ix, iy) = visual::screen_to_image(&small_capture, sent, target);
        let clicked = gui
            .ok(json!({"action": "click_at", "artifact": small_image.short(), "x": ix.round(), "y": iy.round()}))
            .await;
        assert!(clicked.contains(&format!("in window {doc}")), "{clicked}");
        tokio::time::sleep(Duration::from_millis(300)).await;
        let after = gui.observe(doc).await;
        assert!(
            line_of(&after, "checkbox/segment \"bold\"").contains("= \"1\""),
            "{after}"
        );

        // Out of the image: refused.
        let out = gui
            .call(
                json!({"action": "click_at", "artifact": small_image.short(), "x": 5000, "y": 10}),
            )
            .await;
        assert!(
            out.is_error && out.text.starts_with("invalid"),
            "{}",
            out.text
        );

        // After the window is resized, the old screenshot no longer maps.
        gui.ok(json!({"action": "menu", "app": TEXTEDIT, "window": doc, "path": "Window > Zoom"}))
            .await;
        tokio::time::sleep(Duration::from_millis(800)).await;
        let out = gui
            .call(json!({"action": "click_at", "artifact": small_image.short(), "x": ix.round(), "y": iy.round()}))
            .await;
        assert!(
            out.is_error && out.text.starts_with("stale_capture"),
            "{}",
            out.text
        );
        gui.ok(json!({"action": "menu", "app": TEXTEDIT, "window": doc, "path": "Window > Zoom"}))
            .await;
    });
}

#[tokio::test]
#[ignore = "uses the desktop helper and the clipboard; run with --ignored --test-threads=1"]
async fn the_clipboard_is_saved_and_restored() {
    let home = tempfile::tempdir().unwrap();
    let tool = DesktopTool::new(config(home.path(), textedit_grants(all_caps())));
    let ctx = ToolContext::new(home.path());
    let get_input = json!({"action": "clipboard", "mode": "get"});
    let get = || tool.call(&get_input, &ctx);
    let before = get().await.text;
    let set = tool
        .call(
            &json!({"action": "clipboard", "mode": "set", "value": VIETNAMESE}),
            &ctx,
        )
        .await;
    assert!(!set.is_error, "{}", set.text);
    assert!(get().await.text.contains(VIETNAMESE));
    let restored = tool
        .call(&json!({"action": "clipboard", "mode": "restore"}), &ctx)
        .await;
    assert!(restored.text.starts_with("Restored"), "{}", restored.text);
    assert_eq!(get().await.text, before);
    // Without a grant the clipboard is refused.
    let none = DesktopTool::new(config(home.path(), Authorization::default()));
    let out = none
        .call(&json!({"action": "clipboard", "mode": "get"}), &ctx)
        .await;
    assert!(
        out.is_error && out.text.starts_with("not_authorized"),
        "{}",
        out.text
    );
}

#[tokio::test]
#[ignore = "builds and runs the desktop helper; run with --ignored --test-threads=1"]
async fn a_missing_accessibility_permission_is_reported_explicitly() {
    let home = tempfile::tempdir().unwrap();
    let mut config = config(home.path(), textedit_grants(all_caps()));
    config.simulate_untrusted = true;
    let tool = DesktopTool::new(config);
    let ctx = ToolContext::new(home.path());
    let hello = tool.hello().await.unwrap();
    assert!(!hello.ax_trusted);
    assert_eq!(
        hello.manifest.nested_usage,
        hame_session::contract::NestedUsage::None
    );
    for input in [
        json!({"action": "observe", "app": "com.apple.finder"}),
        json!({"action": "windows", "app": "com.apple.finder"}),
        json!({"action": "press", "app": "com.apple.finder", "name": "x"}),
    ] {
        let out = tool.call(&input, &ctx).await;
        assert!(out.is_error, "{}", out.text);
        assert!(out.text.starts_with("permission_denied"), "{}", out.text);
        assert!(
            out.text.contains("Privacy & Security > Accessibility"),
            "{}",
            out.text
        );
    }
}

fn median_p95(mut ms: Vec<f64>) -> (f64, f64) {
    ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p95 = ms[((ms.len() - 1) as f64 * 0.95).round() as usize];
    (ms[ms.len() / 2], p95)
}

/// Per-operation latency through the whole tool (authorization, journal
/// fsyncs, helper round trip), for the backend evidence. Prints numbers.
#[tokio::test]
#[ignore = "drives TextEdit; run with --ignored --test-threads=1 --nocapture"]
async fn tool_level_latency() {
    let mut gui = Gui::start().await;
    gui_test!(gui, async {
        let doc = gui.new_document().await;
        let area = gui.text_area(doc).await;
        let mut observe = Vec::new();
        let mut set = Vec::new();
        let mut windows = Vec::new();
        for i in 0..12 {
            let t = Instant::now();
            gui.windows().await;
            windows.push(t.elapsed().as_secs_f64() * 1000.0);
            let t = Instant::now();
            gui.observe(doc).await;
            observe.push(t.elapsed().as_secs_f64() * 1000.0);
            let value = if i % 2 == 0 {
                VIETNAMESE
            } else {
                "Đ ơ ư — lần hai"
            };
            let t = Instant::now();
            gui.ok(json!({"action": "set_value", "ref": area, "value": value}))
                .await;
            set.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        // The same through a context without a journal: the difference is
        // the cost of the fsynced receipts.
        let bare = ToolContext::new(gui.ctx.cwd.clone()).with_session("gui-test");
        let mut unjournalled = Vec::new();
        for i in 0..12 {
            let value = if i % 2 == 0 { VIETNAMESE } else { "Đ ơ ư" };
            let input = json!({"action": "set_value", "ref": area, "value": value});
            let t = Instant::now();
            let out = gui.tool.call(&input, &bare).await;
            unjournalled.push(t.elapsed().as_secs_f64() * 1000.0);
            assert!(!out.is_error, "{}", out.text);
        }
        let (um, up) = median_p95(unjournalled);
        println!("tool set_value (no journal): median {um:.1} ms, p95 {up:.1} ms, n=12");
        let (om, op) = median_p95(observe);
        let (sm, sp) = median_p95(set);
        let (wm, wp) = median_p95(windows);
        println!("tool windows: median {wm:.1} ms, p95 {wp:.1} ms, n=12");
        println!("tool observe: median {om:.1} ms, p95 {op:.1} ms, n=12");
        println!("tool set_value (journalled): median {sm:.1} ms, p95 {sp:.1} ms, n=12");
    });
}

const NUMBERS: &str = "com.apple.iWork.Numbers";

fn numbers_grants() -> Authorization {
    let caps = all_caps();
    Authorization {
        grants: [Channel::Desktop, Channel::Visual]
            .into_iter()
            .map(|channel| Grant {
                channel,
                app: Some(NUMBERS.into()),
                origin: None,
                profile: None,
                capabilities: caps.clone(),
            })
            .collect(),
    }
}

/// Numbers draws its sheet on a canvas it exposes only on request; the
/// observation asks for it, shows cells with their addresses, `select`
/// reaches an empty cell, typing a formula follows Numbers' separate
/// formula editor, and a range the table does not take is reported.
/// Works on a copy of the O02 pilot fixture in a temporary folder.
#[tokio::test]
#[ignore = "drives Numbers; run with --ignored --test-threads=1"]
async fn numbers_cells_are_observed_selected_and_typed_into() {
    let dir = tempfile::tempdir().unwrap();
    let doc = dir.path().join("order.numbers");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../evals/pilot/O02/assets/order.numbers"),
        &doc,
    )
    .unwrap();
    let was_running = Command::new("pgrep")
        .args(["-x", "Numbers"])
        .output()
        .is_ok_and(|o| o.status.success());
    assert!(
        Command::new("open")
            .args(["-g", "-a", "Numbers"])
            .arg(&doc)
            .status()
            .unwrap()
            .success()
    );
    let home = tempfile::tempdir().unwrap();
    let journal = Arc::new(ActionJournal::open(home.path().join("actions.v1.jsonl")).unwrap());
    let ctx = ToolContext::new(home.path())
        .with_artifacts(ArtifactStore::new(home.path().join("artifacts")))
        .with_journal(journal)
        .with_session("gui-numbers");
    let tool = DesktopTool::new(config(home.path(), numbers_grants()));
    let call = |input: Value| {
        let tool = &tool;
        let ctx = &ctx;
        async move { tool.call(&input, ctx).await }
    };
    let window = {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let out = call(json!({"action": "windows", "app": NUMBERS})).await;
            if let Some((id, _)) = windows_of(&out.text)
                .into_iter()
                .find(|(_, t)| t.starts_with("order"))
            {
                break id;
            }
            assert!(Instant::now() < deadline, "no order window: {}", out.text);
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    };
    let result = AssertUnwindSafe(async {
        let obs =
            call(json!({"action": "observe", "app": NUMBERS, "window": window, "max_nodes": 60}))
                .await
                .text;
        assert!(obs.contains("cell A2 \"Notebook\""), "{obs}");
        assert!(obs.contains("cell C2 \"25000\""), "{obs}");
        let table = find_ref(&obs, " table ").expect("a table ref");
        // No focus first: selecting a cell brings its window to the front,
        // so typing can follow.
        let out = call(json!({"action": "select", "ref": table, "cell": "D2"})).await;
        assert!(
            out.text.contains(": D2.") && out.text.contains("window is in front"),
            "{}",
            out.text
        );
        let out =
            call(json!({"action": "type", "app": NUMBERS, "window": window, "text": "=B2*C2\n"}))
                .await;
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.contains("editor window"), "{}", out.text);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let obs = call(
                json!({"action": "observe", "app": NUMBERS, "window": window, "max_nodes": 60}),
            )
            .await
            .text;
            if obs.contains("cell D2 \"250000, Formula result\"") {
                break;
            }
            assert!(Instant::now() < deadline, "the formula never landed: {obs}");
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        // Values typed with tabs move along the row inside the cell editor.
        let obs =
            call(json!({"action": "observe", "app": NUMBERS, "window": window, "max_nodes": 5}))
                .await
                .text;
        let table = find_ref(&obs, " table ").expect("a table ref");
        let out = call(json!({"action": "select", "ref": table, "cell": "B3"})).await;
        assert!(out.text.contains(": B3."), "{}", out.text);
        let out =
            call(json!({"action": "type", "app": NUMBERS, "window": window, "text": "7\t61000\n"}))
                .await;
        assert!(!out.is_error, "{}", out.text);
        let obs =
            call(json!({"action": "observe", "app": NUMBERS, "window": window, "max_nodes": 60}))
                .await
                .text;
        assert!(
            obs.contains("cell B3 \"7\"") && obs.contains("cell C3 \"61000\""),
            "{obs}"
        );
        // A range the table does not take is reported, not claimed.
        let table = find_ref(&obs, " table ").expect("a table ref");
        let out = call(json!({"action": "select", "ref": table, "cell": "A2:B3"})).await;
        assert!(out.text.contains("selected only"), "{}", out.text);
    })
    .catch_unwind()
    .await;
    let _ =
        call(json!({"action": "key", "app": NUMBERS, "window": window, "keys": "escape"})).await;
    let _ =
        call(json!({"action": "menu", "app": NUMBERS, "window": window, "path": "File > Close"}))
            .await;
    if !was_running {
        let _ =
            call(json!({"action": "menu", "app": NUMBERS, "path": "Numbers > Quit Numbers"})).await;
    }
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
