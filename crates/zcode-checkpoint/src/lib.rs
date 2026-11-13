//! Workspace checkpoints.
//!
//! Each checkpoint is a commit in a private git repository that lives
//! outside the workspace and uses the workspace as its work tree. The
//! project's own repository, index and history are never touched, and the
//! project's `.gitignore` decides what is left out.

use std::path::{Component, Path, PathBuf};
use std::process::Stdio;

use tokio::process::Command;
use zcode_agent::Checkpointer;
use zcode_kernel::BoxFuture;

const LATEST: &str = "refs/zcode/latest";

pub struct GitCheckpoints {
    git_dir: PathBuf,
    work_tree: PathBuf,
}

/// What [`GitCheckpoints::restore`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Restored {
    /// Checkpoint of the workspace as it was just before restoring; restore
    /// to it to undo the rewind.
    pub before: String,
    /// Files deleted because they did not exist at the restored checkpoint.
    pub removed: Vec<String>,
}

impl GitCheckpoints {
    /// `git_dir` is created on first use. `work_tree` is the workspace.
    pub fn new(git_dir: impl Into<PathBuf>, work_tree: impl Into<PathBuf>) -> Self {
        Self {
            git_dir: zcode_kernel::simplify_path(&git_dir.into()),
            work_tree: zcode_kernel::simplify_path(&work_tree.into()),
        }
    }

