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

/// Windows: a command and everything it starts, in a job object of its
/// own. Windows has no process groups, and a parent's id does not find
/// every descendant: Git's bash starts programs through a process that
/// exits at once, so `taskkill /T` misses them. Membership of a job passes
/// to every process created inside it.
///
/// Dropping it ends them all, unless [`CommandTree::release`] let them go;
/// so does this process exiting. Inside a sandbox worker the job nests in
/// the worker's own.
#[cfg(windows)]
pub struct CommandTree {
    /// The job's handle, as a number so the value can cross threads.
    job: usize,
}

#[cfg(windows)]
impl CommandTree {
    /// Puts the process `pid` in a new tree. `None` when that cannot be
    /// done; the caller then has no more than the process itself to end.
    #[allow(unsafe_code)]
    pub fn new(pid: Option<u32>) -> Option<Self> {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::JobObjects::{AssignProcessToJobObject, CreateJobObjectW};
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
        };

        let pid = pid?;
        // SAFETY: the job is created without a name or security attributes
        // (null is documented for both); the process handle is opened, used
        // and closed here; the job handle is closed once, by `Drop`.
        unsafe {
            let process = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
            if process.is_null() {
                return None;
            }
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                CloseHandle(process);
                return None;
            }
            let tree = Self { job: job as usize };
            let assigned = tree.end_on_close(true) && AssignProcessToJobObject(job, process) != 0;
            CloseHandle(process);
            assigned.then_some(tree)
        }
    }

    /// Lets the processes live on, as for a command that ended by itself
    /// and left something running on purpose.
    pub fn release(self) {
        self.end_on_close(false);
    }

    /// Ends every process in the tree now.
    #[allow(unsafe_code)]
    pub fn end(self) {
        // SAFETY: the handle is open for as long as `self` lives.
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(
                self.job as windows_sys::Win32::Foundation::HANDLE,
                1,
            );
        }
    }

    #[allow(unsafe_code)]
    fn end_on_close(&self, end: bool) -> bool {
        use windows_sys::Win32::System::JobObjects::{
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JobObjectExtendedLimitInformation, SetInformationJobObject,
        };
        // SAFETY: all-zero is the documented "no limits" value of this plain
        // structure; the pointer and size describe that local; the handle is
        // open for as long as `self` lives.
        unsafe {
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            if end {
                limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            }
            SetInformationJobObject(
                self.job as windows_sys::Win32::Foundation::HANDLE,
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) != 0
        }
    }
}

#[cfg(windows)]
impl Drop for CommandTree {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        // SAFETY: the handle was opened by `new` and is closed only here.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(
                self.job as windows_sys::Win32::Foundation::HANDLE,
            );
        }
    }
}
