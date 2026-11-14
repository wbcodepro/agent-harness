//! Windows backend: write confinement through integrity levels.
//!
//! The helper process lowers its own integrity level to Low before it
//! starts the tool. Windows refuses a Low process any write or delete on
//! an object labelled Medium or higher, which is every ordinary file and
//! directory. The writable roots of the policy are given a Low label ahead
//! of time, so they, and only they, stay writable.
//!
//! This confines writes. It does not hide files from reading and does not
//! limit the network, so a policy asking for either cannot be enforced
//! here and is refused.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::policy::{Mode, Network, SandboxError, SandboxPolicy};

/// The directory confined tools use for temporary files: inside
/// `AppData\LocalLow`, the profile's standard location for Low-integrity
/// data.
pub fn low_temp_dir() -> Option<PathBuf> {
    let profile = std::env::var_os("USERPROFILE")?;
    Some(
        Path::new(&profile)
            .join("AppData")
            .join("LocalLow")
            .join("zcode-tmp"),
    )
}

/// Whether this backend can enforce everything `policy` asks for.
pub(crate) fn check(policy: &SandboxPolicy) -> Result<(), SandboxError> {
    // A proxy can be offered to tools (the variables are set) but nothing
    // makes them use it; cutting the network off cannot be done at all.
    if policy.network == Network::Blocked || !policy.unreadable.is_empty() || policy.hide_env_files
    {
        return Err(SandboxError::Unavailable(
            "on Windows only writes can be confined; hiding files from tools and limiting \
             their network access are not available"
                .into(),
        ));
    }
    Ok(())
}