    async fn git(&self, args: &[&str]) -> Result<String, String> {
        let output = Command::new("git")
            .arg("--git-dir")
            .arg(&self.git_dir)
            .arg("--work-tree")
            .arg(&self.work_tree)
            .args([
                "-c",
                "user.name=zcode",
                "-c",
                "user.email=zcode@localhost",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "core.autocrlf=false",
                "-c",
                "core.quotepath=false",
            ])
            .args(args)
            .current_dir(&self.work_tree)
            // The user's git configuration must not change what a
            // checkpoint contains.
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .stdin(Stdio::null())
            .output()
            .await
            .map_err(|e| format!("cannot run git: {e}"))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout)
                .trim_end()
                .to_string())
        } else {
            Err(format!(
                "git {} failed: {}",
                args.first().copied().unwrap_or(""),
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }

    async fn ensure_repository(&self) -> Result<(), String> {
        if self.git_dir.join("HEAD").exists() {
            return Ok(());
        }
        std::fs::create_dir_all(&self.git_dir)
            .map_err(|e| format!("cannot create {}: {e}", self.git_dir.display()))?;
        self.git(&["init", "-q"]).await.map(|_| ())
    }

    /// Saves the workspace and returns the checkpoint's commit id. When
    /// nothing changed since the last checkpoint, returns that one.
    pub async fn snapshot(&self) -> Result<String, String> {
        self.ensure_repository().await?;
        let links = linked_dirs(&self.work_tree);
        self.leave_out(&links).await?;
        self.git(&["add", "-A", "."]).await?;
        let tree = self.git(&["write-tree"]).await?;
        let parent = self
            .git(&["rev-parse", "-q", "--verify", LATEST])
            .await
            .ok();
        if let Some(parent) = &parent
            && self
                .git(&["rev-parse", &format!("{parent}^{{tree}}")])
                .await
                .as_deref()
                == Ok(&tree)
        {
            return Ok(parent.clone());
        }
        let mut args = vec!["commit-tree", &tree, "-m", "checkpoint"];
        if let Some(parent) = &parent {
            args.extend(["-p", parent]);
        }
        let commit = self.git(&args).await?;
        self.git(&["update-ref", LATEST, &commit]).await?;
        Ok(commit)
    }

    /// Puts the workspace back to how it was at `commit`: changed and
    /// deleted files are restored, files created since are removed. Files
    /// the project ignores are left alone. The current state is checkpointed
    /// first, so a restore can itself be undone.
    pub async fn restore(&self, commit: &str) -> Result<Restored, String> {
        self.ensure_repository().await?;
        self.git(&["cat-file", "-e", &format!("{commit}^{{commit}}")])
            .await
            .map_err(|_| format!("checkpoint {commit} does not exist"))?;
        let before = self.snapshot().await?;

        // Where the checkpoint has a real directory and the workspace now
        // has a link, restoring would write through the link. Take the link
        // away (not what it leads to) so the directory can come back.
        let links = linked_dirs(&self.work_tree);
        if !links.is_empty() {
            let wanted = self
                .git(&["ls-tree", "-r", "-z", "--name-only", commit])
                .await?;
            let wanted: Vec<&str> = wanted.split('\0').collect();
            for link in links.iter().filter(|link| any_inside(&wanted, link)) {
                std::fs::remove_dir(self.work_tree.join(link))
                    .map_err(|e| format!("cannot remove the link {link}: {e}"))?;
            }
        }

        let added = self
            .git(&[
                "diff",
                "--name-only",
                "--diff-filter=A",
                "--no-renames",
                commit,
                &before,
            ])
            .await?;
        let mut removed = Vec::new();
        for name in added.lines().filter(|line| !line.is_empty()) {
            let relative = Path::new(name);
            // Names come from our own repository, but never follow one out
            // of the workspace.
            if relative
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
            {
                continue;
            }
            if links.iter().any(|link| any_inside(&[name], link)) {
                continue;
            }
            if std::fs::remove_file(self.work_tree.join(relative)).is_ok() {
                removed.push(name.to_string());
            }
        }

        self.git(&["read-tree", commit]).await?;
        self.git(&["checkout-index", "-a", "-f"]).await?;
        self.snapshot().await?;
        Ok(Restored { before, removed })
    }
}

impl GitCheckpoints {
    /// Keeps `links`, and whatever is reached through them, out of the next
    /// `git add`, and drops what earlier checkpoints recorded under them.
    async fn leave_out(&self, links: &[String]) -> Result<(), String> {
        let exclude = self.git_dir.join("info").join("exclude");
        let patterns: String = links
            .iter()
            .map(|link| format!("/{}\n", escape_pattern(link)))
            .collect();
        if std::fs::read_to_string(&exclude).unwrap_or_default() != patterns {
            std::fs::create_dir_all(self.git_dir.join("info"))
                .and_then(|()| std::fs::write(&exclude, patterns))
                .map_err(|e| format!("cannot write {}: {e}", exclude.display()))?;
        }
        if links.is_empty() {
            return Ok(());
        }
        let tracked = self.git(&["ls-files", "-z"]).await?;
        let tracked: Vec<&str> = tracked.split('\0').collect();
        let stale: Vec<String> = links
            .iter()
            .filter(|link| any_inside(&tracked, link))
            .map(|link| format!(":(literal){link}"))
            .collect();
        // In batches: a command line has a length limit.
        for batch in stale.chunks(50) {
            let mut args = vec!["rm", "--cached", "-r", "-f", "-q", "--ignore-unmatch", "--"];
            args.extend(batch.iter().map(String::as_str));
            self.git(&args).await?;
        }
        Ok(())
    }
}

/// Whether one of `names` (paths with `/`) lies inside the directory `dir`.
fn any_inside(names: &[&str], dir: &str) -> bool {
    names.iter().any(|name| {
        name.len() > dir.len()
            && name.as_bytes()[dir.len()] == b'/'
            && name[..dir.len()].eq_ignore_ascii_case(dir)
    })
}

/// A path as a literal `.gitignore` pattern.
fn escape_pattern(path: &str) -> String {
    let mut pattern = String::with_capacity(path.len());
    for c in path.chars() {
        if matches!(c, '\\' | '*' | '?' | '[' | ' ') {
            pattern.push('\\');
        }
        pattern.push(c);
    }
    pattern
}

/// Directories in the workspace that are links to somewhere else, as paths
/// from `root` with `/`. Links are not looked into.
///
/// Only Windows needs this. Git there walks into a junction as if it were a
/// plain directory, so a checkpoint would copy what is behind it and a
/// restore would overwrite and delete files outside the workspace, with the
/// harness's rights rather than the confined tool's. Elsewhere git records a
/// symbolic link as a link and never writes through one.
#[cfg(windows)]
fn linked_dirs(root: &Path) -> Vec<String> {
    use std::os::windows::fs::FileTypeExt;

    let mut links = Vec::new();
    let mut pending = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, prefix)) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let (Ok(kind), Ok(name)) = (entry.file_type(), entry.file_name().into_string()) else {
                continue;
            };
            let relative = format!("{prefix}{name}");
            if kind.is_symlink_dir() {
                links.push(relative);
            } else if kind.is_dir() && name != ".git" {
                pending.push((entry.path(), format!("{relative}/")));
            }
        }
    }
    links.sort();
    links
}

#[cfg(not(windows))]
fn linked_dirs(_root: &Path) -> Vec<String> {
    Vec::new()
}

impl Checkpointer for GitCheckpoints {
    fn snapshot(&self) -> BoxFuture<'_, Result<String, String>> {
        Box::pin(GitCheckpoints::snapshot(self))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::fs;

    fn state(root: &Path) -> BTreeMap<String, String> {
        fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(root, &path, out);
                } else {
                    out.insert(
                        path.strip_prefix(root)
                            .unwrap()
                            .to_string_lossy()
                            // Compare with forward slashes on every platform.
                            .replace('\\', "/"),
                        fs::read_to_string(&path).unwrap(),
                    );
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(root, root, &mut out);
        out
    }

    #[tokio::test]
    async fn restore_brings_back_an_earlier_workspace_and_can_be_undone() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("work");
        fs::create_dir_all(work.join("src")).unwrap();
        fs::write(work.join(".gitignore"), "build/\n").unwrap();
        fs::write(work.join("src/a.rs"), "one").unwrap();
        fs::write(work.join("delete-me"), "soon gone").unwrap();
        fs::create_dir(work.join("build")).unwrap();
        fs::write(work.join("build/artifact"), "v1").unwrap();
        let checkpoints = GitCheckpoints::new(dir.path().join("store"), &work);

