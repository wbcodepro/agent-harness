//! Free-space guard for the database's file system: new envelopes are
//! refused while free space is below the configured floor.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a free-space reading is reused.
const RECHECK: Duration = Duration::from_secs(5);

pub struct DiskGuard {
    dir: PathBuf,
    floor: u64,
    last: Mutex<Option<(Instant, bool)>>,
}

impl DiskGuard {
    /// Watches the directory that holds `database_path`.
    pub fn new(database_path: &Path, floor: u64) -> Self {
        let dir = match database_path.parent() {
            Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
            _ => PathBuf::from("."),
        };
        Self {
            dir,
            floor,
            last: Mutex::new(None),
        }
    }

    /// Whether there is at least `floor` bytes of free space. Unknown free
    /// space (unsupported platform, failed query) counts as enough.
    pub fn has_room(&self) -> bool {
        if self.floor == 0 {
            return true;
        }
        let Ok(mut last) = self.last.lock() else {
            return true;
        };
        if let Some((at, ok)) = *last
            && at.elapsed() < RECHECK
        {
            return ok;
        }
        let ok = free_bytes(&self.dir).is_none_or(|free| free >= self.floor);
        if !ok {
            tracing::warn!(
                floor = self.floor,
                "free disk space below the floor; refusing envelopes"
            );
        }
        *last = Some((Instant::now(), ok));
        ok
    }
}

#[cfg(unix)]
fn free_bytes(dir: &Path) -> Option<u64> {
    let s = nix::sys::statvfs::statvfs(dir).ok()?;
    #[allow(clippy::unnecessary_cast)]
    Some((s.blocks_available() as u64).saturating_mul(s.fragment_size() as u64))
}

#[cfg(not(unix))]
fn free_bytes(_dir: &Path) -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_zero_always_has_room_and_huge_floor_never() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("relay.db");
        assert!(DiskGuard::new(&db, 0).has_room());
        #[cfg(unix)]
        assert!(!DiskGuard::new(&db, u64::MAX).has_room());
        #[cfg(unix)]
        assert!(DiskGuard::new(&db, 1).has_room());
    }
}