fn icacls(args: &[&std::ffi::OsStr]) -> Result<String, SandboxError> {
    let output = Command::new("icacls")
        .args(args)
        .output()
        .map_err(|e| SandboxError::Unavailable(format!("cannot run icacls: {e}")))?;
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    if output.status.success() {
        Ok(text)
    } else {
        Err(SandboxError::Unavailable(format!(
            "icacls failed: {} {}",
            text.trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

/// Makes the policy's writable roots writable for Low-integrity processes.
/// Run by the harness, at its normal integrity, before any tool starts.
///
/// The label is inherited by everything created under a root afterwards.
/// It stays on the directory after the harness exits: other Low-integrity
/// programs of the same user can then write there too.
pub fn prepare(policy: &SandboxPolicy) -> Result<(), SandboxError> {
    if policy.mode != Mode::WorkspaceWrite {
        return Ok(());
    }
    for root in &policy.writable {
        std::fs::create_dir_all(root).map_err(|e| {
            SandboxError::Unavailable(format!("cannot create {}: {e}", root.display()))
        })?;
        let labelled = icacls(&[root.as_os_str()])?.contains("Low Mandatory Level");
        if !labelled {
            // /T also relabels what is already inside; inheritance covers
            // what is created later.
            icacls(&[
                root.as_os_str(),
                "/setintegritylevel".as_ref(),
                "(OI)(CI)L".as_ref(),
                "/T".as_ref(),
                "/C".as_ref(),
                "/Q".as_ref(),
            ])?;
        }
    }
    Ok(())
}

/// Lowers the calling process to Low integrity. Everything it starts
/// afterwards inherits that level. There is no way back up.
pub fn apply_to_current_process(policy: &SandboxPolicy) -> Result<(), SandboxError> {
    if !policy.is_confined() {
        return Ok(());
    }
    check(policy)?;
    lower_integrity()
        .map_err(|e| SandboxError::Unavailable(format!("cannot lower the integrity level: {e}")))
}

#[allow(unsafe_code)]
fn lower_integrity() -> std::io::Result<()> {
    use std::ffi::c_void;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
    use windows_sys::Win32::Security::Authorization::ConvertStringSidToSidW;
    use windows_sys::Win32::Security::{
        GetLengthSid, PSID, SID_AND_ATTRIBUTES, SetTokenInformation, TOKEN_ADJUST_DEFAULT,
        TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TokenIntegrityLevel,
    };
    use windows_sys::Win32::System::SystemServices::SE_GROUP_INTEGRITY;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    // The well-known SID of the Low mandatory level.
    let low: Vec<u16> = "S-1-16-4096\0".encode_utf16().collect();

    // SAFETY: every pointer handed to the system below points at a live
    // local of the type the call documents; the token handle and the SID
    // allocated by the system are released exactly once, after last use.
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ADJUST_DEFAULT | TOKEN_QUERY,
            &mut token,
        ) == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        let mut sid: PSID = std::ptr::null_mut();
        if ConvertStringSidToSidW(low.as_ptr(), &mut sid) == 0 {
            let error = std::io::Error::last_os_error();
            CloseHandle(token);
            return Err(error);
        }
        let label = TOKEN_MANDATORY_LABEL {
            Label: SID_AND_ATTRIBUTES {
                Sid: sid,
                Attributes: SE_GROUP_INTEGRITY as u32,
            },
        };
        let size = std::mem::size_of::<TOKEN_MANDATORY_LABEL>() as u32 + GetLengthSid(sid);
        let set = SetTokenInformation(
            token,
            TokenIntegrityLevel,
            (&raw const label).cast::<c_void>(),
            size,
        );
        let error = std::io::Error::last_os_error();
        LocalFree(sid.cast());
        CloseHandle(token);
        if set == 0 {
            return Err(error);
        }
    }
    Ok(())
}

/// A process and everything it starts, so that all of it can be ended
/// together: Windows has no process groups to signal. Dropping this ends
/// them, unless [`ProcessTree::release`] let them go first.
pub(crate) struct ProcessTree {
    /// The job object's handle, kept as a number so the value can cross
    /// threads.
    job: usize,
}

impl ProcessTree {
    /// Puts `process` (a process handle), and whatever it starts from now
    /// on, in a new tree.
    #[allow(unsafe_code)]
    pub(crate) fn around(process: std::os::windows::io::RawHandle) -> std::io::Result<Self> {
        use windows_sys::Win32::System::JobObjects::{AssignProcessToJobObject, CreateJobObjectW};

        // SAFETY: the job is created without a name or security attributes
        // (null is documented for both); `process` is a live process handle
        // owned by the caller for the duration of the call.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            // From here the handle is closed on every path, by `Drop`.
            let tree = Self { job: job as usize };
            tree.end_on_close(true)?;
            if AssignProcessToJobObject(job, process.cast()) == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(tree)
        }
    }

    /// Lets the processes live on: what a command deliberately left in the
    /// background outlasts the call that started it.
    pub(crate) fn release(self) {
        let _ = self.end_on_close(false);
    }

    #[allow(unsafe_code)]
    fn end_on_close(&self, end: bool) -> std::io::Result<()> {
        use windows_sys::Win32::System::JobObjects::{
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JobObjectExtendedLimitInformation, SetInformationJobObject,
        };

        // SAFETY: the structure is plain data for which all-zero is the
        // documented "no limits" value; the pointer and size passed describe
        // that local; the job handle is open for as long as `self` lives.
        unsafe {
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            if end {
                limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            }
            let set = SetInformationJobObject(
                self.job as windows_sys::Win32::Foundation::HANDLE,
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if set == 0 {
                return Err(std::io::Error::last_os_error());
            }
        }
        Ok(())
    }
}

impl Drop for ProcessTree {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        // SAFETY: the handle was opened by `around` and is closed only here.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(
                self.job as windows_sys::Win32::Foundation::HANDLE,
            );
        }
    }
}

/// Stops this process's own input and output handles from being handed to
/// the programs it starts. A program left running in the background would
/// otherwise hold the pipe the harness reads the worker's answer from, and
/// the harness would wait for it.
#[allow(unsafe_code)]
pub fn keep_stdio_private() {
    use windows_sys::Win32::Foundation::{
        HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation,
    };
    use windows_sys::Win32::System::Console::{
        GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };

    for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        // SAFETY: both calls take a handle value and no pointers; a handle
        // that is missing or invalid is skipped.
        unsafe {
            let handle = GetStdHandle(which);
            if !handle.is_null() && handle != INVALID_HANDLE_VALUE {
                SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0);
            }
        }
    }
}