        let first = checkpoints.snapshot().await.unwrap();
        assert_eq!(
            checkpoints.snapshot().await.unwrap(),
            first,
            "no change, no new checkpoint"
        );
        let at_first = state(&work);

        fs::write(work.join("src/a.rs"), "two").unwrap();
        fs::write(work.join("src/new file.rs"), "added").unwrap();
        fs::remove_file(work.join("delete-me")).unwrap();
        fs::write(work.join("build/artifact"), "v2").unwrap();
        let second = checkpoints.snapshot().await.unwrap();
        assert_ne!(second, first);
        let at_second = state(&work);

        let restored = checkpoints.restore(&first).await.unwrap();
        assert_eq!(restored.before, second);
        assert_eq!(restored.removed, ["src/new file.rs"]);
        let mut expected = at_first.clone();
        // Ignored files are not part of a checkpoint and keep their content.
        expected.insert("build/artifact".into(), "v2".into());
        assert_eq!(state(&work), expected);

        checkpoints.restore(&restored.before).await.unwrap();
        assert_eq!(state(&work), at_second);

        // The workspace itself gained no git metadata.
        assert!(!work.join(".git").exists());
        assert!(
            checkpoints
                .restore("0000000000000000000000000000000000000000")
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn a_project_with_its_own_repository_is_left_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("work");
        fs::create_dir(&work).unwrap();
        let run = |args: &[&str]| {
            assert!(
                std::process::Command::new("git")
                    .args(args)
                    .current_dir(&work)
                    .env("GIT_CONFIG_GLOBAL", "/dev/null")
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        };
        run(&["init", "-q"]);
        fs::write(work.join("f"), "1").unwrap();
        let head_before = fs::read_to_string(work.join(".git/HEAD")).unwrap();
        let index_existed = work.join(".git/index").exists();

        let checkpoints = GitCheckpoints::new(dir.path().join("store"), &work);
        let first = checkpoints.snapshot().await.unwrap();
        fs::write(work.join("f"), "2").unwrap();
        checkpoints.snapshot().await.unwrap();
        checkpoints.restore(&first).await.unwrap();

        assert_eq!(fs::read_to_string(work.join("f")).unwrap(), "1");
        assert_eq!(
            fs::read_to_string(work.join(".git/HEAD")).unwrap(),
            head_before
        );
        assert_eq!(work.join(".git/index").exists(), index_existed);
    }

    /// Git on Windows walks into a junction as if it were a directory.
    #[cfg(windows)]
    #[tokio::test]
    async fn a_link_out_of_the_workspace_is_neither_saved_nor_restored_through() {
        let junction = |link: &Path, target: &Path| {
            assert!(
                std::process::Command::new("cmd")
                    .args(["/c", "mklink", "/J"])
                    .arg(link)
                    .arg(target)
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        };
        let dir = tempfile::tempdir().unwrap();
        let (work, outside) = (dir.path().join("work"), dir.path().join("outside"));
        fs::create_dir_all(work.join("data")).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(work.join("data/real.txt"), "one").unwrap();
        fs::write(outside.join("theirs.txt"), "v1").unwrap();
        junction(&work.join("sub link [1]"), &outside);
        let checkpoints = GitCheckpoints::new(dir.path().join("store"), &work);

        let first = checkpoints.snapshot().await.unwrap();
        let saved = checkpoints
            .git(&["ls-tree", "-r", "--name-only", &first])
            .await
            .unwrap();
        assert_eq!(
            saved, "data/real.txt",
            "what is behind the link is not saved"
        );

        // Outside changes on its own; the workspace changes too.
        fs::write(outside.join("theirs.txt"), "v2").unwrap();
        fs::write(outside.join("new.txt"), "new").unwrap();
        fs::write(work.join("data/real.txt"), "two").unwrap();
        // A directory of the checkpoint is replaced by a link leading out.
        fs::remove_dir_all(work.join("data")).unwrap();
        junction(&work.join("data"), &outside);
        fs::write(outside.join("real.txt"), "theirs as well").unwrap();

        checkpoints.restore(&first).await.unwrap();
        assert_eq!(
            fs::read_to_string(work.join("data/real.txt")).unwrap(),
            "one"
        );
        assert!(
            work.join("sub link [1]/theirs.txt").exists(),
            "the link stays"
        );
        let read = |name: &str| fs::read_to_string(outside.join(name)).unwrap();
        assert_eq!(
            (read("theirs.txt"), read("new.txt"), read("real.txt")),
            ("v2".into(), "new".into(), "theirs as well".into()),
            "nothing outside the workspace was restored or removed"
        );
    }
}
