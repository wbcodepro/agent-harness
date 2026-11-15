//! What the web interface remembers about a session beyond its log: the
//! title the user gave it, whether it is pinned, and the permission mode
//! and model chosen for it.
//!
//! Kept in one JSON file in hame's data directory (`web/sessions.json`),
//! which the agent's tools cannot read or write like everything else there.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

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
    /// File edits run without asking; commands and the rest ask.
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
            Mode::Ask => "asks before every change and every command",
            Mode::AcceptEdits => "file edits run; commands and other actions ask",
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

/// The metadata file, read and written whole under a lock.
pub struct MetaStore {
    path: PathBuf,
    lock: Mutex<()>,
}

impl MetaStore {
    pub fn new(home: &Path) -> Self {
        Self {
            path: home.join("web").join("sessions.json"),
            lock: Mutex::new(()),
        }
    }

    fn load(&self) -> BTreeMap<String, SessionMeta> {
        std::fs::read(&self.path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn get(&self, id: &str) -> SessionMeta {
        let _guard = self.lock.lock().expect("meta lock");
        self.load().remove(id).unwrap_or_default()
    }

    pub fn all(&self) -> BTreeMap<String, SessionMeta> {
        let _guard = self.lock.lock().expect("meta lock");
        self.load()
    }

    /// Changes one session's entry; `None` from `change` removes it.
    pub fn update(
        &self,
        id: &str,
        change: impl FnOnce(SessionMeta) -> Option<SessionMeta>,
    ) -> Result<SessionMeta, String> {
        let _guard = self.lock.lock().expect("meta lock");
        let mut all = self.load();
        let current = all.remove(id).unwrap_or_default();
        let next = change(current);
        if let Some(next) = &next
            && *next != SessionMeta::default()
        {
            all.insert(id.to_string(), next.clone());
        }
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let bytes = serde_json::to_vec_pretty(&all).map_err(|e| e.to_string())?;
        let temp = self.path.with_extension("json.tmp");
        std::fs::write(&temp, bytes).map_err(|e| e.to_string())?;
        std::fs::rename(&temp, &self.path).map_err(|e| e.to_string())?;
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
