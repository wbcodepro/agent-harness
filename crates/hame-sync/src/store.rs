//! The host's files in the hame home, under `sync/`:
//!
//! - `identity.json`: the host's private keys;
//! - `allowlist.json`: enrolled devices with their pinned keys and last
//!   counters, revoked keys, and the host's own counter;
//! - `relay.json`: the relay address, this host's device id and tokens.
//!
//! Every file is written with mode 0600 in a 0700 directory, atomically
//! (temporary file, then rename). Loading refuses a file or directory that
//! the group or others can read or write: keys on disk with mode 644 are
//! how other products leaked them.

use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::SyncError;
use crate::keys::{Identity, PublicIdentity, StoredIdentity, b64, unb64_32};

/// A device the host accepts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllowedDevice {
    pub device: String,
    pub label: String,
    pub ed: String,
    pub x: String,
    pub added: u64,
    /// The highest envelope counter accepted from this device.
    #[serde(default)]
    pub last_counter: u64,
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
    /// The last counter the host used in an envelope it sent.
    #[serde(default)]
    pub host_counter: u64,
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

    /// Adds a device. A known device with a different key is refused,
    /// never silently replaced; so is a revoked key.
    pub fn add(
        &mut self,
        device: &str,
        label: &str,
        keys: &PublicIdentity,
        counter: u64,
    ) -> Result<(), SyncError> {
        if self.is_revoked(keys) {
            return Err(SyncError::Pairing("this key was revoked".into()));
        }
        if let Some(known) = self.get(device) {
            if known.keys()? != *keys {
                return Err(SyncError::KeyChanged(device.into()));
            }
            return Ok(());
        }
        self.devices.push(AllowedDevice {
            device: device.into(),
            label: label.into(),
            ed: b64(&keys.ed),
            x: b64(&keys.x),
            added: crate::pairing::now_unix(),
            last_counter: counter,
        });
        Ok(())
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
        if let Some(stored) = self.load::<StoredIdentity>("identity.json")? {
            return stored.identity();
        }
        let identity = Identity::generate();
        self.save("identity.json", &StoredIdentity::from_identity(&identity))?;
        Ok(identity)
    }

    pub fn allowlist(&self) -> Result<Allowlist, SyncError> {
        Ok(self.load("allowlist.json")?.unwrap_or_default())
    }

    pub fn save_allowlist(&self, allowlist: &Allowlist) -> Result<(), SyncError> {
        self.save("allowlist.json", allowlist)
    }

    /// When and how large `allowlist.json` was last written, to notice a
    /// change made by another process (`hame connect revoke`).
    pub fn allowlist_stamp(&self) -> Option<(std::time::SystemTime, u64)> {
        let metadata = std::fs::metadata(self.dir.join("allowlist.json")).ok()?;
        Some((metadata.modified().ok()?, metadata.len()))
    }

    pub fn relay(&self) -> Result<Option<RelayConfig>, SyncError> {
        self.load("relay.json")
    }

    pub fn save_relay(&self, relay: &RelayConfig) -> Result<(), SyncError> {
        self.save("relay.json", relay)
    }
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
    use std::io::Write;
    let temp = path.with_extension(format!("tmp-{}", std::process::id()));
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
        list.add("phone", "Phone", &phone, 5).unwrap();
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
}
