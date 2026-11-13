use std::fs;

use serde::Deserialize;
use serde_json::{Value, json};
use zcode_session::ToolSpec;

use crate::{BoxFuture, Effect, Tool, ToolContext, ToolOutput, parse_input};

const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;
const DEFAULT_LINE_LIMIT: usize = 2000;
const MAX_LINE_CHARS: usize = 2000;

/// Reads a text file as numbered lines.
pub struct Read;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadInput {
    path: String,
    offset: Option<usize>,
    limit: Option<usize>,
}

impl Tool for Read {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read".into(),
            description: "Read a text file. Returns numbered lines. Use offset and limit to \
                          page through files longer than 2000 lines."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Absolute path, or relative to the working directory."},
                    "offset": {"type": "integer", "minimum": 1, "description": "First line to return, 1-based. Default 1."},
                    "limit": {"type": "integer", "minimum": 1, "description": "Maximum number of lines. Default 2000."}
                },
                "required": ["path"],
                "additionalProperties": false
            }),
        }
    }

    fn effect(&self) -> Effect {
        Effect::ReadOnly
    }

    fn call<'a>(&'a self, input: &'a Value, ctx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            let input: ReadInput = match parse_input(input) {
                Ok(input) => input,
                Err(output) => return output,
            };
            let path = ctx.resolve(&input.path);
            match fs::metadata(&path) {
                Ok(meta) if meta.is_dir() => {
                    return ToolOutput::error(format!("{} is a directory.", path.display()));
                }
                Ok(meta) if meta.len() > MAX_FILE_BYTES => {
                    return ToolOutput::error(format!(
                        "{} is {} bytes; files over {MAX_FILE_BYTES} bytes cannot be read with this tool.",
                        path.display(),
                        meta.len()
                    ));
                }
                Ok(_) => {}
                Err(e) => return ToolOutput::error(format!("Cannot read {}: {e}", path.display())),
            }
            let bytes = match fs::read(&path) {
                Ok(bytes) => bytes,
                Err(e) => return ToolOutput::error(format!("Cannot read {}: {e}", path.display())),
            };
            let Ok(text) = String::from_utf8(bytes) else {
                return ToolOutput::error(format!("{} is not UTF-8 text.", path.display()));
            };
            if text.is_empty() {
                return ToolOutput::ok("(empty file)");
            }

            let offset = input.offset.unwrap_or(1).max(1);
            let limit = input.limit.unwrap_or(DEFAULT_LINE_LIMIT).max(1);
            let total = text.lines().count();
            if offset > total {
                return ToolOutput::error(format!(
                    "offset {offset} is past the end of the file ({total} lines)."
                ));
            }
            let mut out = String::new();
            for (index, line) in text.lines().enumerate().skip(offset - 1).take(limit) {
                let shown: String = line.chars().take(MAX_LINE_CHARS).collect();
                out.push_str(&format!("{}\t{shown}", index + 1));
                if shown.len() < line.len() {
                    out.push_str(" [line truncated]");
                }
                out.push('\n');
            }
            let last = (offset - 1 + limit).min(total);
            if last < total {
                out.push_str(&format!(
                    "[showing lines {offset}-{last} of {total}; continue with offset {}]\n",
                    last + 1
                ));
            }
            ToolOutput::ok(out)
        })
    }
}

/// Creates or overwrites a file.
pub struct Write;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteInput {
    path: String,
    content: String,
}

impl Tool for Write {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "write".into(),
            description: "Create a file, or replace its whole content. Missing parent \
                          directories are created. To change part of a file, use edit."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Absolute path, or relative to the working directory."},
                    "content": {"type": "string", "description": "The full new content of the file."}
                },
                "required": ["path", "content"],
                "additionalProperties": false
            }),
        }
    }

    fn effect(&self) -> Effect {
        Effect::Write
    }

    fn call<'a>(&'a self, input: &'a Value, ctx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            let input: WriteInput = match parse_input(input) {
                Ok(input) => input,
                Err(output) => return output,
            };
            let path = ctx.resolve(&input.path);
            if let Some(parent) = path.parent()
                && let Err(e) = fs::create_dir_all(parent)
            {
                return ToolOutput::error(format!("Cannot create {}: {e}", parent.display()));
            }
            // A model that means to change a few lines sometimes writes
            // only those lines, and loses the rest of the file. It cannot
            // be told apart from a deliberate rewrite, but it can be said.
            let before = fs::read_to_string(&path).ok();
            match fs::write(&path, &input.content) {
                Ok(()) => {
                    let mut said =
                        format!("Wrote {} bytes to {}.", input.content.len(), path.display());
                    if let Some(before) = before
                        && before.len() > 200
                        && input.content.len() * 2 < before.len()
                    {
                        said.push_str(&format!(
                            " Note: this replaced the whole file, which had {} lines and now \
                             has {}. If you meant to change only part of it, the rest is gone: \
                             write the complete file again.",
                            before.lines().count(),
                            input.content.lines().count()
                        ));
                    }
                    ToolOutput::ok(said)
                }
                Err(e) => ToolOutput::error(format!("Cannot write {}: {e}", path.display())),
            }
        })
    }
}

