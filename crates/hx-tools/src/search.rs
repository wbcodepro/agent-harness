use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::SystemTime;

use globset::{Glob as GlobPattern, GlobMatcher};
use hx_session::ToolSpec;
use ignore::WalkBuilder;
use regex::{Regex, RegexBuilder};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{BoxFuture, Effect, Tool, ToolContext, ToolOutput, parse_input};

const MAX_SEARCHED_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_GLOB_RESULTS: usize = 200;
const DEFAULT_GREP_RESULTS: usize = 200;
const MAX_SHOWN_LINE_CHARS: usize = 300;

/// Files under `root`, skipping what the project ignores and `.git`.
fn walk(root: &Path) -> impl Iterator<Item = PathBuf> {
    WalkBuilder::new(root)
        .hidden(false)
        .filter_entry(|entry| entry.file_name() != ".git")
        .build()
        .flatten()
        .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
        .map(|entry| entry.into_path())
}

fn relative<'a>(path: &'a Path, root: &Path) -> &'a Path {
    path.strip_prefix(root).unwrap_or(path)
}

fn compile_glob(pattern: &str) -> Result<GlobMatcher, ToolOutput> {
    GlobPattern::new(pattern)
        .map(|glob| glob.compile_matcher())
        .map_err(|e| ToolOutput::error(format!("Invalid glob: {e}")))
}

/// Text of a file worth searching: not huge, not binary.
fn searchable_text(path: &Path) -> Option<String> {
    if fs::metadata(path).ok()?.len() > MAX_SEARCHED_FILE_BYTES {
        return None;
    }
    String::from_utf8(fs::read(path).ok()?).ok()
}

/// Finds files by name pattern.
pub struct Glob;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GlobInput {
    pattern: String,
    path: Option<String>,
}

impl Tool for Glob {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "glob".into(),
            description: "List files whose path matches a glob such as **/*.rs or src/*.py, \
                          newest first. Files ignored by the project are skipped."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "Glob matched against paths relative to the search directory."},
                    "path": {"type": "string", "description": "Directory to search. Defaults to the working directory."}
                },
                "required": ["pattern"],
                "additionalProperties": false
            }),
        }
    }

    fn effect(&self) -> Effect {
        Effect::ReadOnly
    }

    fn call<'a>(&'a self, input: &'a Value, ctx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            let input: GlobInput = match parse_input(input) {
                Ok(input) => input,
                Err(output) => return output,
            };
            let matcher = match compile_glob(&input.pattern) {
                Ok(matcher) => matcher,
                Err(output) => return output,
            };
            let root = ctx.resolve(input.path.as_deref().unwrap_or("."));
            if !root.is_dir() {
                return ToolOutput::error(format!("{} is not a directory.", root.display()));
            }
            let mut found: Vec<(SystemTime, PathBuf)> = walk(&root)
                .filter(|path| matcher.is_match(relative(path, &root)))
                .map(|path| {
                    let modified = fs::metadata(&path)
                        .and_then(|meta| meta.modified())
                        .unwrap_or(SystemTime::UNIX_EPOCH);
                    (modified, path)
                })
                .collect();
            if found.is_empty() {
                return ToolOutput::ok("No files matched.");
            }
            found.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
            let total = found.len();
            let mut out = String::new();
            for (_, path) in found.iter().take(MAX_GLOB_RESULTS) {
                out.push_str(&relative(path, &root).to_string_lossy());
                out.push('\n');
            }
            if total > MAX_GLOB_RESULTS {
                out.push_str(&format!(
                    "[{total} files matched; showing the {MAX_GLOB_RESULTS} newest]\n"
                ));
            }
            ToolOutput::ok(out)
        })
    }
}

/// Searches file contents with a regular expression.
pub struct Grep;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GrepInput {
    pattern: String,
    path: Option<String>,
    glob: Option<String>,
    #[serde(default)]
    ignore_case: bool,
    max_results: Option<usize>,
}

