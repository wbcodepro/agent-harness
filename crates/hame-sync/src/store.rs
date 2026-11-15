//! The host's files in the hame home, under `sync/`:
//!
//! - `identity.json`: the host's private keys;
//! - `allowlist.json`: enrolled devices with their pinned keys and replay
//!   windows, revoked keys, the host's counter reservation and the grant
//!   list version;
//! - `revocations.log`: revoked devices, one JSON line each, only ever
//!   appended to. Whatever `allowlist.json` says, a device listed here is
//!   not enrolled: a process holding a stale copy of the allowlist cannot
//!   bring a revoked device back;
//! - `relay.json`: the relay address, this host's device id and tokens;
//! - `lock`: an advisory lock taken for every read-modify-write, by the
//!   running connector and by `hame connect revoke` alike.
//!
//! Every file is written with mode 0600 in a 0700 directory, atomically
//! (temporary file, then rename). Loading refuses a file or directory that
//! the group or others can read or write: keys on disk with mode 644 are
//! how other products leaked them.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::SyncError;
use crate::envelope::ReplayWindow;
use crate::keys::{Identity, PublicIdentity, StoredIdentity, b64, unb64_32};

/// A device the host accepts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllowedDevice {
    pub device: String,
    pub label: String,
    pub ed: String,
    pub x: String,
    pub added: u64,
    /// The envelope counters accepted from this device.
    #[serde(default)]
    pub window: ReplayWindow,
}

impl AllowedDevice {
    pub fn keys(&self) -> Result<PublicIdentity, SyncError> {
        Ok(PublicIdentity {
            ed: unb64_32(&self.ed)?,
            x: unb64_32(&self.x)?,
        })
    }
}

/// A key that was revoked and is refused from then on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevokedKey {
    pub device: String,
    pub ed: String,
    pub revoked: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Allowlist {
    #[serde(default)]
    pub devices: Vec<AllowedDevice>,
    #[serde(default)]
    pub revoked: Vec<RevokedKey>,
    /// The host's counters up to this value may have been used: a host
    /// restarts above it (the connector reserves blocks ahead).
    #[serde(default)]
    pub host_counter: u64,
    /// The version of the last grant list published to the relay.
    #[serde(default)]
    pub grants_version: u64,
}

impl Allowlist {
    pub fn get(&self, device: &str) -> Option<&AllowedDevice> {
        self.devices.iter().find(|d| d.device == device)
    }

    pub fn get_mut(&mut self, device: &str) -> Option<&mut AllowedDevice> {
        self.devices.iter_mut().find(|d| d.device == device)
    }

    pub fn is_revoked(&self, keys: &PublicIdentity) -> bool {
        let ed = b64(&keys.ed);
        self.revoked.iter().any(|r| r.ed == ed)
    }

    /// Adds a device whose pairing envelope carried `counter`, and returns
    /// the highest counter accepted from it, which the device must continue
    /// above. A known device with a different key is refused, never
    /// silently replaced; so is a revoked key. A known device pairing again
    /// with its own key (it lost or rolled back its sequence) keeps its
    /// replay window: lowering it would let a relay replay every envelope
    /// the device sent before. The `paired` answer tells the device where
    /// to continue.
    pub fn add(
        &mut self,
        device: &str,
        label: &str,
        keys: &PublicIdentity,
        counter: u64,
    ) -> Result<u64, SyncError> {
        if self.is_revoked(keys) {
            return Err(SyncError::Pairing("this key was revoked".into()));
        }
        if let Some(known) = self.get_mut(device) {
            if known.keys()? != *keys {
                return Err(SyncError::KeyChanged(device.into()));
            }
            if counter > known.window.last {
                known.window.record(counter);
            }
            known.label = label.into();
            return Ok(known.window.last);
        }
        self.devices.push(AllowedDevice {
            device: device.into(),
            label: label.into(),
            ed: b64(&keys.ed),
            x: b64(&keys.x),
            added: crate::pairing::now_unix(),
            window: ReplayWindow::starting_at(counter),
        });
        Ok(counter)
    }

    /// Removes a device and remembers its key as revoked. False when the
    /// device was not enrolled.
    pub fn revoke(&mut self, device: &str) -> bool {
        let Some(at) = self.devices.iter().position(|d| d.device == device) else {
            return false;
        };
        let removed = self.devices.remove(at);
        self.revoked.push(RevokedKey {
            device: removed.device,
            ed: removed.ed,
            revoked: crate::pairing::now_unix(),
        });
        true
    }