/// The leading whitespace of a line.
pub(crate) fn indent(line: &str) -> &str {
    &line[..line.len() - line.trim_start().len()]
}

/// Moves a line written at indentation `theirs` to where the file has
/// `ours`: adds what is missing, or takes away what is extra.
pub(crate) fn reindent(line: &str, theirs: &str, ours: &str) -> String {
    if theirs == ours || line.trim().is_empty() {
        line.to_string()
    } else if let Some(missing) = ours.strip_suffix(theirs) {
        format!("{missing}{line}")
    } else if let Some(extra) = theirs.strip_suffix(ours) {
        // A line indented less than the surplus loses what it has.
        let surplus = line
            .char_indices()
            .take_while(|(at, c)| *at < extra.len() && c.is_whitespace())
            .count();
        line[surplus..].to_string()
    } else if let Some(rest) = line.strip_prefix(theirs) {
        // Tabs against spaces: swap the leading run.
        format!("{ours}{rest}")
    } else {
        line.to_string()
    }
}

/// Finds `old` in `text` when it differs only in whitespace, and replaces it.
///
/// Models often get indentation, trailing spaces or line endings slightly
/// wrong when quoting code. Two passes, each accepted only when exactly one
/// place matches: lines equal after trimming their ends, then lines equal
/// after trimming both sides. In the second case the replacement is moved
/// to the indentation the file has there.
fn lenient_replace(text: &str, old: &str, new: &str) -> Option<String> {
    let file: Vec<&str> = text.split('\n').collect();
    let wanted: Vec<&str> = old.trim_matches('\n').split('\n').collect();
    if wanted.iter().all(|line| line.trim().is_empty()) || wanted.len() > file.len() {
        return None;
    }
    for both_sides in [false, true] {
        let key = |line: &str| -> String {
            if both_sides {
                line.trim().to_string()
            } else {
                line.trim_end().to_string()
            }
        };
        let hits: Vec<usize> = (0..=file.len() - wanted.len())
            .filter(|&at| {
                wanted
                    .iter()
                    .enumerate()
                    .all(|(k, line)| key(file[at + k]) == key(line))
            })
            .collect();
        let [at] = hits[..] else {
            if hits.len() > 1 {
                return None;
            }
            continue;
        };
        // The replacement is written at the model's idea of the
        // indentation, which the match has just shown to be off. Move it
        // to the file's: add what the model left out, or take away what it
        // added. Without this a correct change lands at the wrong depth,
        // which in Python is a syntax error.
        let (theirs, ours) = (indent(wanted[0]), indent(file[at]));
        let reindent = |line: &str| -> String {
            if both_sides {
                reindent(line, theirs, ours)
            } else {
                line.to_string()
            }
        };
        let ending = if file[at].ends_with('\r') { "\r" } else { "" };
        let mut out: Vec<String> = file[..at].iter().map(|line| line.to_string()).collect();
        for line in new.trim_matches('\n').split('\n') {
            let line = line.trim_end_matches('\r');
            if line.trim().is_empty() {
                out.push(format!("{line}{ending}"));
            } else {
                out.push(format!("{}{ending}", reindent(line)));
            }
        }
        out.extend(
            file[at + wanted.len()..]
                .iter()
                .map(|line| line.to_string()),
        );
        return Some(out.join("\n"));
    }
    None
}

