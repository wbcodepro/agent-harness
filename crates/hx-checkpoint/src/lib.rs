//! Workspace checkpoints.
//!
//! Each checkpoint is a commit in a private git repository that lives
//! outside the workspace and uses the workspace as its work tree. The
//! project's own repository, index and history are never touched, and the
//! project's `.gitignore` decides what is left out.

use std::path::{Component, Path, PathBuf};
use std::process::Stdio;

use hx_agent::Checkpointer;
use hx_kernel::BoxFuture;
use tokio::process::Command;

const LATEST: &str = "refs/hx/latest";

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
            git_dir: hx_kernel::simplify_path(&git_dir.into()),
            work_tree: hx_kernel::simplify_path(&work_tree.into()),
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
                "user.name=hx",
                "-c",
                "user.email=hx@localhost",
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
}
