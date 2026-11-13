//! The `apply_patch` tool: several file changes in one call, written as a
//! patch with context lines instead of exact search strings.
//!
//! Some model families are trained on this format and edit more reliably
//! with it than with search-and-replace.

use std::fs;
use std::path::PathBuf;

use serde::Deserialize;
use serde_json::{Value, json};
use zcode_session::ToolSpec;

use crate::{BoxFuture, Effect, Tool, ToolContext, ToolOutput, parse_input};

pub struct ApplyPatch;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PatchInput {
    patch: String,
}

const DESCRIPTION: &str = "Change files with a patch. Format:\n\
*** Begin Patch\n\
*** Add File: path/new.txt\n\
+every line of the new file starts with a plus\n\
*** Update File: path/existing.py\n\
@@ def existing_function():\n\
 an unchanged line starts with a space\n\
-a removed line starts with a minus\n\
+an added line starts with a plus\n\
*** Delete File: path/old.txt\n\
*** End Patch\n\
In an update, give about three unchanged lines before and after each change, copied exactly \
from the file. A line starting with @@ may name the function or class the change is in, and \
starts a new change in the same file. `*** Move to: new/path` directly after an Update File \
line renames the file. Nothing is written unless the whole patch applies.";

#[derive(Debug, PartialEq)]
enum Op {
    Add {
        path: String,
        lines: Vec<String>,
    },
    Delete {
        path: String,
    },
    Update {
        path: String,
        move_to: Option<String>,
        hunks: Vec<Hunk>,
    },
}

#[derive(Debug, PartialEq)]
enum Line {
    Context(String),
    Remove(String),
    Add(String),
}

/// One change: the context and removed lines, found in the file, lose the
/// removed ones and gain the added ones.
#[derive(Debug, Default, PartialEq)]
struct Hunk {
    /// Text after `@@`: a line somewhere above the change, to find it by.
    anchor: Option<String>,
    lines: Vec<Line>,
    /// The hunk ends at the end of the file.
    at_end: bool,
}

impl Hunk {
    fn is_empty(&self) -> bool {
        self.anchor.is_none() && self.lines.is_empty()
    }

    fn changes_something(&self) -> bool {
        self.lines
            .iter()
            .any(|line| !matches!(line, Line::Context(_)))
    }

    /// The lines that must be in the file for the hunk to apply.
    fn old(&self) -> Vec<&str> {
        self.lines
            .iter()
            .filter_map(|line| match line {
                Line::Context(text) | Line::Remove(text) => Some(text.as_str()),
                Line::Add(_) => None,
            })
            .collect()
    }

    /// What replaces them. Unchanged lines are taken from the file, not
    /// from the patch: the patch may have matched them only loosely, and
    /// must not rewrite their whitespace. For the same reason added lines
    /// are moved to the file's indentation when the patch's is off.
    fn replacement(&self, found: &[String]) -> Vec<String> {
        let old = self.old();
        let (theirs, ours) = match (old.first(), found.first()) {
            (Some(theirs), Some(ours)) => {
                (crate::files::indent(theirs), crate::files::indent(ours))
            }
            _ => ("", ""),
        };
        let mut found = found.iter();
        let mut out = Vec::new();
        for line in &self.lines {
            match line {
                Line::Context(_) => out.extend(found.next().cloned()),
                Line::Remove(_) => {
                    found.next();
                }
                Line::Add(text) => out.push(crate::files::reindent(text, theirs, ours)),
            }
        }
        out
    }
}