impl Tool for Grep {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "grep".into(),
            description: "Search file contents with a regular expression. Returns matching \
                          lines as path:line:text. Files ignored by the project, binary files \
                          and very large files are skipped."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "Regular expression (Rust regex syntax)."},
                    "path": {"type": "string", "description": "File or directory to search. Defaults to the working directory."},
                    "glob": {"type": "string", "description": "Only search files whose relative path matches this glob, e.g. **/*.ts."},
                    "ignore_case": {"type": "boolean", "description": "Case-insensitive match. Default false."},
                    "max_results": {"type": "integer", "minimum": 1, "description": "Maximum matching lines to return. Default 200."}
                },
                "required": ["pattern"],
                "additionalProperties": false
            }),
        }
    }

    fn effect(&self) -> Effect {
        Effect::ReadOnly
    }

    fn call<'a>(&'a self, input: &'a Value, ctx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            let input: GrepInput = match parse_input(input) {
                Ok(input) => input,
                Err(output) => return output,
            };
            let regex = match RegexBuilder::new(&input.pattern)
                .case_insensitive(input.ignore_case)
                .build()
            {
                Ok(regex) => regex,
                Err(e) => return ToolOutput::error(format!("Invalid regular expression: {e}")),
            };
            let matcher = match input.glob.as_deref().map(compile_glob).transpose() {
                Ok(matcher) => matcher,
                Err(output) => return output,
            };
            let root = ctx.resolve(input.path.as_deref().unwrap_or("."));
            if !root.exists() {
                return ToolOutput::error(format!("{} does not exist.", root.display()));
            }
            let limit = input.max_results.unwrap_or(DEFAULT_GREP_RESULTS).max(1);

            let mut files: Vec<PathBuf> = walk(&root)
                .filter(|path| {
                    matcher
                        .as_ref()
                        .is_none_or(|matcher| matcher.is_match(relative(path, &root)))
                })
                .collect();
            files.sort();

            let mut out = String::new();
            let mut shown = 0usize;
            let mut total = 0usize;
            for path in &files {
                let Some(text) = searchable_text(path) else {
                    continue;
                };
                for (index, line) in text.lines().enumerate() {
                    if !regex.is_match(line) {
                        continue;
                    }
                    total += 1;
                    if shown < limit {
                        shown += 1;
                        let line: String = line.chars().take(MAX_SHOWN_LINE_CHARS).collect();
                        let name = if root.is_file() {
                            path.as_path()
                        } else {
                            relative(path, &root)
                        };
                        out.push_str(&format!("{}:{}:{line}\n", name.display(), index + 1));
                    }
                }
            }
            if total == 0 {
                return ToolOutput::ok("No matches.");
            }
            if total > shown {
                out.push_str(&format!(
                    "[{total} matching lines; showing the first {shown}. Narrow the pattern or the path.]\n"
                ));
            }
            ToolOutput::ok(out)
        })
    }
}

/// Summarises a codebase: its most referenced files and what they define.
pub struct RepoMap;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RepoMapInput {
    path: Option<String>,
    max_chars: Option<usize>,
}

const SOURCE_EXTENSIONS: &[&str] = &[
    "rs", "py", "js", "jsx", "ts", "tsx", "mjs", "go", "java", "kt", "swift", "c", "h", "cc",
    "cpp", "hpp", "cs", "rb", "php", "scala", "lua", "sh", "zig", "ex", "exs",
];

fn definition_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r"^\s*(?:(?:pub(?:\([^)]*\))?|export|default|public|private|protected|internal|static|final|abstract|async|unsafe|override|open|declare)\s+)*(?:fn|struct|enum|trait|type|mod|def|class|interface|function\*?|func|fun|module|defmodule|macro_rules!)\s+(?:\([^)]*\)\s*)?([A-Za-z_][A-Za-z0-9_]*)",
        )
        .expect("the definition pattern is valid")
    })
}

fn identifier_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"[A-Za-z_][A-Za-z0-9_]{2,}").expect("valid pattern"))
}

struct Definition {
    name: String,
    line: usize,
    signature: String,
}