/// Says how far `old` gets in `text` before it stops matching, for the
/// error shown when an edit finds nothing. "Not found" alone sends a model
/// back to retry the same text; the line where it goes wrong does not.
pub(crate) fn where_it_differs(text: &str, old: &str) -> String {
    let clip = |line: &str| -> String { line.trim().chars().take(160).collect() };
    let file: Vec<&str> = text.lines().collect();
    let wanted: Vec<&str> = old.trim_matches('\n').lines().collect();
    let Some(first) = wanted.iter().position(|line| !line.trim().is_empty()) else {
        return String::new();
    };
    // The longest run of old_string's lines found in order in the file,
    // ignoring leading and trailing whitespace.
    let run_from = |at: usize| {
        wanted[first..]
            .iter()
            .zip(&file[at..])
            .take_while(|(want, have)| want.trim() == have.trim())
            .count()
    };
    let best = (0..file.len())
        .filter(|&at| file[at].trim() == wanted[first].trim())
        .map(|at| (run_from(at), at))
        .max_by_key(|(run, at)| (*run, std::cmp::Reverse(*at)));
    match best {
        Some((run, _)) if first + run == wanted.len() => {
            " It matches more than one place when whitespace is ignored: add more surrounding \
             lines to make it unique."
                .to_string()
        }
        Some((run, at)) => {
            let theirs = clip(wanted[first + run]);
            match file.get(at + run) {
                Some(ours) => format!(
                    " It matches the file from line {} for {run} line(s), then differs. Your \
                     next line is `{theirs}`, but line {} of the file is `{}`.",
                    at + 1,
                    at + run + 1,
                    clip(ours)
                ),
                None => format!(
                    " It matches the file from line {} for {run} line(s), but the file ends \
                     there and your text goes on with `{theirs}`.",
                    at + 1
                ),
            }
        }
        None => {
            let later = wanted[first + 1..].iter().find_map(|line| {
                let at = file
                    .iter()
                    .position(|have| !line.trim().is_empty() && have.trim() == line.trim())?;
                Some((clip(line), at + 1))
            });
            match later {
                Some((line, at)) => format!(
                    " Its first line, `{}`, is not in the file, though `{line}` is at line {at}.",
                    clip(wanted[first])
                ),
                None => " None of its lines are in the file; the file may have changed since \
                         you read it."
                    .to_string(),
            }
        }
    }
}

/// Replaces exact text in a file.
pub struct Edit;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EditInput {
    path: String,
    old_string: String,
    new_string: String,
    #[serde(default)]
    replace_all: bool,
}