fn parse(patch: &str) -> Result<Vec<Op>, String> {
    // Models wrap patches in code fences or add a word before them; only
    // what is between the markers counts.
    let lines: Vec<&str> = patch
        .lines()
        .map(|line| line.trim_end_matches('\r'))
        .collect();
    let begin = lines
        .iter()
        .position(|line| line.trim() == "*** Begin Patch")
        .ok_or("The patch must start with the line `*** Begin Patch`.")?;
    let end = lines
        .iter()
        .rposition(|line| line.trim() == "*** End Patch")
        .filter(|end| *end > begin)
        .unwrap_or(lines.len());

    let mut ops: Vec<Op> = Vec::new();
    for (number, line) in lines[begin + 1..end].iter().enumerate() {
        let header = line.trim_end();
        if let Some(path) = header.strip_prefix("*** Add File: ") {
            ops.push(Op::Add {
                path: path.trim().into(),
                lines: Vec::new(),
            });
        } else if let Some(path) = header.strip_prefix("*** Delete File: ") {
            ops.push(Op::Delete {
                path: path.trim().into(),
            });
        } else if let Some(path) = header.strip_prefix("*** Update File: ") {
            ops.push(Op::Update {
                path: path.trim().into(),
                move_to: None,
                hunks: vec![Hunk::default()],
            });
        } else if let Some(target) = header.strip_prefix("*** Move to: ") {
            match ops.last_mut() {
                Some(Op::Update { move_to, hunks, .. }) if hunks.iter().all(Hunk::is_empty) => {
                    *move_to = Some(target.trim().into());
                }
                _ => return Err("`*** Move to:` must directly follow `*** Update File:`.".into()),
            }
        } else if header == "*** End of File" {
            if let Some(Op::Update { hunks, .. }) = ops.last_mut()
                && let Some(hunk) = hunks.last_mut()
            {
                hunk.at_end = true;
            }
        } else {
            match ops.last_mut() {
                None => {
                    return Err(format!(
                        "Line {} of the patch is outside any file section: {line:?}. Start a \
                         section with `*** Add File:`, `*** Update File:` or `*** Delete File:`.",
                        number + 2
                    ));
                }
                Some(Op::Add { lines, .. }) => match line.strip_prefix('+') {
                    Some(text) => lines.push(text.into()),
                    None if line.trim().is_empty() => lines.push(String::new()),
                    None => {
                        return Err(format!(
                            "Every line of an added file must start with `+`: {line:?}"
                        ));
                    }
                },
                Some(Op::Delete { .. }) => {
                    if !line.trim().is_empty() {
                        return Err(format!(
                            "Unexpected line after `*** Delete File:`: {line:?}"
                        ));
                    }
                }
                Some(Op::Update { hunks, .. }) => {
                    if let Some(anchor) = line.strip_prefix("@@") {
                        let anchor = anchor.trim().trim_end_matches("@@").trim();
                        if hunks.last().is_some_and(|hunk| !hunk.is_empty()) {
                            hunks.push(Hunk::default());
                        }
                        if let Some(hunk) = hunks.last_mut() {
                            hunk.anchor = (!anchor.is_empty()).then(|| anchor.to_string());
                        }
                    } else if let Some(hunk) = hunks.last_mut() {
                        hunk.lines.push(if let Some(text) = line.strip_prefix('+') {
                            Line::Add(text.into())
                        } else if let Some(text) = line.strip_prefix('-') {
                            Line::Remove(text.into())
                        } else {
                            // A context line. An empty line that lost its
                            // leading space is still one.
                            Line::Context(line.strip_prefix(' ').unwrap_or(line).into())
                        });
                    }
                }
            }
        }
    }
    for op in &mut ops {
        if let Op::Update { path, hunks, .. } = op {
            hunks.retain(Hunk::changes_something);
            if hunks.is_empty() {
                return Err(format!("The update of {path} changes nothing."));
            }
        }
    }
    if ops.is_empty() {
        return Err("The patch has no file sections.".into());
    }
    Ok(ops)
}

/// Finds where `needle` occurs in `lines` at or after `from`: exactly, then
/// ignoring trailing whitespace, then ignoring indentation too.
fn find(lines: &[String], needle: &[&str], from: usize) -> Option<usize> {
    if needle.len() > lines.len() {
        return None;
    }
    let last = lines.len() - needle.len();
    let passes: [fn(&str) -> &str; 3] = [|line| line, str::trim_end, str::trim];
    passes.iter().find_map(|normalize| {
        (from..=last).find(|&start| {
            needle
                .iter()
                .zip(&lines[start..])
                .all(|(want, have)| normalize(want) == normalize(have))
        })
    })
}

