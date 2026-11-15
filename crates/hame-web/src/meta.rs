//! What the web interface remembers about a session beyond its log: the
//! title the user gave it, whether it is pinned, and the permission mode
//! and model chosen for it.
//!
//! Kept in one JSON file in hame's data directory (`web/sessions.json`),
//! which the agent's tools cannot read or write like everything else there.
//! Every change holds a file lock (`web/sessions.lock`), so two hame
//! processes do not lose each other's changes, and replaces the file
//! through a uniquely named temporary file. A file that cannot be read is
//! kept as a backup and reported, never silently replaced.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// How much the agent may do without asking, as the interface offers it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    /// hame's configured behaviour: confined tools run without asking.
    #[default]
    Default,
    /// Nothing may be changed: every call that is not read-only is denied.
    ReadOnly,
    /// Every call that is not read-only asks first.
    Ask,
    /// File edits inside the session's folder run without asking; the rest
    /// asks.
    AcceptEdits,
    /// No sandbox and no questions.
    FullAccess,
}

impl Mode {
    pub const ALL: [Mode; 5] = [
        Mode::Default,
        Mode::ReadOnly,
        Mode::Ask,
        Mode::AcceptEdits,
        Mode::FullAccess,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Mode::Default => "default",
            Mode::ReadOnly => "read-only",
            Mode::Ask => "ask",
            Mode::AcceptEdits => "accept-edits",
            Mode::FullAccess => "full-access",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Mode::Default => "Sandboxed",
            Mode::ReadOnly => "Read only",
            Mode::Ask => "Ask",
            Mode::AcceptEdits => "Accept edits",
            Mode::FullAccess => "Full access",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Mode::Default => "hame's configured rules: tools run in the sandbox without asking",
            Mode::ReadOnly => "reads and searches only; every change is refused",
            Mode::Ask => {
                "asks before every change and command, except what an allow rule in config.toml \
                 lets through"
            }
            Mode::AcceptEdits => {
                "file edits inside the session's folder run; commands and everything else ask"
            }
            Mode::FullAccess => "no sandbox and no questions; use with care",
        }
    }

    pub fn parse(name: &str) -> Option<Mode> {
        Mode::ALL.into_iter().find(|mode| mode.name() == name)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
    #[serde(default, skip_serializing_if = "is_default_mode")]
    pub mode: Mode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

fn is_default_mode(mode: &Mode) -> bool {
    *mode == Mode::Default
}

type All = BTreeMap<String, SessionMeta>;

/// The metadata file, read and written whole under a lock.
pub struct MetaStore {
    path: PathBuf,
    lock: Mutex<()>,
    /// What went wrong reading the file, for `/api/info`.
    warning: Mutex<Option<String>>,
}

fn stamp() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

impl MetaStore {
    pub fn new(home: &Path) -> Self {
        Self {
            path: home.join("web").join("sessions.json"),
            lock: Mutex::new(()),
            warning: Mutex::new(None),
        }
    }

    /// A problem reading the file, if there was one.
    pub fn warning(&self) -> Option<String> {
        self.warning.lock().expect("meta warning").clone()
    }

    /// Takes the lock other hame processes take too.
    fn file_lock(&self) -> Result<File, String> {
        let dir = self.path.parent().expect("sessions.json has a folder");
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join("sessions.lock"))
            .map_err(|e| e.to_string())?;
        file.lock().map_err(|e| e.to_string())?;
        Ok(file)
    }

    /// Reads the file. One that exists but cannot be read is moved aside
    /// to a backup (once) and reported; the sessions then start from
    /// defaults instead of the server refusing to work.
    fn load(&self) -> All {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return All::new(),
            Err(e) => {
                self.report(format!("cannot read {}: {e}", self.path.display()));
                return All::new();
            }
        };
        match serde_json::from_slice(&bytes) {
            Ok(all) => all,
            Err(e) => {
                let backup = self
                    .path
                    .with_extension(format!("json.unreadable-{}", stamp()));
                let kept = std::fs::rename(&self.path, &backup).is_ok();
                self.report(format!(
                    "{} could not be read ({e}); titles, pins, modes and models of web sessions \
                     start from defaults.{}",
                    self.path.display(),
                    if kept {
                        format!(" The old file is kept as {}.", backup.display())
                    } else {
                        String::new()
                    }
                ));
                All::new()
            }
        }
    }

    fn report(&self, warning: String) {
        eprintln!("hame web: {warning}");
        *self.warning.lock().expect("meta warning") = Some(warning);
    }

    pub fn get(&self, id: &str) -> SessionMeta {
        self.all().remove(id).unwrap_or_default()
    }

    pub fn all(&self) -> All {
        let _guard = self.lock.lock().expect("meta lock");
        let _file = self.file_lock();
        self.load()
    }

    /// Changes one session's entry; `None` from `change` removes it.
    pub fn update(
        &self,
        id: &str,
        change: impl FnOnce(SessionMeta) -> Option<SessionMeta>,
    ) -> Result<SessionMeta, String> {
        let _guard = self.lock.lock().expect("meta lock");
        let _file = self.file_lock()?;
        let mut all = self.load();
        let current = all.remove(id).unwrap_or_default();
        let next = change(current);
        if let Some(next) = &next
            && *next != SessionMeta::default()
        {
            all.insert(id.to_string(), next.clone());
        }
        let bytes = serde_json::to_vec_pretty(&all).map_err(|e| e.to_string())?;
        let temp = self
            .path
            .with_extension(format!("json.{}-{}.tmp", std::process::id(), stamp()));
        std::fs::write(&temp, bytes).map_err(|e| e.to_string())?;
        std::fs::rename(&temp, &self.path).map_err(|e| {
            let _ = std::fs::remove_file(&temp);
            e.to_string()
        })?;
        Ok(next.unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_survive_a_restart_and_defaults_are_not_stored() {
        let dir = tempfile::tempdir().unwrap();
        let store = MetaStore::new(dir.path());
        store
            .update("a", |mut meta| {
                meta.title = Some("Fix the build".into());
                meta.mode = Mode::Ask;
                Some(meta)
            })
            .unwrap();
        store.update("b", Some).unwrap();
        let again = MetaStore::new(dir.path());
        assert_eq!(again.get("a").title.as_deref(), Some("Fix the build"));
        assert_eq!(again.get("a").mode, Mode::Ask);
        assert_eq!(again.all().len(), 1, "an all-default entry is not kept");
        again.update("a", |_| None).unwrap();
        assert!(again.all().is_empty());
        assert_eq!(again.warning(), None);
        // No temporary file is left behind.
        let left: Vec<_> = std::fs::read_dir(dir.path().join("web"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(left.is_empty(), "{left:?}");
    }

    #[test]
    fn an_unreadable_file_is_kept_as_a_backup_and_reported() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("web")).unwrap();
        std::fs::write(dir.path().join("web/sessions.json"), b"{not json").unwrap();
        let store = MetaStore::new(dir.path());
        assert_eq!(store.get("a"), SessionMeta::default());
        let warning = store.warning().expect("the problem is reported");
        assert!(warning.contains("could not be read"), "{warning}");
        let backups: Vec<_> = std::fs::read_dir(dir.path().join("web"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter(|name| name.contains("unreadable"))
            .collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(
            std::fs::read(dir.path().join("web").join(&backups[0])).unwrap(),
            b"{not json"
        );
        // Changes still work from defaults.
        store
            .update("a", |mut meta| {
                meta.pinned = true;
                Some(meta)
            })
            .unwrap();
        assert!(store.get("a").pinned);
    }

    #[test]
    fn modes_have_stable_names() {
        for mode in Mode::ALL {
            assert_eq!(Mode::parse(mode.name()), Some(mode));
            assert_eq!(
                serde_json::to_value(mode).unwrap(),
                serde_json::Value::String(mode.name().into())
            );
        }
        assert_eq!(Mode::parse("yolo"), None);
    }
}
