//! The desktop lease: a cross-process file lock that only one hame process
//! holds while it emits physical input (typing, key presses, clicks).
//!
//! Synthesized input goes to whatever has focus, so two processes typing
//! at once would interleave their characters into the same window. The
//! lock is advisory and released by the operating system when the holder
//! exits, so a crashed process cannot leave the desktop leased.

use std::fs::{self, File, TryLockError};
use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// A held lease. Dropping it releases the lock.
#[derive(Debug)]
pub struct DesktopLease {
    file: File,
    path: PathBuf,
}

impl DesktopLease {
    /// Takes the lease, waiting up to `wait` for another holder to release
    /// it. On failure says who holds it.
    pub fn acquire(path: &Path, wait: Duration) -> Result<Self, String> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| format!("Cannot create {}: {e}", dir.display()))?;
        }
        let mut file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)
            .map_err(|e| format!("Cannot open the desktop lease {}: {e}", path.display()))?;
        let deadline = Instant::now() + wait;
        loop {
            match file.try_lock() {
                Ok(()) => break,
                Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(TryLockError::WouldBlock) => {
                    let mut holder = String::new();
                    let _ = file.read_to_string(&mut holder);
                    let holder = holder.trim();
                    return Err(format!(
                        "lease_busy: another hame process is emitting desktop input ({}). \
                         Only one process may type or click at a time; try again when it is done.",
                        if holder.is_empty() {
                            "holder unknown"
                        } else {
                            holder
                        }
                    ));
                }
                Err(TryLockError::Error(e)) => {
                    return Err(format!("Cannot lock {}: {e}", path.display()));
                }
            }
        }
        // Who holds it, for the message another process shows.
        let _ = file.set_len(0);
        let _ = file.seek(SeekFrom::Start(0));
        let _ = writeln!(file, "pid {}", std::process::id());
        let _ = file.flush();
        Ok(Self {
            file,
            path: path.to_path_buf(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for DesktopLease {
    fn drop(&mut self) {
        let _ = self.file.set_len(0);
        let _ = self.file.unlock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_one_holder_at_a_time_and_release_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("locks").join("desktop.lease");
        let first = DesktopLease::acquire(&path, Duration::ZERO).unwrap();
        assert_eq!(first.path(), path);
        // A second open file description conflicts even in this process,
        // exactly as another process would.
        let err = DesktopLease::acquire(&path, Duration::from_millis(120)).unwrap_err();
        assert!(err.starts_with("lease_busy"), "{err}");
        assert!(
            err.contains(&format!("pid {}", std::process::id())),
            "{err}"
        );
        drop(first);
        let again = DesktopLease::acquire(&path, Duration::ZERO).unwrap();
        drop(again);
    }
}
