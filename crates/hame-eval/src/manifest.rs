//! Task sets: a versioned TOML manifest, or a directory of `task.toml`
//! tasks in the older layout. Both load into the same [`TaskSpec`].
//!
//! A manifest (`manifest.toml`) lists tasks with an id, a prompt and its
//! translations, the channels the agent may use, how the workspace is set
//! up and reset, side effects that are forbidden, the hidden checker and a
//! time limit. Each task's files live in a directory beside the manifest:
//!
//! ```text
//! F01/files/      copied into a fresh workspace for every run
//! F01/check/      the hidden checker; never copied, exported as $HAME_CHECK_DIR
//! F01/solution/   a reference solution (solve.py), for testing the checker
//! ```
//!
//! The whole task set directory is made unreadable to the agent's tools by
//! the sandbox, so the agent sees neither checker nor solution.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

/// What hame's eval runner can always provide. `browser` and `desktop` are
/// added by the runner when this machine has them (see
/// [`TaskSpec::missing_capabilities`]); tasks that need anything else are
/// reported as unsupported rather than run and failed.
pub const SUPPORTED_CAPABILITIES: &[&str] = &["files", "shell"];

/// The categories of the pilot diagnostic suite. Others are accepted, for
/// task sets this list does not know about yet.
pub const CATEGORIES: &[&str] = &[
    "file",
    "browser",
    "desktop",
    "office",
    "cross_app",
    "recovery",
];

#[derive(Debug, Clone, PartialEq)]
pub struct TaskSet {
    pub name: String,
    /// Identifies the exact task definitions: the manifest's version, or a
    /// content hash for a directory of `task.toml` files.
    pub version: String,
    pub root: PathBuf,
    pub tasks: Vec<TaskSpec>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TaskSpec {
    pub id: String,
    pub title: Option<String>,
    pub category: String,
    /// The English prompt.
    pub prompt: String,
    /// Translations by language code, e.g. `vi`.
    pub prompts: BTreeMap<String, String>,
    /// How the agent may act: `files`, `shell`, `browser`, `desktop`, ...
    pub channels: Vec<String>,
    /// What the runner must provide for the task to run at all.
    pub requires: Vec<String>,
    /// Copied into the fresh workspace.
    pub fixture: Option<PathBuf>,
    /// Older layout: copied over the workspace before the check, so tests
    /// the agent edited are put back.
    pub overlay: Option<PathBuf>,
    /// Exported to the checker as `$HAME_CHECK_DIR`.
    pub check_dir: Option<PathBuf>,
    pub solution: Option<PathBuf>,
    /// Shell command run in the workspace after the fixture is copied.
    pub setup: Option<String>,
    /// `fresh_copy`: every run starts from a new copy of the fixture in a
    /// new directory, with a new data directory for the agent.
    pub reset: String,
    /// Shell command run in the workspace afterwards; exit 0 means pass.
    pub check: String,
    /// Side effects the task forbids, in words (enforced by the checker).
    pub forbidden: Vec<String>,
    /// Workspace paths that must be byte-for-byte unchanged afterwards
    /// (enforced by the runner; a change is a forbidden effect).
    pub unchanged: Vec<String>,
    pub timeout_secs: Option<u64>,
    /// Hidden from the agent's tools.
    pub hidden_root: PathBuf,
    /// The task's own directory (`$HAME_TASK_DIR`).
    pub dir: PathBuf,
    /// A live fixture: a shell command started before setup that prints
    /// `PORT <n>` (or `URL <url>`) as its first line and serves until its
    /// standard input closes. Its address is `$HAME_FIXTURE_URL` and
    /// `{fixture_url}` in the prompt. Started fresh for every run, so its
    /// state is reset; stopped after the check.
    pub server: Option<String>,
    /// Started beside the agent and stopped when it finishes: fault
    /// injection and interference (a modal, a cancelled dialog, a kill).
    pub background: Option<String>,
    /// Run after the check whatever happened, to put the machine back
    /// (close windows, quit an application the run started).
    pub teardown: Option<String>,
    /// When the background process killed the agent (it touched
    /// `$HAME_RUN_DIR/agent.killed`), the session is continued once with
    /// this prompt, as a user would after a crash.
    pub resume: Option<String>,
    /// Merged into the agent's config.toml for this task (the arm's own
    /// settings win; arrays are joined), e.g. the `[computer]` grants.
    pub config: toml::Table,
}

impl TaskSpec {
    /// The prompt in `lang`, falling back to English.
    pub fn prompt_for(&self, lang: Option<&str>) -> &str {
        lang.and_then(|lang| self.prompts.get(lang))
            .map_or(self.prompt.as_str(), String::as_str)
    }