fn apply_hunks(path: &str, text: &str, hunks: &[Hunk]) -> Result<String, String> {
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let ends_with_newline = text.ends_with('\n') || text.is_empty();
    let mut lines: Vec<String> = text
        .lines()
        .map(|line| line.trim_end_matches('\r').to_string())
        .collect();

    let mut cursor = 0;
    for (index, hunk) in hunks.iter().enumerate() {
        let describe = |problem: &str| {
            format!(
                "Change {} of {} to {path} does not apply: {problem} Read the file and copy \
                 the unchanged and removed lines exactly.",
                index + 1,
                hunks.len()
            )
        };
        let old = hunk.old();
        let mut from = cursor;
        if let Some(anchor) = &hunk.anchor {
            // An anchor that cannot be found is not fatal: the context lines
            // decide.
            if let Some(at) = (from..lines.len()).find(|&at| lines[at].trim() == anchor.trim()) {
                from = at;
            }
        }
        let at = if old.is_empty() {
            if hunk.anchor.is_some() && !hunk.at_end {
                (from + 1).min(lines.len())
            } else {
                lines.len()
            }
        } else {
            let found = if hunk.at_end {
                // Search from the end of the file first.
                let tail = lines.len().saturating_sub(old.len());
                find(&lines, &old, tail.max(from)).or_else(|| find(&lines, &old, from))
            } else {
                find(&lines, &old, from)
            };
            // Hunks out of order still apply if their lines are unique.
            match found.or_else(|| find(&lines, &old, 0)) {
                Some(at) => at,
                None => {
                    let first = old.iter().find(|line| !line.trim().is_empty());
                    return Err(describe(&format!(
                        "these lines are not in the file (first one: {:?}).",
                        first.copied().unwrap_or_default()
                    )));
                }
            }
        };
        let end = at + old.len();
        let new = hunk.replacement(&lines[at..end]);
        cursor = at + new.len();
        lines.splice(at..end, new);
    }

    let mut updated = lines.join(newline);
    if ends_with_newline && !updated.is_empty() {
        updated.push_str(newline);
    }
    Ok(updated)
}

/// A file as the patch leaves it: `None` means removed.
type Outcome = (PathBuf, Option<String>);

fn plan(ops: &[Op], ctx: &ToolContext) -> Result<(Vec<Outcome>, Vec<String>), String> {
    let mut outcomes: Vec<Outcome> = Vec::new();
    let mut summary = Vec::new();
    for op in ops {
        match op {
            Op::Add { path, lines } => {
                let target = ctx.resolve(path);
                if target.exists() {
                    return Err(format!(
                        "{path} already exists. Use `*** Update File:` to change it, or delete \
                         it first."
                    ));
                }
                let mut content = lines.join("\n");
                if !content.is_empty() {
                    content.push('\n');
                }
                outcomes.push((target, Some(content)));
                summary.push(format!("A {path}"));
            }
            Op::Delete { path } => {
                let target = ctx.resolve(path);
                if !target.is_file() {
                    return Err(format!("{path} does not exist, so it cannot be deleted."));
                }
                outcomes.push((target, None));
                summary.push(format!("D {path}"));
            }
            Op::Update {
                path,
                move_to,
                hunks,
            } => {
                let source = ctx.resolve(path);
                let text =
                    fs::read_to_string(&source).map_err(|e| format!("Cannot read {path}: {e}"))?;
                let updated = apply_hunks(path, &text, hunks)?;
                match move_to {
                    Some(new_path) => {
                        outcomes.push((source, None));
                        outcomes.push((ctx.resolve(new_path), Some(updated)));
                        summary.push(format!("R {path} -> {new_path}"));
                    }
                    None => {
                        outcomes.push((source, Some(updated)));
                        summary.push(format!("M {path}"));
                    }
                }
            }
        }
    }
    Ok((outcomes, summary))
}

