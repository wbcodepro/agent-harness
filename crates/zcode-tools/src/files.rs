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
            match fs::write(&path, &input.content) {
                Ok(()) => ToolOutput::ok(format!(
                    "Wrote {} bytes to {}.",
                    input.content.len(),
                    path.display()
                )),
                Err(e) => ToolOutput::error(format!("Cannot write {}: {e}", path.display())),
            }
        })
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
                return ToolOutput::error("old_string must not be empty.");
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
            match count {
                0 => {
                    return ToolOutput::error(format!(
                        "old_string was not found in {}. Read the file and copy the text exactly.",
                        path.display()
                    ));
                }
                1 => {}
                _ if input.replace_all => {}
                n => {
                    return ToolOutput::error(format!(
                        "old_string occurs {n} times in {}. Add surrounding text to make it \
                         unique, or set replace_all.",
                        path.display()
                    ));
                }
            }
            let updated = text.replace(&input.old_string, &input.new_string);
            match fs::write(&path, updated) {
                Ok(()) => ToolOutput::ok(format!(
                    "Replaced {count} occurrence(s) in {}.",
                    path.display()
                )),
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