    /// The required capabilities that neither [`SUPPORTED_CAPABILITIES`]
    /// nor `available` (what this machine offers besides) provide.
    pub fn missing_capabilities(&self, available: &[&str]) -> Vec<String> {
        self.requires
            .iter()
            .filter(|need| {
                !SUPPORTED_CAPABILITIES.contains(&need.as_str())
                    && !available.contains(&need.as_str())
            })
            .cloned()
            .collect()
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    task_set: String,
    version: String,
    #[serde(default)]
    defaults: Defaults,
    #[serde(rename = "task", default)]
    tasks: Vec<ManifestTask>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Defaults {
    channels: Option<Vec<String>>,
    requires: Option<Vec<String>>,
    timeout_secs: Option<u64>,
    check: Option<String>,
    server: Option<String>,
    teardown: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestTask {
    id: String,
    title: Option<String>,
    #[serde(default = "file_category")]
    category: String,
    /// Directory of the task's files, relative to the manifest. Defaults
    /// to the id.
    dir: Option<String>,
    prompt: String,
    #[serde(default)]
    prompts: BTreeMap<String, String>,
    channels: Option<Vec<String>>,
    requires: Option<Vec<String>>,
    setup: Option<String>,
    #[serde(default = "fresh_copy")]
    reset: String,
    check: Option<String>,
    #[serde(default)]
    forbidden: Vec<String>,
    #[serde(default)]
    unchanged: Vec<String>,
    timeout_secs: Option<u64>,
    server: Option<String>,
    background: Option<String>,
    teardown: Option<String>,
    resume: Option<String>,
    #[serde(default)]
    config: toml::Table,
}

fn file_category() -> String {
    "file".into()
}

fn fresh_copy() -> String {
    "fresh_copy".into()
}

/// The older per-task file, `task.toml`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyTask {
    prompt: String,
    check: String,
    setup: Option<String>,
}

pub const MANIFEST_FILE: &str = "manifest.toml";

/// Loads a manifest file, a directory holding `manifest.toml`, or a
/// directory of `task.toml` tasks. `filter` keeps tasks whose id contains
/// it.
pub fn load(path: &Path, filter: Option<&str>) -> Result<TaskSet, String> {
    let manifest = if path.is_file() {
        Some(path.to_path_buf())
    } else if path.join(MANIFEST_FILE).is_file() {
        Some(path.join(MANIFEST_FILE))
    } else {
        None
    };
    let mut set = match manifest {
        Some(file) => load_manifest(&file)?,
        None => load_legacy(path)?,
    };
    set.tasks
        .retain(|task| filter.is_none_or(|f| task.id.contains(f)));
    if set.tasks.is_empty() {
        return Err(format!("no tasks found in {}", path.display()));
    }
    Ok(set)
}

pub fn parse_manifest(text: &str, root: &Path) -> Result<TaskSet, String> {
    let manifest: Manifest = toml::from_str(text).map_err(|e| e.to_string())?;
    let mut seen = std::collections::BTreeSet::new();
    let mut tasks = Vec::new();
    for task in manifest.tasks {
        if !seen.insert(task.id.clone()) {
            return Err(format!("task id {} appears twice", task.id));
        }
        if task.reset != "fresh_copy" {
            return Err(format!(
                "task {}: reset {:?} is not supported; use \"fresh_copy\"",
                task.id, task.reset
            ));
        }
        let dir = root.join(task.dir.as_deref().unwrap_or(&task.id));
        let existing = |name: &str| Some(dir.join(name)).filter(|path| path.is_dir());
        let check = task
            .check
            .or_else(|| manifest.defaults.check.clone())
            .ok_or_else(|| format!("task {} has no check", task.id))?;
        tasks.push(TaskSpec {
            title: task.title,
            category: task.category,
            prompt: task.prompt,
            prompts: task.prompts,
            channels: task
                .channels
                .or_else(|| manifest.defaults.channels.clone())
                .unwrap_or_else(|| vec!["files".into(), "shell".into()]),
            requires: task
                .requires
                .or_else(|| manifest.defaults.requires.clone())
                .unwrap_or_else(|| vec!["files".into(), "shell".into()]),
            fixture: existing("files"),
            overlay: None,
            check_dir: existing("check"),
            solution: existing("solution"),
            setup: task.setup,
            reset: task.reset,
            check,
            forbidden: task.forbidden,
            unchanged: task.unchanged,
            timeout_secs: task.timeout_secs.or(manifest.defaults.timeout_secs),
            hidden_root: root.to_path_buf(),
            server: task.server.or_else(|| manifest.defaults.server.clone()),
            background: task.background,
            teardown: task.teardown.or_else(|| manifest.defaults.teardown.clone()),
            resume: task.resume,
            config: task.config,
            dir,
            id: task.id,
        });
    }
    tasks.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(TaskSet {
        name: manifest.task_set,
        version: manifest.version,
        root: root.to_path_buf(),
        tasks,
    })
}

fn load_manifest(file: &Path) -> Result<TaskSet, String> {
    let text = std::fs::read_to_string(file)
        .map_err(|e| format!("cannot read {}: {e}", file.display()))?;
    let root = file.parent().unwrap_or(Path::new("."));
    parse_manifest(&text, root).map_err(|e| format!("{}: {e}", file.display()))
}

fn load_legacy(suite: &Path) -> Result<TaskSet, String> {
    let mut entries: Vec<(String, PathBuf)> = std::fs::read_dir(suite)
        .map_err(|e| format!("cannot read {}: {e}", suite.display()))?
        .flatten()
        .filter(|entry| entry.path().join("task.toml").is_file())
        .map(|entry| {
            (
                entry.file_name().to_string_lossy().into_owned(),
                entry.path(),
            )
        })
        .collect();
    entries.sort();
    let mut digest = Sha256::new();
    let mut tasks = Vec::new();
    for (name, dir) in entries {
        let text = std::fs::read_to_string(dir.join("task.toml"))
            .map_err(|e| format!("{name}: cannot read task.toml: {e}"))?;
        digest.update(name.as_bytes());
        digest.update(text.as_bytes());
        let task: LegacyTask = toml::from_str(&text).map_err(|e| format!("{name}: {e}"))?;
        let existing = |sub: &str| Some(dir.join(sub)).filter(|path| path.is_dir());
        tasks.push(TaskSpec {
            id: name,
            title: None,
            category: "file".into(),
            prompt: task.prompt,
            prompts: BTreeMap::new(),
            channels: vec!["files".into(), "shell".into()],
            requires: vec!["files".into(), "shell".into()],
            fixture: existing("files"),
            overlay: existing("check"),
            check_dir: None,
            solution: None,
            setup: task.setup,
            reset: "fresh_copy".into(),
            check: task.check,
            forbidden: Vec::new(),
            unchanged: Vec::new(),
            timeout_secs: None,
            hidden_root: suite.to_path_buf(),
            dir: dir.clone(),
            server: None,
            background: None,
            teardown: None,
            resume: None,
            config: toml::Table::new(),
        });
    }
    Ok(TaskSet {
        name: suite.file_name().map_or_else(
            || "suite".into(),
            |name| name.to_string_lossy().into_owned(),
        ),
        version: format!("sha256:{}", &hex(&digest.finalize())[..16]),
        root: suite.to_path_buf(),
        tasks,
    })
}

/// Replaces `{name}` in `text` with each variable's value; anything else
/// in braces is left as it is.
pub fn substitute(text: &str, vars: &[(&str, &str)]) -> String {
    let mut out = text.to_string();
    for (name, value) in vars {
        out = out.replace(&format!("{{{name}}}"), value);
    }
    out
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// The SHA-256 of every file under `root`, by `/`-separated relative path.
/// Version-control directories are skipped.
pub fn file_hashes(root: &Path) -> io::Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            let kind = entry.file_type()?;
            if kind.is_dir() {
                if !matches!(entry.file_name().to_str(), Some(".git" | ".hg" | ".svn")) {
                    stack.push(path);
                }
            } else if kind.is_file() {
                let relative = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .components()
                    .map(|part| part.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                out.insert(relative, sha256_hex(&std::fs::read(&path)?));
            }
        }
    }
    Ok(out)
}

/// One hash for a whole tree: identifies an initial state.
pub fn tree_hash(root: &Path) -> io::Result<String> {
    let mut digest = Sha256::new();
    for (path, hash) in file_hashes(root)? {
        digest.update(path.as_bytes());
        digest.update([0]);
        digest.update(hash.as_bytes());
        digest.update(b"\n");
    }
    Ok(format!("sha256:{}", &hex(&digest.finalize())[..16]))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"
task_set = "pilot"
version = "2026-11-15.1"

[defaults]
timeout_secs = 300
check = "python3 \"$HAME_CHECK_DIR/check.py\""
teardown = "true"

[[task]]
id = "F02"
title = "Repair"
prompt = "Fix it."
prompts = { vi = "Sửa nó." }
unchanged = ["input/orders.csv"]
forbidden = ["editing the input"]

[[task]]
id = "B01"
category = "browser"
prompt = "Fill the form."
channels = ["browser"]
requires = ["browser"]
check = "true"
timeout_secs = 60
server = "python3 app.py"
resume = "Continue."
config = { computer = { browser = true } }
"#;

    #[test]
    fn a_manifest_parses_with_defaults_and_extension_points() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("F02/files")).unwrap();
        std::fs::create_dir_all(dir.path().join("F02/check")).unwrap();
        let set = parse_manifest(MANIFEST, dir.path()).unwrap();
        assert_eq!(
            (set.name.as_str(), set.version.as_str()),
            ("pilot", "2026-11-15.1")
        );
        let ids: Vec<_> = set.tasks.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["B01", "F02"], "sorted by id");
        let f02 = &set.tasks[1];
        assert_eq!(f02.category, "file");
        assert_eq!(f02.timeout_secs, Some(300));
        assert_eq!(f02.check, "python3 \"$HAME_CHECK_DIR/check.py\"");
        assert_eq!(f02.channels, ["files", "shell"]);
        assert_eq!(f02.prompt_for(Some("vi")), "Sửa nó.");
        assert_eq!(f02.prompt_for(Some("fr")), "Fix it.");
        assert_eq!(f02.prompt_for(None), "Fix it.");
        assert_eq!(f02.fixture, Some(dir.path().join("F02/files")));
        assert_eq!(f02.check_dir, Some(dir.path().join("F02/check")));
        assert_eq!(f02.solution, None);
        assert_eq!(f02.unchanged, ["input/orders.csv"]);
        assert!(f02.missing_capabilities(&[]).is_empty());
        let b01 = &set.tasks[0];
        assert_eq!(b01.timeout_secs, Some(60));
        assert_eq!(b01.missing_capabilities(&[]), ["browser"]);
        assert!(b01.missing_capabilities(&["browser"]).is_empty());
        assert_eq!(b01.fixture, None);
        assert_eq!(b01.server.as_deref(), Some("python3 app.py"));
        assert_eq!(b01.resume.as_deref(), Some("Continue."));
        assert_eq!(b01.teardown.as_deref(), Some("true"), "from the defaults");
        assert_eq!(b01.config["computer"]["browser"].as_bool(), Some(true));
        assert_eq!(b01.dir, dir.path().join("B01"));
        assert_eq!(f02.server, None);

