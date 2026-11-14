use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::SystemTime;

use globset::{Glob as GlobPattern, GlobMatcher};
use ignore::WalkBuilder;
use regex::{Regex, RegexBuilder};
use serde::Deserialize;
use serde_json::{Value, json};
use zcode_session::ToolSpec;

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

/// A path as shown to the model: with forward slashes on every platform.
fn display_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
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
                          newest first, as paths relative to the working directory. Files \
                          ignored by the project are skipped."
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
                // Relative to the working directory, not to the directory
                // searched, so a path can be handed straight to another tool.
                out.push_str(&display_path(relative(path, &ctx.cwd)));
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
                          lines as path:line:text, with paths relative to the working \
                          directory. A match inside a function or class is \
                          followed by [in <its name>, line <where it starts>]. Files ignored \
                          by the project, binary files and very large files are skipped."
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
                // Parsed only for a file that has a match to show.
                let mut file_scopes: Option<Vec<Scope>> = None;
                for (index, line) in text.lines().enumerate() {
                    if !regex.is_match(line) {
                        continue;
                    }
                    total += 1;
                    if shown < limit {
                        shown += 1;
                        let line: String = line.chars().take(MAX_SHOWN_LINE_CHARS).collect();
                        let name = relative(path, &ctx.cwd);
                        // Which function a line is in is usually the next
                        // thing asked; saying it here saves reading the file.
                        let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                        let inside = enclosing(
                            file_scopes.get_or_insert_with(|| scopes(extension, &text)),
                            index,
                        )
                        .map(|scope| format!("  [in {}, line {}]", scope.name, scope.start + 1))
                        .unwrap_or_default();
                        out.push_str(&format!(
                            "{}:{}:{line}{inside}\n",
                            display_path(name),
                            index + 1
                        ));
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

/// The tree-sitter grammar for a file extension, where one is bundled.
fn grammar(extension: &str) -> Option<tree_sitter::Language> {
    Some(match extension {
        "rs" => tree_sitter_rust::LANGUAGE.into(),
        "py" => tree_sitter_python::LANGUAGE.into(),
        "js" | "jsx" | "mjs" => tree_sitter_javascript::LANGUAGE.into(),
        "ts" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        "tsx" => tree_sitter_typescript::LANGUAGE_TSX.into(),
        "go" => tree_sitter_go::LANGUAGE.into(),
        _ => return None,
    })
}

/// Syntax node kinds that introduce a named definition, across the bundled
/// grammars.
const DEFINITION_KINDS: &[&str] = &[
    // Rust
    "function_item",
    "function_signature_item",
    "struct_item",
    "enum_item",
    "trait_item",
    "type_item",
    "mod_item",
    "const_item",
    "static_item",
    "macro_definition",
    "union_item",
    // Python
    "function_definition",
    "class_definition",
    // JavaScript and TypeScript
    "function_declaration",
    "generator_function_declaration",
    "class_declaration",
    "method_definition",
    "interface_declaration",
    "type_alias_declaration",
    "enum_declaration",
    "abstract_class_declaration",
    // Go
    "method_declaration",
    "type_spec",
];

fn signature_of(line: &str) -> String {
    let shown: String = line.trim().chars().take(110).collect();
    shown.trim_end_matches(['{', ' ', ':']).to_string()
}

/// Definitions and mentioned identifiers of a source file, from its syntax
/// tree. `None` when there is no grammar for the file or it cannot be
/// parsed; the caller then falls back to line patterns.
fn outline(extension: &str, text: &str) -> Option<(Vec<Definition>, HashSet<String>)> {
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&grammar(extension)?).ok()?;
    let tree = parser.parse(text, None)?;
    let source = text.as_bytes();
    let lines: Vec<&str> = text.lines().collect();
    let mut definitions = Vec::new();
    let mut mentions = HashSet::new();

    let mut cursor = tree.walk();
    'walk: loop {
        let node = cursor.node();
        let kind = node.kind();
        // `const f = () => ...` and `const f = function ...` define f.
        let is_function_binding = kind == "variable_declarator"
            && node.child_by_field_name("value").is_some_and(|value| {
                matches!(
                    value.kind(),
                    "arrow_function" | "function_expression" | "function"
                )
            });
        if (DEFINITION_KINDS.contains(&kind) || is_function_binding)
            && let Some(name) = node.child_by_field_name("name")
            && let Ok(name) = name.utf8_text(source)
        {
            let row = node.start_position().row;
            definitions.push(Definition {
                name: name.to_string(),
                line: row + 1,
                signature: signature_of(lines.get(row).copied().unwrap_or("")),
            });
        }
        if kind.ends_with("identifier")
            && let Ok(name) = node.utf8_text(source)
            && name.len() >= 3
        {
            mentions.insert(name.to_string());
        }

        if cursor.goto_first_child() {
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                break 'walk;
            }
        }
    }
    Some((definitions, mentions))
}