    /// Applies revocations: every device whose id and key are listed is
    /// removed, and every listed key is remembered as revoked.
    fn apply(&mut self, revocations: &[RevokedKey]) {
        for revoked in revocations {
            self.devices
                .retain(|d| !(d.device == revoked.device && d.ed == revoked.ed));
            if !self.revoked.iter().any(|r| r.ed == revoked.ed) {
                self.revoked.push(revoked.clone());
            }
        }
    }
}

/// Where the host is registered.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayConfig {
    pub url: String,
    pub device_id: String,
    #[serde(default)]
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: String,
}

/// The `sync/` directory of a hame home.
#[derive(Clone, Debug)]
pub struct HostStore {
    dir: PathBuf,
}

/// Holds the store's advisory lock until dropped.
pub struct StoreLock {
    _file: std::fs::File,
}

/// The stamps of the files that make up the allowlist.
pub type Stamp = Vec<(std::time::SystemTime, u64)>;

impl HostStore {
    pub fn new(home: &Path) -> Self {
        Self {
            dir: home.join("sync"),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn ensure_dir(&self) -> Result<(), SyncError> {
        if !self.dir.exists() {
            create_private_dir(&self.dir)?;
        }
        check_private(&self.dir)
    }

    /// Takes the store's exclusive advisory lock (blocking).
    pub fn lock(&self) -> Result<StoreLock, SyncError> {
        self.ensure_dir()?;
        let path = self.dir.join("lock");
        let file = private_options(false)
            .open(&path)
            .map_err(|e| SyncError::io(&path, e))?;
        file.lock().map_err(|e| SyncError::io(&path, e))?;
        Ok(StoreLock { _file: file })
    }

    fn load<T: DeserializeOwned>(&self, name: &str) -> Result<Option<T>, SyncError> {
        let path = self.dir.join(name);
        if !path.exists() {
            return Ok(None);
        }
        check_private(&self.dir)?;
        check_private(&path)?;
        let text = std::fs::read_to_string(&path).map_err(|e| SyncError::io(&path, e))?;
        serde_json::from_str(&text)
            .map(Some)
            .map_err(|e| SyncError::Store(format!("{}: {e}", path.display())))
    }

    fn save<T: Serialize>(&self, name: &str, value: &T) -> Result<(), SyncError> {
        self.ensure_dir()?;
        let text = serde_json::to_vec_pretty(value).expect("serialises");
        write_private(&self.dir.join(name), &text)
    }

    /// The host's identity, created on first use.
    pub fn identity(&self) -> Result<Identity, SyncError> {
        let _lock = self.lock()?;
        if let Some(stored) = self.load::<StoredIdentity>("identity.json")? {
            return stored.identity();
        }
        let identity = Identity::generate();
        self.save("identity.json", &StoredIdentity::from_identity(&identity))?;
        Ok(identity)
    }

    /// Every revocation ever made, in order.
    pub fn revocations(&self) -> Result<Vec<RevokedKey>, SyncError> {
        let path = self.dir.join("revocations.log");
        if !path.exists() {
            return Ok(Vec::new());
        }
        check_private(&path)?;
        let text = std::fs::read_to_string(&path).map_err(|e| SyncError::io(&path, e))?;
        // A damaged line is an error, never skipped: skipping it would
        // silently un-revoke a device.
        let mut entries = Vec::new();
        for (n, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let entry = serde_json::from_str(line).map_err(|_| {
                SyncError::Store(format!(
                    "{} line {} is damaged; repair it (it names a revoked device) or remove it \
                     and revoke that device again",
                    path.display(),
                    n + 1
                ))
            })?;
            entries.push(entry);
        }
        Ok(entries)
    }

    fn append_revocations(&self, entries: &[RevokedKey]) -> Result<(), SyncError> {
        if entries.is_empty() {
            return Ok(());
        }
        let path = self.dir.join("revocations.log");
        // A torn last line (a crash during an append) must not swallow the
        // next entry: start on a new line.
        let torn = std::fs::read(&path)
            .map(|bytes| bytes.last().is_some_and(|b| *b != b'\n'))
            .unwrap_or(false);
        let mut file = private_options(true)
            .open(&path)
            .map_err(|e| SyncError::io(&path, e))?;
        let mut text = String::from(if torn { "\n" } else { "" });
        for entry in entries {
            text.push_str(&serde_json::to_string(entry).expect("serialises"));
            text.push('\n');
        }
        file.write_all(text.as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(|e| SyncError::io(&path, e))
    }

    fn read_allowlist(&self) -> Result<Allowlist, SyncError> {
        let mut allowlist: Allowlist = self.load("allowlist.json")?.unwrap_or_default();
        allowlist.apply(&self.revocations()?);
        Ok(allowlist)
    }

    /// The allowlist with every logged revocation applied.
    pub fn allowlist(&self) -> Result<Allowlist, SyncError> {
        let _lock = self.lock()?;
        self.read_allowlist()
    }

    /// Saves an allowlist. Revocations it holds that are not logged yet are
    /// logged first, and logged revocations are applied to it, so a caller
    /// with an older copy can neither lose a revocation nor undo one.
    pub fn save_allowlist(&self, allowlist: &Allowlist) -> Result<(), SyncError> {
        let _lock = self.lock()?;
        self.write_allowlist(allowlist.clone())
    }

    fn write_allowlist(&self, mut allowlist: Allowlist) -> Result<(), SyncError> {
        let logged = self.revocations()?;
        let new: Vec<RevokedKey> = allowlist
            .revoked
            .iter()
            .filter(|r| !logged.iter().any(|l| l.ed == r.ed && l.device == r.device))
            .cloned()
            .collect();
        self.append_revocations(&new)?;
        allowlist.apply(&logged);
        self.save("allowlist.json", &allowlist)
    }

    /// Read-modify-write under the lock: `change` sees the current file
    /// (revocations applied) and what it leaves is saved. Returns the saved
    /// allowlist and `change`'s result.
    pub fn update_allowlist<T>(
        &self,
        change: impl FnOnce(&mut Allowlist) -> Result<T, SyncError>,
    ) -> Result<(Allowlist, T), SyncError> {
        let _lock = self.lock()?;
        let mut allowlist = self.read_allowlist()?;
        let result = change(&mut allowlist)?;
        self.write_allowlist(allowlist)?;
        Ok((self.read_allowlist()?, result))
    }

    /// When and how large `allowlist.json` and `revocations.log` were last
    /// written, to notice a change made by another process.
    pub fn allowlist_stamp(&self) -> Stamp {
        ["allowlist.json", "revocations.log"]
            .into_iter()
            .filter_map(|name| {
                let metadata = std::fs::metadata(self.dir.join(name)).ok()?;
                Some((metadata.modified().ok()?, metadata.len()))
            })
            .collect()
    }

    /// Makes this process the only `hame connect` for this home until the
    /// returned lock is dropped (a second one would fight over the relay
    /// connection and the counters).
    pub fn lock_instance(&self) -> Result<StoreLock, SyncError> {
        self.ensure_dir()?;
        let path = self.dir.join("connect.lock");
        let file = private_options(false)
            .open(&path)
            .map_err(|e| SyncError::io(&path, e))?;
        match file.try_lock() {
            Ok(()) => Ok(StoreLock { _file: file }),
            Err(std::fs::TryLockError::WouldBlock) => Err(SyncError::Store(
                "another hame connect is running for this hame home".into(),
            )),
            Err(std::fs::TryLockError::Error(e)) => Err(SyncError::io(&path, e)),
        }
    }

    pub fn relay(&self) -> Result<Option<RelayConfig>, SyncError> {
        self.load("relay.json")
    }

    pub fn save_relay(&self, relay: &RelayConfig) -> Result<(), SyncError> {
        let _lock = self.lock()?;
        self.save("relay.json", relay)
    }
}

fn private_options(append: bool) -> std::fs::OpenOptions {
    let mut options = std::fs::OpenOptions::new();
    options.create(true);
    if append {
        options.append(true);
    } else {
        options.read(true).write(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

#[cfg(unix)]
fn create_private_dir(dir: &Path) -> Result<(), SyncError> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|e| SyncError::io(dir, e))
}

#[cfg(not(unix))]
fn create_private_dir(dir: &Path) -> Result<(), SyncError> {
    // On Windows a directory under the user's profile inherits an ACL that
    // only the user (and administrators) can read.
    std::fs::create_dir_all(dir).map_err(|e| SyncError::io(dir, e))
}

/// Refuses a file or directory the group or others can access.
#[cfg(unix)]
pub fn check_private(path: &Path) -> Result<(), SyncError> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path)
        .map_err(|e| SyncError::io(path, e))?
        .permissions()
        .mode();
    if mode & 0o077 != 0 {
        return Err(SyncError::InsecurePermissions {
            path: path.to_path_buf(),
            mode: mode & 0o777,
        });
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn check_private(_path: &Path) -> Result<(), SyncError> {
    Ok(())
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), SyncError> {
    let temp = path.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        crate::keys::hex(&crate::keys::random32()[..6])
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options.open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result.map_err(|e| SyncError::io(path, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_identity_persists_privately() {
        let home = tempfile::tempdir().unwrap();
        let store = HostStore::new(home.path());
        let first = store.identity().unwrap();
        assert_eq!(store.identity().unwrap().public(), first.public());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = store.dir().join("identity.json");
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(matches!(
                store.identity(),
                Err(SyncError::InsecurePermissions { .. })
            ));
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            std::fs::set_permissions(store.dir(), std::fs::Permissions::from_mode(0o755)).unwrap();
            assert!(store.identity().is_err());
        }
    }

    #[test]
    fn the_allowlist_refuses_changed_and_revoked_keys() {
        let mut list = Allowlist::default();
        let phone = Identity::generate().public();
        list.add("phone", "Phone", &phone, 5).unwrap();
        list.get_mut("phone").unwrap().window.record(40);
        // Pairing again with a lower counter keeps the window.
        assert_eq!(list.add("phone", "Phone", &phone, 3).unwrap(), 40);
        let window = list.get("phone").unwrap().window;
        assert!(window.check(40).is_err() && window.check(5).is_err());
        assert_eq!(list.devices.len(), 1);
        let swapped = Identity::generate().public();
        assert!(matches!(
            list.add("phone", "Phone", &swapped, 5),
            Err(SyncError::KeyChanged(_))
        ));
        assert!(list.revoke("phone"));
        assert!(!list.revoke("phone"));
        assert!(list.get("phone").is_none());
        assert!(list.add("phone2", "Phone", &phone, 5).is_err());
    }

    #[test]
    fn a_torn_revocation_line_is_never_skipped_and_never_swallows_the_next() {
        let home = tempfile::tempdir().unwrap();
        let store = HostStore::new(home.path());
        let phone = Identity::generate().public();
        let tablet = Identity::generate().public();
        store
            .update_allowlist(|list| {
                list.add("phone", "Phone", &phone, 1)?;
                list.add("tablet", "Tablet", &tablet, 1)
            })
            .unwrap();
        store
            .update_allowlist(|list| Ok(list.revoke("phone")))
            .unwrap();
        // A crash in the middle of the next append.
        let log = store.dir().join("revocations.log");
        let mut text = std::fs::read_to_string(&log).unwrap();
        text.push_str("{\"device\":\"tab");
        std::fs::write(&log, &text).unwrap();
        assert!(matches!(store.allowlist(), Err(SyncError::Store(_))));
        // Repaired by hand (the torn line removed): the next append starts
        // on its own line even after a torn one.
        std::fs::write(&log, text.trim_end_matches("{\"device\":\"tab")).unwrap();
        let mut unterminated = std::fs::read_to_string(&log).unwrap();
        unterminated.pop();
        std::fs::write(&log, unterminated).unwrap();
        store
            .update_allowlist(|list| Ok(list.revoke("tablet")))
            .unwrap();
        let revoked = store.revocations().unwrap();
        assert_eq!(revoked.len(), 2);
        assert!(store.allowlist().unwrap().devices.is_empty());
    }

    #[test]
    fn one_connector_per_home() {
        let home = tempfile::tempdir().unwrap();
        let store = HostStore::new(home.path());
        let first = store.lock_instance().unwrap();
        assert!(store.lock_instance().is_err());
        drop(first);
        assert!(store.lock_instance().is_ok());
    }

    #[test]
    fn a_stale_copy_cannot_undo_a_revocation() {
        let home = tempfile::tempdir().unwrap();
        let store = HostStore::new(home.path());
        let phone = Identity::generate().public();
        store
            .update_allowlist(|list| list.add("phone", "Phone", &phone, 1))
            .unwrap();
        // The running connector's copy, read before the revocation.
        let mut stale = store.allowlist().unwrap();
        // `hame connect revoke` in another process.
        store
            .update_allowlist(|list| Ok(list.revoke("phone")))
            .unwrap();
        // The connector saves its stale copy (a counter update).
        stale.get_mut("phone").unwrap().window.record(2);
        store.save_allowlist(&stale).unwrap();
        let now = store.allowlist().unwrap();
        assert!(now.get("phone").is_none());
        assert!(now.is_revoked(&phone));
        assert_eq!(store.revocations().unwrap().len(), 1);
    }
}