impl Tool for ApplyPatch {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "apply_patch".into(),
            description: DESCRIPTION.into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "patch": {"type": "string", "description": "The whole patch, from `*** Begin Patch` to `*** End Patch`."}
                },
                "required": ["patch"],
                "additionalProperties": false
            }),
        }
    }

    fn effect(&self) -> Effect {
        Effect::Write
    }

    fn call<'a>(&'a self, input: &'a Value, ctx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            let input: PatchInput = match parse_input(input) {
                Ok(input) => input,
                Err(output) => return output,
            };
            // Everything is worked out before anything is written, so a
            // patch that fails halfway leaves the files as they were.
            let planned = parse(&input.patch).and_then(|ops| plan(&ops, ctx));
            let (outcomes, summary) = match planned {
                Ok(planned) => planned,
                Err(problem) => return ToolOutput::error(problem),
            };
            for (path, content) in &outcomes {
                let result = match content {
                    Some(content) => path
                        .parent()
                        .map_or(Ok(()), fs::create_dir_all)
                        .and_then(|()| fs::write(path, content)),
                    None => fs::remove_file(path),
                };
                if let Err(e) = result {
                    return ToolOutput::error(format!(
                        "Cannot write {}: {e}. Earlier files in this patch were already changed.",
                        path.display()
                    ));
                }
            }
            ToolOutput::ok(format!("Applied:\n{}", summary.join("\n")))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn apply(dir: &std::path::Path, patch: &str) -> ToolOutput {
        ApplyPatch
            .call(&json!({ "patch": patch }), &ToolContext::new(dir))
            .await
    }

    #[tokio::test]
    async fn a_patch_adds_updates_renames_and_deletes_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("calc.py"),
            "def add(a, b):\n    return a - b\n\n\ndef mul(a, b):\n    return a + b\n",
        )
        .unwrap();
        fs::write(dir.path().join("old.txt"), "gone\n").unwrap();
        fs::write(dir.path().join("name.txt"), "one\ntwo\n").unwrap();

        let out = apply(
            dir.path(),
            "*** Begin Patch\n\
             *** Add File: docs/new.md\n\
             +# Title\n\
             +\n\
             +Body.\n\
             *** Update File: calc.py\n\
             @@ def add(a, b):\n\
             -    return a - b\n\
             +    return a + b\n\
             @@ def mul(a, b):\n\
             -    return a + b\n\
             +    return a * b\n\
             *** Delete File: old.txt\n\
             *** Update File: name.txt\n\
             *** Move to: renamed.txt\n\
             \x20one\n\
             -two\n\
             +2\n\
             *** End Patch",
        )
        .await;
        assert!(!out.is_error, "{}", out.text);
        assert_eq!(
            out.text,
            "Applied:\nA docs/new.md\nM calc.py\nD old.txt\nR name.txt -> renamed.txt"
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("docs/new.md")).unwrap(),
            "# Title\n\nBody.\n"
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("calc.py")).unwrap(),
            "def add(a, b):\n    return a + b\n\n\ndef mul(a, b):\n    return a * b\n"
        );
        assert!(!dir.path().join("old.txt").exists());
        assert!(!dir.path().join("name.txt").exists());
        assert_eq!(
            fs::read_to_string(dir.path().join("renamed.txt")).unwrap(),
            "one\n2\n"
        );
    }

    #[tokio::test]
    async fn a_patch_that_does_not_fully_apply_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "alpha\nbeta\n").unwrap();
        fs::write(dir.path().join("b.txt"), "gamma\n").unwrap();

        let out = apply(
            dir.path(),
            "*** Begin Patch\n\
             *** Update File: a.txt\n\
             -alpha\n\
             +ALPHA\n\
             *** Update File: b.txt\n\
             -delta\n\
             +DELTA\n\
             *** End Patch",
        )
        .await;
        assert!(out.is_error);
        assert!(out.text.contains("b.txt"), "{}", out.text);
        assert!(out.text.contains("\"delta\""), "{}", out.text);
        assert_eq!(
            fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "alpha\nbeta\n"
        );

        for (patch, expected) in [
            ("-alpha\n+ALPHA", "*** Begin Patch"),
            (
                "*** Begin Patch\n*** Add File: a.txt\n+x\n*** End Patch",
                "already exists",
            ),
            (
                "*** Begin Patch\n*** Delete File: nope.txt\n*** End Patch",
                "does not exist",
            ),
            (
                "*** Begin Patch\n*** Update File: a.txt\n alpha\n*** End Patch",
                "changes nothing",
            ),
        ] {
            let out = apply(dir.path(), patch).await;
            assert!(out.is_error && out.text.contains(expected), "{}", out.text);
        }
    }

    #[tokio::test]
    async fn common_slips_in_a_patch_are_tolerated() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("w.rs"),
            "fn main() {\r\n    let x = 1;\r\n\r\n    print(x);\r\n}\r\n",
        )
        .unwrap();

        // A code fence around the patch, the indentation left off every line
        // (added lines are moved to the file's depth along with the rest),
        // an empty context line without its leading space, no end marker,
        // and a file with Windows line endings.
        let out = apply(
            dir.path(),
            "```\n*** Begin Patch\n*** Update File: w.rs\n let x = 1;\n\n-print(x);\n+println!(\"{x}\");\n",
        )
        .await;
        assert!(!out.is_error, "{}", out.text);
        assert_eq!(
            fs::read_to_string(dir.path().join("w.rs")).unwrap(),
            "fn main() {\r\n    let x = 1;\r\n\r\n    println!(\"{x}\");\r\n}\r\n"
        );

        // Lines added with no context go to the end of the file.
        fs::write(dir.path().join("list.txt"), "a\nb\n").unwrap();
        let out = apply(
            dir.path(),
            "*** Begin Patch\n*** Update File: list.txt\n+c\n*** End of File\n*** End Patch",
        )
        .await;
        assert!(!out.is_error, "{}", out.text);
        assert_eq!(
            fs::read_to_string(dir.path().join("list.txt")).unwrap(),
            "a\nb\nc\n"
        );
    }
}
