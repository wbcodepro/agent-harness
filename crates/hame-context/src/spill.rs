use std::path::PathBuf;

use hame_agent::ToolRequest;
use hame_kernel::{BoxFuture, Layer, Next};
use hame_tools::ToolOutput;

/// Saves any tool output longer than `threshold` bytes to a file under
/// `dir` and shows the model only its beginning and end, with the file's
/// path so it can read or search the rest on demand.
///
/// Install it outside the redaction layer, so what is written to disk has
/// already had secrets removed.
pub struct SpillLayer {
    pub dir: PathBuf,
    pub threshold: usize,
    /// Bytes of the output kept inline, split between its start and end.
    pub preview: usize,
    /// Leave passing tests and build progress out of command output.
    pub condense: bool,
}

/// The largest index `<= at` that is a character boundary of `text`.
fn floor_boundary(text: &str, mut at: usize) -> usize {
    at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

fn ceil_boundary(text: &str, mut at: usize) -> usize {
    at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at += 1;
    }
    at
}

impl Layer<ToolRequest, ToolOutput> for SpillLayer {
    fn call<'a>(
        &'a self,
        request: ToolRequest,
        next: Next<'a, ToolRequest, ToolOutput>,
    ) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            let name: String = request
                .call_id
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                        c
                    } else {
                        '_'
                    }
                })
                .take(80)
                .collect();
            // A call a script made answers to the script, which wants the
            // data, not a preview; what the script prints is spilled when
            // the script's own call comes back through here.
            let for_script = request.call_id.starts_with("script-");
            let from_shell = request.name == "bash";
            let mut output = next.run(request).await;
            if for_script {
                return output;
            }
            // Passing tests and build progress are left out of a command's
            // output before it is ever logged, so they are not paid for
            // again on each later request. The whole output goes to disk.
            let condensed = if self.condense && from_shell {
                crate::condense::condense(&output.text)
            } else {
                None
            };
            if condensed.is_none() && output.text.len() <= self.threshold {
                return output;
            }

            // Providers reuse call ids across sessions; the timestamp keeps
            // earlier spills from being overwritten.
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_millis());
            let path = self.dir.join(format!("{stamp}-{name}.txt"));
            let saved = std::fs::create_dir_all(&self.dir)
                .and_then(|()| std::fs::write(&path, &output.text));
            // The saved file holds the output as printed; the notes speak
            // of that, not of what is left after lines were taken out.
            let (whole_bytes, whole_lines) = (output.text.len(), output.text.lines().count());
            let mut left_out = String::new();
            if let Some((shown, dropped)) = condensed {
                let whole = output.text.len();
                output.text = shown;
                left_out = format!(
                    " {dropped} lines of passing tests or build progress were also left out."
                );
                if output.text.len() <= self.threshold {
                    if !output.text.ends_with('\n') {
                        output.text.push('\n');
                    }
                    output.text.push_str(&match saved {
                        Ok(()) => format!(
                            "[{dropped} lines of passing tests or build progress were left \
                             out; every other line is as printed. The full output \
                             ({whole} bytes) is saved at {}.]",
                            path.display()
                        ),
                        Err(e) => format!(
                            "[{dropped} lines of passing tests or build progress were left \
                             out; the full output could not be saved ({e}).]"
                        ),
                    });
                    return output;
                }
            }
            let text = &output.text;
            let head = floor_boundary(text, self.preview / 2);
            let tail = ceil_boundary(text, text.len() - self.preview / 2);
            let note = match saved {
                Ok(()) => format!(
                    "[Output was {whole_bytes} bytes, {whole_lines} lines; the middle is omitted \
                     here.{left_out} The full output is saved at {}. Use read (with offset and \
                     limit) or grep on that file to see more.]",
                    path.display()
                ),
                Err(e) => format!(
                    "[Output was {whole_bytes} bytes; the middle is omitted and could not be \
                     saved ({e}).{left_out}]"
                ),
            };
            output.text = format!(
                "{}\n[... {} bytes omitted ...]\n{}\n{note}",
                &text[..head],
                tail - head,
                &text[tail..]
            );
            output
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hame_kernel::Chain;
    use hame_tools::Effect;

    #[tokio::test]
    async fn long_output_is_saved_whole_and_previewed() {
        let dir = tempfile::tempdir().unwrap();
        let chain = Chain::new();
        chain.push(SpillLayer {
            dir: dir.path().join("spill"),
            threshold: 100,
            preview: 40,
            condense: true,
        });
        let request = |call_id: &str| ToolRequest {
            call_id: call_id.into(),
            name: "bash".into(),
            input: serde_json::json!({}),
            effect: Effect::Execute,
        };

        let short = chain
            .run(request("c1"), |_| async { ToolOutput::ok("short") })
            .await;
        assert_eq!(short, ToolOutput::ok("short"));
        assert!(!dir.path().join("spill").exists());

        let long: String = (0..50).map(|n| format!("line {n} é\n")).collect();
        let expected = long.clone();
        let spilled = chain
            .run(request("../c/2"), move |_| {
                let long = long.clone();
                async move { ToolOutput::error(long) }
            })
            .await;
        assert!(spilled.is_error, "the error flag is kept");
        assert!(spilled.text.starts_with("line 0 é\nline 1 é\n"));
        assert!(spilled.text.contains("bytes omitted"));
        assert!(spilled.text.contains("line 49 é\n"));
        assert!(spilled.text.len() < 500);
        // The id cannot steer the file outside the spill directory.
        let path = std::fs::read_dir(dir.path().join("spill"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert!(path.to_string_lossy().ends_with("-___c_2.txt"));
        assert!(spilled.text.contains(&path.display().to_string()));
        assert_eq!(std::fs::read_to_string(path).unwrap(), expected);
    }

    #[tokio::test]
    async fn passing_tests_are_left_out_of_a_command_and_saved() {
        let dir = tempfile::tempdir().unwrap();
        let chain = Chain::new();
        chain.push(SpillLayer {
            dir: dir.path().join("spill"),
            threshold: 10_000,
            preview: 400,
            condense: true,
        });
        let mut log: String = (0..120).map(|n| format!("test t{n} ... ok\n")).collect();
        log.push_str("test broken ... FAILED\ntest result: FAILED. 120 passed; 1 failed\n");
        let run = |name: &str, log: String| {
            let request = ToolRequest {
                call_id: "c1".into(),
                name: name.into(),
                input: serde_json::json!({}),
                effect: Effect::Execute,
            };
            chain.run(request, move |_| {
                let log = log.clone();
                async move { ToolOutput::error(log) }
            })
        };

        let shown = run("bash", log.clone()).await;
        assert!(shown.is_error);
        assert!(shown.text.contains("test broken ... FAILED"));
        assert!(!shown.text.contains("test t7 ... ok"));
        assert!(shown.text.len() < log.len() / 2);
        let saved = std::fs::read_dir(dir.path().join("spill"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(std::fs::read_to_string(&saved).unwrap(), log);
        assert!(shown.text.contains(&saved.display().to_string()));

        // A file that happens to look like a test log is not a command's
        // output: it is shown as it is.
        let read = run("read", log.clone()).await;
        assert_eq!(read.text, log);
    }
}
