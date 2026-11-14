//! Prompt commands and subagent definitions kept as Markdown files, in the
//! layout Claude Code uses: `commands/<name>.md` is a prompt the user
//! invokes as `/<name> arguments`, and `agents/<name>.md` describes a kind
//! of subagent. Both may start with front matter between `---` lines.

use std::path::{Path, PathBuf};

/// A prompt the user can invoke by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub name: String,
    pub description: String,
    body: String,
}

/// A kind of subagent: instructions it works under, and optionally the
/// only tools it may use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentDefinition {
    pub name: String,
    pub description: String,
    pub instructions: String,
    /// Tool names as this harness knows them. `None` leaves the tools as
    /// they are.
    pub tools: Option<Vec<String>>,
}

/// Splits a file into its front matter fields and its body.
fn front_matter(text: &str) -> (Vec<(String, String)>, &str) {
    let Some((front, body)) = text
        .strip_prefix("---")
        .and_then(|rest| rest.split_once("\n---"))
    else {
        return (Vec::new(), text);
    };
    let fields = front
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once(':')?;
            let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
            Some((key.trim().to_string(), value.to_string()))
        })
        .collect();
    (fields, body.trim_start_matches(['\r', '\n']))
}

/// The Markdown files directly under each root, earlier roots first; a
/// name found twice keeps its first file.
fn markdown_files(roots: &[PathBuf]) -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = Vec::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
        paths.sort();
        for path in paths {
            if path.extension().is_none_or(|extension| extension != "md") {
                continue;
            }
            let Some(name) = path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
            else {
                continue;
            };
            if found.iter().any(|(known, _)| *known == name) {
                continue;
            }
            if let Ok(text) = std::fs::read_to_string(&path) {
                found.push((name, text));
            }
        }
    }
    found
}

pub fn discover_commands(roots: &[PathBuf]) -> Vec<Command> {
    markdown_files(roots)
        .into_iter()
        .map(|(name, text)| {
            let (fields, body) = front_matter(&text);
            let field = |key: &str| {
                fields
                    .iter()
                    .find(|(k, _)| k == key)
                    .map(|(_, value)| value.clone())
            };
            Command {
                description: field("description").unwrap_or_default(),
                body: body.trim_end().to_string(),
                name,
            }
        })
        .collect()
}

/// The name Claude Code gives a tool, as this harness names it.
fn local_tool_name(name: &str) -> String {
    match name.trim() {
        "Bash" => "bash".into(),
        "Read" => "read".into(),
        "Write" => "write".into(),
        "Edit" => "edit".into(),
        "Glob" => "glob".into(),
        "Grep" => "grep".into(),
        other => other.to_string(),
    }
}

pub fn discover_agents(roots: &[PathBuf]) -> Vec<AgentDefinition> {
    markdown_files(roots)
        .into_iter()
        .filter_map(|(file_name, text)| {
            let (fields, body) = front_matter(&text);
            let field = |key: &str| {
                fields
                    .iter()
                    .find(|(k, _)| k == key)
                    .map(|(_, value)| value.clone())
                    .filter(|value| !value.is_empty())
            };
            Some(AgentDefinition {
                name: field("name").unwrap_or(file_name),
                // Without a description the model cannot tell when to use it.
                description: field("description")?,
                instructions: body.trim().to_string(),
                tools: field("tools").map(|tools| tools.split(',').map(local_tool_name).collect()),
            })
        })
        .collect()
}

/// If `prompt` invokes one of `commands` (`/name` alone or followed by
/// arguments), the command's prompt with the arguments filled in.
/// Anything else, including a path that happens to start with a slash,
/// is `None`.
pub fn expand(commands: &[Command], prompt: &str) -> Option<String> {
    let rest = prompt.trim_start().strip_prefix('/')?;
    let (name, arguments) = match rest.split_once(char::is_whitespace) {
        Some((name, arguments)) => (name, arguments.trim()),
        None => (rest.trim_end(), ""),
    };
    let command = commands.iter().find(|command| command.name == name)?;
    let mut text = command.body.replace("$ARGUMENTS", arguments);
    // Positional arguments, highest first so `$1` does not eat `$10`.
    let words: Vec<&str> = arguments.split_whitespace().collect();
    for index in (1..=9).rev() {
        text = text.replace(
            &format!("${index}"),
            words.get(index - 1).copied().unwrap_or(""),
        );
    }
    // A command that names no place for its arguments gets them at the end.
    if !arguments.is_empty() && !command.body.contains("$ARGUMENTS") && !command.body.contains("$1")
    {
        text.push_str("\n\n");
        text.push_str(arguments);
    }
    Some(text)
}

/// Where a project and a user keep these files, most specific first.
pub fn standard_roots(home: &Path, cwd: &Path, kind: &str) -> Vec<PathBuf> {
    vec![
        cwd.join(".hame").join(kind),
        cwd.join(".claude").join(kind),
        home.join(kind),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_expand_with_their_arguments_and_paths_are_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let (project, user) = (dir.path().join("project"), dir.path().join("user"));
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&user).unwrap();
        std::fs::write(
            project.join("review.md"),
            "---\ndescription: Review a change\nargument-hint: <branch>\n---\nReview the diff of $1 against main.\nFocus: $ARGUMENTS\n",
        )
        .unwrap();
        std::fs::write(project.join("tidy.md"), "Tidy the imports.").unwrap();
        std::fs::write(user.join("review.md"), "The user's own review.").unwrap();
        std::fs::write(user.join("notes.txt"), "not a command").unwrap();

        let commands = discover_commands(&[project, user]);
        let names: Vec<&str> = commands.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            ["review", "tidy"],
            "the project's file wins, and only .md counts"
        );
        assert_eq!(commands[0].description, "Review a change");

        assert_eq!(
            expand(&commands, "/review feature-x security").unwrap(),
            "Review the diff of feature-x against main.\nFocus: feature-x security"
        );
        assert_eq!(expand(&commands, "/tidy").unwrap(), "Tidy the imports.");
        assert_eq!(
            expand(&commands, "/tidy src/ only").unwrap(),
            "Tidy the imports.\n\nsrc/ only"
        );
        // Not commands: an unknown name, a path, ordinary text.
        assert_eq!(expand(&commands, "/etc/hosts has what in it?"), None);
        assert_eq!(expand(&commands, "/unknown"), None);
        assert_eq!(expand(&commands, "please /tidy"), None);
    }

    #[test]
    fn agent_files_give_instructions_and_a_tool_list() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("reviewer.md"),
            "---\nname: code-reviewer\ndescription: Reviews changes for bugs.\ntools: Read, Grep, Glob, mcp__docs__search\n---\n\nYou review code. Report findings only.\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("vague.md"),
            "No front matter, so no description.",
        )
        .unwrap();

        let agents = discover_agents(&[dir.path().to_path_buf()]);
        assert_eq!(
            agents,
            [AgentDefinition {
                name: "code-reviewer".into(),
                description: "Reviews changes for bugs.".into(),
                instructions: "You review code. Report findings only.".into(),
                tools: Some(vec![
                    "read".into(),
                    "grep".into(),
                    "glob".into(),
                    "mcp__docs__search".into()
                ]),
            }]
        );
    }
}
