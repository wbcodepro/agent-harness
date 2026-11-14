//! Which POSIX shell to run command lines with.
//!
//! Everywhere but Windows that is whatever `bash` and `sh` the system path
//! gives. On Windows the `bash` found first on the path is usually the
//! launcher for the Windows Subsystem for Linux, which runs commands in a
//! different machine with a different filesystem (and does not start at all
//! from a Low-integrity process). Git for Windows ships the bash that works
//! on Windows paths, so that one is looked for: in the usual install
//! locations, then next to the `git` on the path, wherever Git was installed.

use std::path::PathBuf;

/// The bash to run command lines with.
pub fn bash_program() -> PathBuf {
    #[cfg(windows)]
    if let Some(bash) = windows_bash() {
        return bash.to_path_buf();
    }
    "bash".into()
}

/// The `sh` to run command lines with.
pub fn sh_program() -> PathBuf {
    #[cfg(windows)]
    if let Some(bash) = windows_bash() {
        let sh = bash.with_file_name("sh.exe");
        return if sh.is_file() { sh } else { bash.to_path_buf() };
    }
    "sh".into()
}

/// The Python that runs programs here: `python3`, or `python` where that is
/// its name. Windows has a `python3` that starts and runs nothing (it offers
/// to open the Store), so being able to start one proves nothing: each is
/// asked to run an empty program. Looked up once.
pub fn python_program() -> Option<&'static str> {
    static FOUND: std::sync::OnceLock<Option<&'static str>> = std::sync::OnceLock::new();
    *FOUND.get_or_init(|| {
        ["python3", "python"].into_iter().find(|program| {
            std::process::Command::new(program)
                .args(["-c", ""])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        })
    })
}
/// The bash of Git for Windows (or another bash that works on Windows
/// paths), if one is installed: the one `ZCODE_BASH` names, else the one
/// found. Looked up once.
#[cfg(windows)]
pub fn windows_bash() -> Option<&'static std::path::Path> {
    static FOUND: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    FOUND
        .get_or_init(|| {
            // The user's choice comes first. It is the way to give zcode a
            // Git of its own: bash keeps shared state per installation, and
            // a confined bash cannot join the state of an unconfined one.
            if let Some(chosen) = std::env::var_os("ZCODE_BASH").map(PathBuf::from)
                && chosen.is_file()
            {
                return Some(chosen);
            }
            let roots = [
                "ProgramFiles",
                "ProgramW6432",
                "ProgramFiles(x86)",
                "LOCALAPPDATA",
            ]
            .iter()
            .filter_map(std::env::var_os)
            .map(PathBuf::from)
            .collect::<Vec<_>>();
            let path = std::env::var_os("PATH").unwrap_or_default();
            let system = std::env::var_os("SystemRoot").map(PathBuf::from);
            find_bash(&roots, &path, system.as_deref())
        })
        .as_deref()
}

/// `roots` are the directories programs are installed under, `path` the
/// value of `PATH`, `system` the Windows directory.
#[cfg(any(windows, test))]
fn find_bash(
    roots: &[PathBuf],
    path: &std::ffi::OsStr,
    system: Option<&std::path::Path>,
) -> Option<PathBuf> {
    use std::path::Path;

    // The usual install locations.
    for root in roots {
        for layout in ["Git/bin/bash.exe", "Programs/Git/bin/bash.exe"] {
            let candidate = root.join(layout);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    let on_path: Vec<PathBuf> = std::env::split_paths(path)
        .filter(|dir| !dir.as_os_str().is_empty())
        .collect();
    // Next to the git on the path: Git puts `<Git>\cmd` there by default,
    // `<Git>\bin` or `<Git>\mingw64\bin` with other installer choices.
    for dir in &on_path {
        if !dir.join("git.exe").is_file() {
            continue;
        }
        let above = |levels: usize| dir.ancestors().nth(levels).map(Path::to_path_buf);
        for git_root in [above(1), above(2)].into_iter().flatten() {
            let candidate = git_root.join("bin").join("bash.exe");
            if candidate.is_file() && git_root.join("usr").is_dir() {
                return Some(candidate);
            }
        }
    }
    // Any other bash on the path (MSYS2, Cygwin), except the WSL launchers:
    // the one in the Windows directory and the app alias.
    for dir in &on_path {
        let lowered = dir.to_string_lossy().to_ascii_lowercase();
        let is_system = system.is_some_and(|system| {
            lowered.starts_with(&system.to_string_lossy().to_ascii_lowercase())
        });
        if is_system || lowered.contains("windowsapps") {
            continue;
        }
        let candidate = dir.join("bash.exe");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch(path: &std::path::Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "").unwrap();
    }

    fn path_of(dirs: &[PathBuf]) -> std::ffi::OsString {
        std::env::join_paths(dirs).unwrap()
    }

    #[test]
    fn git_bash_is_found_wherever_git_is_installed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let system = root.join("Windows");
        touch(&system.join("System32/bash.exe"));
        let wsl = system.join("System32");

        // Nothing but the WSL launcher: no usable bash.
        assert_eq!(
            find_bash(&[], &path_of(std::slice::from_ref(&wsl)), Some(&system)),
            None
        );

        // Git on another drive, known only through `<Git>\cmd` on the path.
        let git = root.join("D/Program Files/Git");
        touch(&git.join("cmd/git.exe"));
        touch(&git.join("bin/bash.exe"));
        touch(&git.join("usr/bin/bash.exe"));
        let path = path_of(&[wsl.clone(), git.join("cmd")]);
        assert_eq!(
            find_bash(&[], &path, Some(&system)),
            Some(git.join("bin").join("bash.exe"))
        );
        // The same with `<Git>\mingw64\bin` or `<Git>\bin` on the path.
        touch(&git.join("mingw64/bin/git.exe"));
        touch(&git.join("bin/git.exe"));
        for on_path in ["mingw64/bin", "bin"] {
            let path = path_of(&[wsl.clone(), git.join(on_path)]);
            assert_eq!(
                find_bash(&[], &path, Some(&system)),
                Some(git.join("bin").join("bash.exe")),
                "{on_path}"
            );
        }

        // The usual location wins over the path.
        let programs = root.join("Program Files");
        touch(&programs.join("Git/bin/bash.exe"));
        assert_eq!(
            find_bash(std::slice::from_ref(&programs), &path, Some(&system)),
            Some(programs.join("Git/bin/bash.exe"))
        );

        // Without git, another bash on the path will do.
        let msys = root.join("msys64/usr/bin");
        touch(&msys.join("bash.exe"));
        assert_eq!(
            find_bash(&[], &path_of(&[wsl, msys.clone()]), Some(&system)),
            Some(msys.join("bash.exe"))
        );
    }
}
