//! A file that already exists has to be read before it is overwritten.
//!
//! Models, weaker ones especially, will write a whole new file over one
//! that came with the project without looking at it: a test the task said
//! must pass, a config with settings they did not know about. The file's
//! content was the information they were missing. Making the read come
//! first costs one call when the overwrite was intended and prevents the
//! mistake when it was not.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use hame_agent::ToolRequest;
use hame_kernel::{BoxFuture, Layer, Next};
use hame_tools::ToolOutput;

/// Refuses a `write` to an existing file the agent has not read, written
/// or edited since this agent was started.
///
/// Only the `write` tool is covered. A shell redirection can still
/// overwrite a file unread; this is a guard against a common mistake, not
/// a protection boundary.
pub struct ReadFirst {
    cwd: PathBuf,
    /// Files whose content the model has seen or produced itself.
    known: Mutex<HashSet<PathBuf>>,
}

impl ReadFirst {
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: cwd.into(),
            known: Mutex::new(HashSet::new()),
        }
    }

    /// One spelling per file: the same file reached by a relative path, an
    /// absolute one or through a link has to count as the same.
    fn key(&self, path: &str) -> PathBuf {
        let joined = if Path::new(path).is_absolute() {
            PathBuf::from(path)
        } else {
            self.cwd.join(path)
        };
        std::fs::canonicalize(&joined).unwrap_or(joined)
    }
}

impl Layer<ToolRequest, ToolOutput> for ReadFirst {
    fn call<'a>(
        &'a self,
        request: ToolRequest,
        next: Next<'a, ToolRequest, ToolOutput>,
    ) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            let tracked = matches!(request.name.as_str(), "read" | "write" | "edit");
            let Some(path) = request.input["path"].as_str().filter(|_| tracked) else {
                return next.run(request).await;
            };
            let key = self.key(path);
            if request.name == "write"
                && key.is_file()
                && !self
                    .known
                    .lock()
                    .expect("read-first lock poisoned")
                    .contains(&key)
            {
                return ToolOutput::error(format!(
                    "{path} already exists and you have not read it. Read it first: it may \
                     hold something the task depends on. Then write it again if you still \
                     mean to replace all of it."
                ));
            }
            let output = next.run(request).await;
            if !output.is_error {
                // A file just created has its final path only now.
                let key = std::fs::canonicalize(&key).unwrap_or(key);
                self.known
                    .lock()
                    .expect("read-first lock poisoned")
                    .insert(key);
            }
            output
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hame_kernel::Chain;
    use hame_tools::Effect;
    use serde_json::json;

    #[tokio::test]
    async fn an_existing_file_must_be_read_before_it_is_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::write(root.join("test_app.py"), "assert app() == 42\n").unwrap();
        let chain: Chain<ToolRequest, ToolOutput> = Chain::new();
        chain.push(ReadFirst::new(&root));
        let call = |name: &'static str, path: String| {
            let request = ToolRequest {
                call_id: "c".into(),
                name: name.into(),
                input: json!({"path": path, "content": "x"}),
                effect: Effect::Write,
            };
            let root = root.clone();
            chain.run(request, move |request: ToolRequest| {
                // Stands in for the tools: a write creates the file.
                if request.name == "write" {
                    let path = request.input["path"].as_str().unwrap();
                    std::fs::write(root.join(path), "x").unwrap();
                }
                async { ToolOutput::ok("ran") }
            })
        };

        let refused = call("write", "test_app.py".into()).await;
        assert!(
            refused.is_error && refused.text.contains("have not read it"),
            "{}",
            refused.text
        );
        assert_eq!(
            std::fs::read_to_string(root.join("test_app.py")).unwrap(),
            "assert app() == 42\n",
            "the file is untouched"
        );

        // Reading it, by any spelling of its path, lifts the refusal.
        assert_eq!(
            call("read", root.join("test_app.py").display().to_string())
                .await
                .text,
            "ran"
        );
        assert_eq!(call("write", "./test_app.py".into()).await.text, "ran");

        // A new file needs no read, and neither does rewriting one's own.
        assert_eq!(call("write", "app.py".into()).await.text, "ran");
        assert_eq!(call("write", "app.py".into()).await.text, "ran");
        // Other tools are not this layer's business.
        assert_eq!(call("bash", "test_app.py".into()).await.text, "ran");
    }
}
