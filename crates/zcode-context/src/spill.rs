use std::path::PathBuf;

use zcode_agent::ToolRequest;
use zcode_kernel::{BoxFuture, Layer, Next};
use zcode_tools::ToolOutput;

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
            let mut output = next.run(request).await;
            if output.text.len() <= self.threshold {
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
            let text = &output.text;
            let head = floor_boundary(text, self.preview / 2);
            let tail = ceil_boundary(text, text.len() - self.preview / 2);
            let note = match saved {
                Ok(()) => format!(
                    "[Output was {} bytes, {} lines; the middle is omitted here. The full \
                     output is saved at {}. Use read (with offset and limit) or grep on that \
                     file to see more.]",
                    text.len(),
                    text.lines().count(),
                    path.display()
                ),
                Err(e) => format!(
                    "[Output was {} bytes; the middle is omitted and could not be saved ({e}).]",
                    text.len()
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
    use zcode_kernel::Chain;
    use zcode_tools::Effect;

    #[tokio::test]
    async fn long_output_is_saved_whole_and_previewed() {
        let dir = tempfile::tempdir().unwrap();
        let chain = Chain::new();
        chain.push(SpillLayer {
            dir: dir.path().join("spill"),
            threshold: 100,
            preview: 40,
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
}