impl Tool for Edit {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "edit".into(),
            description: "Replace exact text in a file. old_string must match the file \
                          exactly, including whitespace, and must occur once unless \
                          replace_all is true."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Absolute path, or relative to the working directory."},
                    "old_string": {"type": "string", "description": "Text to find. Must not be empty."},
                    "new_string": {"type": "string", "description": "Text to put in its place."},
                    "replace_all": {"type": "boolean", "description": "Replace every occurrence. Default false."}
                },
                "required": ["path", "old_string", "new_string"],
                "additionalProperties": false
            }),
        }
    }

    fn effect(&self) -> Effect {
        Effect::Write
    }

    fn call<'a>(&'a self, input: &'a Value, ctx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            let input: EditInput = match parse_input(input) {
                Ok(input) => input,
                Err(output) => return output,
            };
            if input.old_string.is_empty() {
                // Usually an attempt to add lines. Say how that is done,
                // or the next attempt is a shell one-liner that mangles
                // the file.
                let path = ctx.resolve(&input.path);
                let first = fs::read_to_string(&path)
                    .ok()
                    .and_then(|text| text.lines().next().map(str::to_string));
                return ToolOutput::error(match first {
                    Some(first) => format!(
                        "old_string must not be empty. To add lines, put an existing line \
                         next to where they go in old_string, and that same line together \
                         with the new lines in new_string. To add at the top of this file, \
                         use its first line as old_string: `{}`",
                        first.chars().take(160).collect::<String>()
                    ),
                    None => "old_string must not be empty. To create a file or fill an empty \
                             one, use write."
                        .to_string(),
                });
            }
            if input.old_string == input.new_string {
                return ToolOutput::error("old_string and new_string are identical.");
            }
            let path = ctx.resolve(&input.path);
            let text = match fs::read_to_string(&path) {
                Ok(text) => text,
                Err(e) => return ToolOutput::error(format!("Cannot read {}: {e}", path.display())),
            };
            let count = text.matches(&input.old_string).count();
            let (updated, note) = match count {
                0 => match lenient_replace(&text, &input.old_string, &input.new_string) {
                    Some(updated) => (
                        updated,
                        "Replaced 1 occurrence (matched ignoring whitespace differences)"
                            .to_string(),
                    ),
                    None => {
                        return ToolOutput::error(format!(
                            "old_string was not found in {}.{} Read the file and copy the text exactly.",
                            path.display(),
                            where_it_differs(&text, &input.old_string)
                        ));
                    }
                },
                n if n == 1 || input.replace_all => (
                    text.replace(&input.old_string, &input.new_string),
                    format!("Replaced {n} occurrence(s)"),
                ),
                n => {
                    return ToolOutput::error(format!(
                        "old_string occurs {n} times in {}. Add surrounding text to make it \
                         unique, or set replace_all.",
                        path.display()
                    ));
                }
            };
            match fs::write(&path, updated) {
                Ok(()) => ToolOutput::ok(format!("{note} in {}.", path.display())),
                Err(e) => ToolOutput::error(format!("Cannot write {}: {e}", path.display())),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(dir: &std::path::Path) -> ToolContext {
        ToolContext::new(dir)
    }

    #[tokio::test]
    async fn write_then_read_pages_through_numbered_lines() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx(dir.path());
        let content: String = (1..=5).map(|n| format!("line {n}\n")).collect();

        let wrote = Write
            .call(&json!({"path": "sub/a.txt", "content": content}), &ctx)
            .await;
        assert!(!wrote.is_error, "{}", wrote.text);

        let page = Read
            .call(&json!({"path": "sub/a.txt", "offset": 2, "limit": 2}), &ctx)
            .await;
        assert_eq!(
            page.text,
            "2\tline 2\n3\tline 3\n[showing lines 2-3 of 5; continue with offset 4]\n"
        );

        let rest = Read
            .call(&json!({"path": "sub/a.txt", "offset": 4}), &ctx)
            .await;
        assert_eq!(rest.text, "4\tline 4\n5\tline 5\n");
    }

    #[tokio::test]
    async fn read_reports_problems_the_model_can_act_on() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx(dir.path());
        fs::write(dir.path().join("bin"), [0xff, 0xfe, 0x00]).unwrap();
        fs::write(dir.path().join("one"), "x\n").unwrap();

        for (input, needle) in [
            (json!({"path": "missing"}), "Cannot read"),
            (json!({"path": "."}), "is a directory"),
            (json!({"path": "bin"}), "not UTF-8"),
            (json!({"path": "one", "offset": 9}), "past the end"),
            (json!({"file": "one"}), "Invalid input"),
        ] {
            let out = Read.call(&input, &ctx).await;
            assert!(out.is_error && out.text.contains(needle), "{}", out.text);
        }
    }

    #[test]
    fn whitespace_slips_do_not_fail_an_edit_but_ambiguity_does() {
        let file = "fn main() {\n    if ready {\n        start();   \n    }\n}\n";
        // Trailing spaces missing from the quote.
        assert_eq!(
            lenient_replace(file, "        start();", "        begin();").unwrap(),
            "fn main() {\n    if ready {\n        begin();\n    }\n}\n"
        );
        // Indentation dropped by the model: the replacement is shifted back.
        assert_eq!(
            lenient_replace(
                file,
                "if ready {\n    start();\n}",
                "if ready {\n    begin();\n    log();\n}"
            )
            .unwrap(),
            "fn main() {\n    if ready {\n        begin();\n        log();\n    }\n}\n"
        );
        // Indentation the model added: the replacement is shifted out
        // again, keeping the shape the model gave it.
        assert_eq!(
            lenient_replace(
                file,
                "            if ready {\n                start();\n            }",
                "            if ready && armed {\n                start();\n            }"
            )
            .unwrap(),
            "fn main() {\n    if ready && armed {\n        start();\n    }\n}\n"
        );
        // Tabs where the file has spaces.
        assert_eq!(
            lenient_replace(file, "\t\tstart();", "\t\tbegin();").unwrap(),
            "fn main() {\n    if ready {\n        begin();\n    }\n}\n"
        );
        // Windows line endings are kept.
        assert_eq!(
            lenient_replace("a = 1\r\nb = 2\r\n", "b = 2", "b = 3").unwrap(),
            "a = 1\r\nb = 3\r\n"
        );
        // Two places would match: refuse rather than guess.
        assert!(lenient_replace("  x();\nif a {\n    x();\n}\n", "x();", "y();").is_none());
        assert!(lenient_replace(file, "stop();", "x").is_none());
        assert!(lenient_replace(file, "\n\n", "x").is_none());
    }

    #[test]
    fn a_failed_edit_says_where_the_text_stops_matching() {
        let file = "def total(rows):\n    result = 0\n    for row in rows:\n        result += row.amount\n    return result\n";
        assert_eq!(
            where_it_differs(
                file,
                "def total(rows):\n    result = 0\n    for r in rows:\n"
            ),
            " It matches the file from line 1 for 2 line(s), then differs. Your next line is \
             `for r in rows:`, but line 3 of the file is `for row in rows:`."
        );
        assert_eq!(
            where_it_differs(file, "    return result\n    # done\n"),
            " It matches the file from line 5 for 1 line(s), but the file ends there and your \
             text goes on with `# done`."
        );
        assert_eq!(
            where_it_differs(file, "def sum(rows):\n    result = 0\n"),
            " Its first line, `def sum(rows):`, is not in the file, though `result = 0` is at \
             line 2."
        );
        assert!(where_it_differs(file, "nothing\nlike this").contains("None of its lines"));
        assert!(where_it_differs("x()\nif a:\n    x()\n", "x()").contains("more than one place"));
    }

    #[tokio::test]
    async fn an_empty_old_string_is_answered_with_how_to_add_lines() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx(dir.path());
        fs::write(dir.path().join("a.py"), "def total():\n    return 1\n").unwrap();
        let edit = |path: &'static str| {
            let ctx = ctx.clone();
            async move {
                Edit.call(
                    &json!({"path": path, "old_string": "", "new_string": "import os\n"}),
                    &ctx,
                )
                .await
            }
        };
        let said = edit("a.py").await;
        assert!(said.is_error);
        assert!(
            said.text
                .ends_with("use its first line as old_string: `def total():`"),
            "{}",
            said.text
        );
        assert!(edit("missing.py").await.text.contains("use write"));
        assert_eq!(
            fs::read_to_string(dir.path().join("a.py")).unwrap(),
            "def total():\n    return 1\n"
        );
    }

    #[tokio::test]
    async fn writing_a_fragment_over_a_file_is_pointed_out() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx(dir.path());
        let whole = "line of the original file\n".repeat(20);
        let write = |content: String| {
            let ctx = ctx.clone();
            async move {
                Write
                    .call(&json!({"path": "a.py", "content": content}), &ctx)
                    .await
            }
        };
        assert!(!write(whole.clone()).await.text.contains("Note:"));
        // A deliberate rewrite of similar size says nothing more.
        assert!(
            !write(whole.replace("original", "new"))
                .await
                .text
                .contains("Note:")
        );
        let fragment = write("line of the fixed file\n".into()).await;
        assert!(!fragment.is_error);
        assert!(
            fragment.text.contains("had 20 lines and now has 1"),
            "{}",
            fragment.text
        );
    }

    #[tokio::test]
    async fn edit_requires_one_exact_match_unless_replacing_all() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx(dir.path());
        let path = dir.path().join("a.rs");
        fs::write(&path, "let a = 1;\nlet b = 1;\n").unwrap();
        let edit = |old: &str, new: &str, all: bool| json!({"path": "a.rs", "old_string": old, "new_string": new, "replace_all": all});

        let ambiguous = Edit.call(&edit("= 1", "= 2", false), &ctx).await;
        assert!(ambiguous.is_error && ambiguous.text.contains("occurs 2 times"));
        let missing = Edit.call(&edit("= 9", "= 2", false), &ctx).await;
        assert!(missing.is_error && missing.text.contains("not found"));
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "let a = 1;\nlet b = 1;\n"
        );

        let one = Edit.call(&edit("a = 1", "a = 2", false), &ctx).await;
        assert!(!one.is_error, "{}", one.text);
        let all = Edit.call(&edit("let", "const", true), &ctx).await;
        assert!(!all.is_error, "{}", all.text);
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "const a = 2;\nconst b = 1;\n"
        );
    }
}