/// The first place the syntax tree of `text` has an error, as a 1-based
/// line number. `None` when the text parses cleanly or there is no grammar
/// for the extension.
fn first_syntax_error(extension: &str, text: &str) -> Option<usize> {
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&grammar(extension)?).ok()?;
    let tree = parser.parse(text, None)?;
    if !tree.root_node().has_error() {
        return None;
    }
    let mut cursor = tree.walk();
    loop {
        let node = cursor.node();
        if node.is_error() || node.is_missing() {
            return Some(node.start_position().row + 1);
        }
        // Only subtrees that contain the error are worth entering.
        if node.has_error() && cursor.goto_first_child() {
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return Some(tree.root_node().start_position().row + 1);
            }
        }
    }
}

/// Asks Python itself whether `text` is valid Python. The outer `None`
/// means it could not be asked (no Python to run); the inner one that the
/// text is fine. The bundled grammar is not used for Python because it
/// accepts wrong indentation, which is the mistake that matters most.
fn python_syntax_error(text: &str) -> Option<Option<(usize, String)>> {
    use std::io::Write as _;
    const CHECK: &str = "import ast,sys\ntry:\n ast.parse(sys.stdin.read())\nexcept SyntaxError as e:\n print(f'{e.lineno}:{e.msg}')";
    // Windows has `python`; its `python3` is a stub that runs nothing.
    let output = ["python3", "python"].iter().find_map(|program| {
        let mut child = std::process::Command::new(program)
            .args(["-I", "-c", CHECK])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok()?;
        child.stdin.take()?.write_all(text.as_bytes()).ok()?;
        let output = child.wait_with_output().ok()?;
        output.status.success().then_some(output)
    })?;
    let said = String::from_utf8_lossy(&output.stdout);
    let Some((line, message)) = said.trim().split_once(':') else {
        return Some(None);
    };
    Some(Some((line.parse().ok()?, message.trim().to_string())))
}

/// The first syntax error in `text`, as a 1-based line and what is wrong.
fn syntax_error(extension: &str, text: &str) -> Option<(usize, String)> {
    if extension == "py"
        && let Some(checked) = python_syntax_error(text)
    {
        return checked;
    }
    first_syntax_error(extension, text).map(|line| (line, "syntax error".to_string()))
}

/// What to tell the model when a change left a source file that no longer
/// parses though it did before. Empty when there is nothing to say: no
/// grammar, the file was already broken, or it is still fine.
///
/// A model that breaks the syntax otherwise finds out several steps later,
/// from a test run whose error it then has to connect back to the edit.
pub(crate) fn syntax_warning(path: &Path, before: Option<&str>, after: &str) -> String {
    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let Some((line, message)) = syntax_error(extension, after) else {
        return String::new();
    };
    if before.is_some_and(|before| syntax_error(extension, before).is_some()) {
        return String::new();
    }
    let shown: String = after
        .lines()
        .nth(line.saturating_sub(1))
        .unwrap_or("")
        .chars()
        .take(160)
        .collect();
    format!(
        " Warning: the file no longer parses ({message}, line {line}: `{}`). Check the \
         indentation and brackets of what you just changed and fix it now.",
        shown.trim_end()
    )
}

/// A definition that spans several lines, by its first and last row.
struct Scope {
    start: usize,
    end: usize,
    name: String,
}

/// The multi-line definitions of a source file, for telling which one a
/// given line is inside. Empty when there is no grammar for the file.
fn scopes(extension: &str, text: &str) -> Vec<Scope> {
    let mut found = Vec::new();
    let Some(language) = grammar(extension) else {
        return found;
    };
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&language).is_err() {
        return found;
    }
    let Some(tree) = parser.parse(text, None) else {
        return found;
    };
    let source = text.as_bytes();
    let mut cursor = tree.walk();
    'walk: loop {
        let node = cursor.node();
        let (start, end) = (node.start_position().row, node.end_position().row);
        if end > start
            && DEFINITION_KINDS.contains(&node.kind())
            && let Some(name) = node.child_by_field_name("name")
            && let Ok(name) = name.utf8_text(source)
        {
            found.push(Scope {
                start,
                end,
                name: name.to_string(),
            });
        }
        if cursor.goto_first_child() {
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                break 'walk;
            }
        }
    }
    found
}