impl Tool for RepoMap {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "repo_map".into(),
            description: "Get an overview of a codebase: source files ranked by how much the \
                          rest of the code refers to what they define, each with its main \
                          definitions and their line numbers. Use it first in an unfamiliar \
                          repository, then read the files that matter."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Directory to map. Defaults to the working directory."},
                    "max_chars": {"type": "integer", "minimum": 500, "description": "Size budget for the map. Default 6000."}
                },
                "additionalProperties": false
            }),
        }
    }

    fn effect(&self) -> Effect {
        Effect::ReadOnly
    }

    fn call<'a>(&'a self, input: &'a Value, ctx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            let input: RepoMapInput = match parse_input(input) {
                Ok(input) => input,
                Err(output) => return output,
            };
            let root = ctx.resolve(input.path.as_deref().unwrap_or("."));
            if !root.is_dir() {
                return ToolOutput::error(format!("{} is not a directory.", root.display()));
            }
            let budget = input.max_chars.unwrap_or(6000).max(500);

            // Per file: what it defines and which identifiers it mentions.
            let mut files: Vec<(PathBuf, Vec<Definition>, HashSet<String>)> = Vec::new();
            for path in walk(&root) {
                let is_source = path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| SOURCE_EXTENSIONS.contains(&ext));
                if !is_source {
                    continue;
                }
                let Some(text) = searchable_text(&path) else {
                    continue;
                };
                let mut definitions = Vec::new();
                for (index, line) in text.lines().enumerate() {
                    if let Some(captures) = definition_regex().captures(line) {
                        let signature: String = line.trim().chars().take(110).collect();
                        definitions.push(Definition {
                            name: captures[1].to_string(),
                            line: index + 1,
                            signature: signature.trim_end_matches(['{', ' ', ':']).to_string(),
                        });
                    }
                }
                let mentions = identifier_regex()
                    .find_iter(&text)
                    .map(|found| found.as_str().to_string())
                    .collect();
                files.push((path, definitions, mentions));
            }
            if files.is_empty() {
                return ToolOutput::ok("No source files found.");
            }

            // A definition's weight is the number of other files mentioning
            // its name; a file's rank is the sum over its definitions.
            let mut mentioned_in: HashMap<&str, usize> = HashMap::new();
            for (_, _, mentions) in &files {
                for name in mentions {
                    *mentioned_in.entry(name.as_str()).or_default() += 1;
                }
            }
            let weight = |name: &str| {
                mentioned_in
                    .get(name)
                    .copied()
                    .unwrap_or(1)
                    .saturating_sub(1)
            };
            let mut ranked: Vec<(usize, &PathBuf, &Vec<Definition>)> = files
                .iter()
                .map(|(path, definitions, _)| {
                    let names: HashSet<&str> =
                        definitions.iter().map(|d| d.name.as_str()).collect();
                    (names.into_iter().map(weight).sum(), path, definitions)
                })
                .collect();
            ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));

            let mut out = format!(
                "{} source files; most referenced first. Numbers are line numbers.\n",
                ranked.len()
            );
            let mut listed = 0usize;
            for (_, path, definitions) in &ranked {
                let mut section = format!("\n{}\n", relative(path, &root).display());
                let mut top: Vec<&Definition> = definitions.iter().collect();
                top.sort_by_key(|d| std::cmp::Reverse(weight(&d.name)));
                top.truncate(12);
                top.sort_by_key(|d| d.line);
                for definition in top {
                    section.push_str(&format!(
                        "  {}: {}\n",
                        definition.line, definition.signature
                    ));
                }
                if out.len() + section.len() > budget {
                    break;
                }
                out.push_str(&section);
                listed += 1;
            }
            if listed < ranked.len() {
                out.push_str(&format!(
                    "\n[{} more files not shown; raise max_chars or map a subdirectory]\n",
                    ranked.len() - listed
                ));
            }
            ToolOutput::ok(out)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> (tempfile::TempDir, ToolContext) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src/util")).unwrap();
        fs::create_dir_all(root.join("target")).unwrap();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join(".gitignore"), "target/\n").unwrap();
        fs::write(
            root.join("src/util/money.rs"),
            "pub struct Money {\n    cents: i64,\n}\n\npub fn parse_money(text: &str) -> Money {\n    todo!()\n}\n\nfn helper_unused() {}\n",
        )
        .unwrap();
        fs::write(
            root.join("src/cart.rs"),
            "use crate::util::money::{Money, parse_money};\n\npub fn total(items: &[Money]) -> Money {\n    parse_money(\"0\")\n}\n",
        )
        .unwrap();
        fs::write(
            root.join("src/main.py"),
            "class Checkout:\n    def pay(self):\n        return total([parse_money('1')])\n",
        )
        .unwrap();
        fs::write(
            root.join("target/generated.rs"),
            "pub fn parse_money() {}\n",
        )
        .unwrap();
        fs::write(root.join(".git/config"), "parse_money\n").unwrap();
        fs::write(root.join("logo.bin"), [0u8, 159, 146, 150]).unwrap();
        let ctx = ToolContext::new(root);
        (dir, ctx)
    }

    #[tokio::test]
    async fn glob_lists_matching_files_and_skips_ignored_ones() {
        let (_dir, ctx) = project();
        let out = Glob.call(&json!({"pattern": "**/*.rs"}), &ctx).await;
        let mut lines: Vec<&str> = out.text.lines().collect();
        lines.sort();
        assert_eq!(lines, ["src/cart.rs", "src/util/money.rs"]);

        let none = Glob.call(&json!({"pattern": "*.go"}), &ctx).await;
        assert_eq!(none, ToolOutput::ok("No files matched."));
        let bad = Glob.call(&json!({"pattern": "a{b"}), &ctx).await;
        assert!(bad.is_error);
    }

    #[tokio::test]
    async fn grep_reports_path_line_and_text_within_limits() {
        let (_dir, ctx) = project();
        let out = Grep.call(&json!({"pattern": r"parse_money\("}), &ctx).await;
        assert_eq!(
            out.text,
            "src/cart.rs:4:    parse_money(\"0\")\n\
             src/main.py:3:        return total([parse_money('1')])\n\
             src/util/money.rs:5:pub fn parse_money(text: &str) -> Money {\n"
        );

        let filtered = Grep
            .call(
                &json!({"pattern": "MONEY", "glob": "**/*.py", "ignore_case": true}),
                &ctx,
            )
            .await;
        assert_eq!(filtered.text.lines().count(), 1);

        let capped = Grep
            .call(&json!({"pattern": "Money", "max_results": 1}), &ctx)
            .await;
        assert!(capped.text.contains("matching lines; showing the first 1"));

        let single = Grep
            .call(
                &json!({"pattern": "cents", "path": "src/util/money.rs"}),
                &ctx,
            )
            .await;
        assert!(
            single.text.ends_with("money.rs:2:    cents: i64,\n"),
            "{}",
            single.text
        );

        assert_eq!(
            Grep.call(&json!({"pattern": "zzz"}), &ctx).await,
            ToolOutput::ok("No matches.")
        );
        assert!(Grep.call(&json!({"pattern": "("}), &ctx).await.is_error);
    }

    #[tokio::test]
    async fn repo_map_ranks_files_by_how_much_others_refer_to_them() {
        let (_dir, ctx) = project();
        let out = RepoMap.call(&json!({}), &ctx).await;
        assert!(out.text.starts_with("3 source files"), "{}", out.text);
        let money = out.text.find("src/util/money.rs").unwrap();
        let cart = out.text.find("src/cart.rs").unwrap();
        let main = out.text.find("src/main.py").unwrap();
        // money.rs defines what both other files use; main.py defines
        // nothing anyone else uses.
        assert!(money < cart && cart < main, "{}", out.text);
        assert!(
            out.text
                .contains("  5: pub fn parse_money(text: &str) -> Money")
        );
        assert!(out.text.contains("  1: class Checkout"));
        assert!(!out.text.contains("generated.rs"));

        let tiny = RepoMap.call(&json!({"max_chars": 500}), &ctx).await;
        assert!(tiny.text.len() <= 600);
    }
}