        let twice = format!("{MANIFEST}\n[[task]]\nid = \"F02\"\nprompt = \"again\"\n");
        assert!(
            parse_manifest(&twice, dir.path())
                .unwrap_err()
                .contains("twice")
        );
        let unknown = MANIFEST.replace("title = \"Repair\"", "titel = \"Repair\"");
        assert!(
            parse_manifest(&unknown, dir.path()).is_err(),
            "typos are errors"
        );
        let reset = MANIFEST.replace("title = \"Repair\"", "reset = \"snapshot\"");
        assert!(
            parse_manifest(&reset, dir.path())
                .unwrap_err()
                .contains("reset")
        );
    }

    #[test]
    fn the_older_layout_loads_into_the_same_shape() {
        let dir = tempfile::tempdir().unwrap();
        let task = dir.path().join("rename");
        std::fs::create_dir_all(task.join("check")).unwrap();
        std::fs::write(task.join("task.toml"), "prompt = \"p\"\ncheck = \"true\"\n").unwrap();
        let set = load(dir.path(), None).unwrap();
        assert_eq!(set.tasks.len(), 1);
        assert_eq!(set.tasks[0].overlay, Some(task.join("check")));
        assert!(set.version.starts_with("sha256:"));
        assert!(load(dir.path(), Some("nothing")).is_err());
    }

    #[test]
    fn prompt_variables_are_substituted() {
        let vars = [("fixture_url", "http://127.0.0.1:5"), ("workspace", "/w")];
        assert_eq!(
            substitute(
                "Open {fixture_url}/form, save in {workspace}/out {x}",
                &vars
            ),
            "Open http://127.0.0.1:5/form, save in /w/out {x}"
        );
    }

    #[test]
    fn tree_hashes_follow_content_and_paths() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a/.git")).unwrap();
        std::fs::write(dir.path().join("a/x.txt"), "1").unwrap();
        std::fs::write(dir.path().join("a/.git/HEAD"), "ignored").unwrap();
        let hashes = file_hashes(dir.path()).unwrap();
        assert_eq!(hashes.keys().collect::<Vec<_>>(), ["a/x.txt"]);
        let before = tree_hash(dir.path()).unwrap();
        std::fs::write(dir.path().join("a/x.txt"), "2").unwrap();
        assert_ne!(before, tree_hash(dir.path()).unwrap());
    }
}