/// The innermost definition that `row` is inside of, not counting one that
/// starts on that very line.
fn enclosing(scopes: &[Scope], row: usize) -> Option<&Scope> {
    scopes
        .iter()
        .filter(|scope| scope.start < row && row <= scope.end)
        .max_by_key(|scope| scope.start)
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
                let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
                let (definitions, mentions) = outline(extension, &text).unwrap_or_else(|| {
                    // No grammar for this language: recognise definitions
                    // by the shape of their first line.
                    let definitions = text
                        .lines()
                        .enumerate()
                        .filter_map(|(index, line)| {
                            let captures = definition_regex().captures(line)?;
                            Some(Definition {
                                name: captures[1].to_string(),
                                line: index + 1,
                                signature: signature_of(line),
                            })
                        })
                        .collect();
                    let mentions = identifier_regex()
                        .find_iter(&text)
                        .map(|found| found.as_str().to_string())
                        .collect();
                    (definitions, mentions)
                });
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
                let mut section = format!("\n{}\n", display_path(relative(path, &ctx.cwd)));
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
            // A match inside a definition says which; the innermost one
            // (the method, not its class), and not a definition's own line.
            "src/cart.rs:4:    parse_money(\"0\")  [in total, line 3]\n\
             src/main.py:3:        return total([parse_money('1')])  [in pay, line 2]\n\
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
            single
                .text
                .ends_with("money.rs:2:    cents: i64,  [in Money, line 1]\n"),
            "{}",
            single.text
        );

        // Searching a subdirectory still names files from the working
        // directory, so what comes back can be passed to read or edit.
        let narrowed = Grep
            .call(&json!({"pattern": "todo", "path": "src/util"}), &ctx)
            .await;
        assert_eq!(
            narrowed.text,
            "src/util/money.rs:6:    todo!()  [in parse_money, line 5]\n"
        );
        let listed = Glob
            .call(&json!({"pattern": "*.rs", "path": "src/util"}), &ctx)
            .await;
        assert_eq!(listed.text, "src/util/money.rs\n");

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

    fn names(extension: &str, text: &str) -> Vec<(String, usize)> {
        let (definitions, _) = outline(extension, text).expect("a grammar is bundled");
        definitions.into_iter().map(|d| (d.name, d.line)).collect()
    }

    #[test]
    fn definitions_come_from_the_syntax_tree_not_from_look_alike_text() {
        let rust = "/// fn in_a_comment() {}\nconst TEXT: &str = \"fn in_a_string() {}\";\n\
            impl Cart {\n    #[inline]\n    pub(crate) async fn\n        checkout(&self) {}\n}\n\
            macro_rules! money { () => {}; }\n";
        assert_eq!(
            names("rs", rust),
            [
                ("TEXT".to_string(), 2),
                ("checkout".to_string(), 5),
                ("money".to_string(), 8)
            ]
        );

        let typescript = "export const total = async (items: Item[]) => 0;\nconst limit = 3;\n\
            interface Item { price: number }\nclass Cart {\n  add(item: Item) {}\n}\n";
        assert_eq!(
            names("ts", typescript),
            [
                ("total".to_string(), 1),
                ("Item".to_string(), 3),
                ("Cart".to_string(), 4),
                ("add".to_string(), 5),
            ]
        );

        let go = "package cart\n\ntype Cart struct{}\n\nfunc (c *Cart) Total() int { return 0 }\n";
        assert_eq!(
            names("go", go),
            [("Cart".to_string(), 3), ("Total".to_string(), 5)]
        );

        let python = "class Shop:\n    def open(self):\n        def inner(): pass\n";
        assert_eq!(
            names("py", python),
            [
                ("Shop".to_string(), 1),
                ("open".to_string(), 2),
                ("inner".to_string(), 3)
            ]
        );

        // Languages without a bundled grammar use the line patterns.
        assert!(outline("rb", "def hi; end").is_none());
    }
}
