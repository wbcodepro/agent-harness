use std::process::Stdio;

/// Kills a whole process group when dropped, unless disarmed.
///
/// Arm it with the id of a process that leads its own group; dropping it on
/// a timeout or a cancelled call then takes everything that process started
/// down with it.
pub struct GroupGuard(Option<u32>);

impl GroupGuard {
    pub fn new(leader_pid: Option<u32>) -> Self {
        Self(leader_pid)
    }

    /// A guard that does nothing when dropped.
    pub fn disarmed() -> Self {
        Self(None)
    }

    pub fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for GroupGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.0 {
            let _ = std::process::Command::new("kill")
                .args(["-KILL", "--", &format!("-{pid}")])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        #[cfg(not(unix))]
        let _ = (self.0, Stdio::null());
    }
}
