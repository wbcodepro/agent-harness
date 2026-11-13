use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};
use zcode_kernel::BoxFuture;
use zcode_session::ToolSpec;
use zcode_tools::{Effect, Tool, ToolContext, ToolOutput};

/// An instruction pack: a directory with a `SKILL.md` whose front matter
/// names and describes it. Only the description is always in context; the
/// body is loaded when the model asks for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub dir: PathBuf,
    body: String,
}

fn parse(dir: &Path, text: &str) -> Option<Skill> {
    let rest = text.strip_prefix("---")?;
    let (front, body) = rest.split_once("\n---")?;
    let field = |key: &str| {
        front.lines().find_map(|line| {
            let value = line.strip_prefix(key)?.strip_prefix(':')?.trim();
            let value = value.trim_matches(|c| c == '"' || c == '\'');
            (!value.is_empty()).then(|| value.to_string())
        })
    };
    Some(Skill {
        name: field("name").or_else(|| Some(dir.file_name()?.to_string_lossy().into_owned()))?,
        description: field("description")?,
        dir: dir.to_path_buf(),
        body: body.trim_start_matches('\n').trim_end().to_string(),
    })
}

/// Reads a `SKILL.md`: `None` when it has no front matter with a
/// description.
pub fn parse_skill(dir: &Path, text: &str) -> Option<Skill> {
    parse(dir, text)
}

/// Finds skills under each root (`<root>/<name>/SKILL.md`). Earlier roots
/// win when two skills share a name.
pub fn discover(roots: &[PathBuf]) -> Vec<Skill> {
    let mut skills: Vec<Skill> = Vec::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        let mut dirs: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
        dirs.sort();
        for dir in dirs {
            let Ok(text) = std::fs::read_to_string(dir.join("SKILL.md")) else {
                continue;
            };
            if let Some(skill) = parse(&dir, &text)
                && !skills.iter().any(|known| known.name == skill.name)
            {
                skills.push(skill);
            }
        }
    }
    skills
}

/// Text for the system prompt listing what can be loaded. Empty when there
/// are no skills.
pub fn prompt_section(skills: &[Skill]) -> String {
    if skills.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "\n\n# Skills\n\nThese skills hold instructions for specific kinds of task. When a task \
         matches one, call the skill tool with its name before starting, and follow what it says.\n",
    );
    for skill in skills {
        out.push_str(&format!("- {}: {}\n", skill.name, skill.description));
    }
    out
}

/// Loads a skill's instructions into the conversation.
pub struct SkillTool {
    skills: Arc<Vec<Skill>>,
}

impl SkillTool {
    pub fn new(skills: Vec<Skill>) -> Self {
        Self {
            skills: Arc::new(skills),
        }
    }
}

impl Tool for SkillTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "skill".into(),
            description: "Load the instructions of a skill listed in the system prompt.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {"name": {"type": "string", "description": "The skill's name."}},
                "required": ["name"],
                "additionalProperties": false
            }),
        }
    }

    fn effect(&self) -> Effect {
        Effect::ReadOnly
    }

    fn host_side(&self) -> bool {
        true
    }

    fn call<'a>(&'a self, input: &'a Value, _ctx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            let Some(name) = input["name"].as_str() else {
                return ToolOutput::error("Invalid input: name is required.");
            };
            match self.skills.iter().find(|skill| skill.name == name) {
                Some(skill) => ToolOutput::ok(format!(
                    "{}\n\n[Files this skill refers to are under {}]",
                    skill.body,
                    skill.dir.display()
                )),
                None => ToolOutput::error(format!(
                    "There is no skill named {name:?}. Available: {}.",
                    self.skills
                        .iter()
                        .map(|skill| skill.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[tokio::test]
    async fn skills_are_discovered_listed_and_loaded_on_demand() {
        let dir = tempfile::tempdir().unwrap();
        let (project, user) = (dir.path().join("project"), dir.path().join("user"));
        let write = |root: &Path, name: &str, text: &str| {
            fs::create_dir_all(root.join(name)).unwrap();
            fs::write(root.join(name).join("SKILL.md"), text).unwrap();
        };
        write(
            &project,
            "release",
            "---\nname: release\ndescription: \"Cut a release\"\n---\n\nRun `make release`.\n",
        );
        write(
            &user,
            "release",
            "---\nname: release\ndescription: shadowed\n---\nold\n",
        );
        write(
            &user,
            "review",
            "---\ndescription: Review a diff\n---\nRead the diff first.\n",
        );
        write(&user, "broken", "no front matter");

        let skills = discover(&[project.clone(), user, dir.path().join("missing")]);
        let names: Vec<&str> = skills.iter().map(|skill| skill.name.as_str()).collect();
        assert_eq!(
            names,
            ["release", "review"],
            "project wins; name defaults to the directory"
        );

        let section = prompt_section(&skills);
        assert!(section.contains("- release: Cut a release\n- review: Review a diff\n"));
        assert_eq!(prompt_section(&[]), "");

        let tool = SkillTool::new(skills);
        let ctx = ToolContext::new(dir.path());
        let loaded = tool.call(&json!({"name": "release"}), &ctx).await;
        assert!(loaded.text.starts_with("Run `make release`."));
        assert!(
            loaded
                .text
                .contains(&project.join("release").display().to_string())
        );
        let missing = tool.call(&json!({"name": "nope"}), &ctx).await;
        assert!(missing.is_error && missing.text.contains("release, review"));
    }
}
