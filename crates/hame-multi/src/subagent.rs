use std::any::Any;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use hame_agent::{Agent, Session};
use hame_kernel::BoxFuture;
use hame_session::{Block, CreateOptions, Event, SessionId, SessionStore, ToolSpec};
use hame_tools::{Effect, Tool, ToolContext, ToolOutput};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::process::Command;

/// What kind of agent a subagent call needs.
#[derive(Debug, Clone)]
pub struct ChildSpec {
    /// Where the child works.
    pub cwd: PathBuf,
    /// The child may look but not change anything.
    pub read_only: bool,
    /// How many agents are above the child.
    pub depth: u32,
}

/// A ready child agent, with whatever must stay alive while it runs.
pub struct Child {
    pub agent: Agent,
    pub keep_alive: Box<dyn Any + Send + Sync>,
}

/// Builds child agents. Supplied by the application so children get the
/// same model, policy and confinement as their parent.
pub type ChildFactory =
    Arc<dyn Fn(ChildSpec) -> BoxFuture<'static, Result<Child, String>> + Send + Sync>;

/// Delegates a task to a fresh agent and returns its final answer.
pub struct SubagentTool {
    pub factory: ChildFactory,
    pub store: SessionStore,
    /// The parent's working directory.
    pub cwd: PathBuf,
    /// Where isolated worktrees are created.
    pub worktrees: PathBuf,
    /// Depth of the agent that owns this tool.
    pub depth: u32,
    /// Kinds of subagent the model can ask for by name.
    pub kinds: Vec<SubagentKind>,
}

/// A named kind of subagent: extra instructions, and optionally the only
/// tools it gets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentKind {
    pub name: String,
    pub description: String,
    pub instructions: String,
    pub tools: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    task: String,
    #[serde(default)]
    mode: Mode,
    #[serde(default)]
    isolate: bool,
    agent: Option<String>,
}

#[derive(Deserialize, Default, Clone, Copy, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Mode {
    Explore,
    #[default]
    Work,
}

async fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|e| format!("cannot run git: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// A git worktree on its own branch, for one worker.
struct Worktree {
    path: PathBuf,
    branch: String,
}

impl Worktree {
    async fn create(repo: &Path, root: &Path, id: &SessionId) -> Result<Self, String> {
        git(repo, &["rev-parse", "--show-toplevel"])
            .await
            .map_err(|_| {
                "isolate needs the working directory to be a git repository".to_string()
            })?;
        let short = &id.as_str()[id.as_str().len().saturating_sub(12)..];
        let branch = format!("hame/{short}");
        let path = root.join(short);
        std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
        git(
            repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                &branch,
                &path.to_string_lossy(),
            ],
        )
        .await
        .map_err(|e| format!("cannot create a worktree: {e}"))?;
        Ok(Self { path, branch })
    }

    /// Commits whatever the worker left, so the branch holds its work.
    /// Returns a one-line summary of the changes, or `None` if there are none.
    async fn commit(&self, task: &str) -> Result<Option<String>, String> {
        git(&self.path, &["add", "-A"]).await?;
        if git(&self.path, &["status", "--porcelain"])
            .await?
            .is_empty()
        {
            return Ok(None);
        }
        let subject: String = task.lines().next().unwrap_or("").chars().take(60).collect();
        let message = format!("subagent: {subject}");
        if git(&self.path, &["commit", "-q", "-m", &message])
            .await
            .is_err()
        {
            // No identity configured for this repository: commit as hame.
            git(
                &self.path,
                &[
                    "-c",
                    "user.name=hame",
                    "-c",
                    "user.email=hame@localhost",
                    "commit",
                    "-q",
                    "-m",
                    &message,
                ],
            )
            .await?;
        }
        git(&self.path, &["show", "--stat", "--format=", "HEAD"])
            .await
            .map(|stat| stat.lines().last().map(|line| line.trim().to_string()))
    }
}

impl Tool for SubagentTool {
    fn spec(&self) -> ToolSpec {
        let mut properties = json!({
            "task": {"type": "string", "description": "Complete instructions, including what to report back."},
            "mode": {"type": "string", "enum": ["explore", "work"], "description": "Default work."},
            "isolate": {"type": "boolean", "description": "Work in a separate git worktree and branch. Default false."}
        });
        let mut kinds = String::new();
        if !self.kinds.is_empty() {
            let names: Vec<&str> = self.kinds.iter().map(|kind| kind.name.as_str()).collect();
            properties["agent"] = json!({"type": "string", "enum": names,
                "description": "A specialised kind of subagent to use. Omit for a general one."});
            kinds.push_str(" Specialised kinds, chosen with agent:");
            for kind in &self.kinds {
                kinds.push_str(&format!(" {} ({});", kind.name, kind.description));
            }
        }
        ToolSpec {
            name: "subagent".into(),
            description: format!(
                "{}{kinds}",
                "Delegate a self-contained task to a fresh agent and get back its final \
                          answer. The subagent starts with an empty context: it sees none of this \
                          conversation, so put everything it needs in the task. Use it to keep \
                          long investigations out of your own context, and to do independent \
                          pieces of work at the same time: several subagent calls in one message \
                          run in parallel. mode \"explore\" is read-only; mode \"work\" may change \
                          files. Set isolate to give a worker its own git worktree and branch, so \
                          parallel workers cannot overwrite each other; you then merge the branch \
                          it reports."
            ),
            input_schema: json!({
                "type": "object",
                "properties": properties,
                "required": ["task"],
                "additionalProperties": false
            }),
        }
    }

    fn effect(&self) -> Effect {
        Effect::Execute
    }

    fn host_side(&self) -> bool {
        true
    }

    fn concurrent(&self) -> bool {
        true
    }

    fn call<'a>(&'a self, input: &'a Value, _ctx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            let input: Input = match Input::deserialize(input) {
                Ok(input) => input,
                Err(e) => return ToolOutput::error(format!("Invalid input: {e}")),
            };
            match self.run(input).await {
                Ok(text) => ToolOutput::ok(text),
                Err(why) => ToolOutput::error(why),
            }
        })
    }
}

impl SubagentTool {
    async fn run(&self, input: Input) -> Result<String, String> {
        let kind = match &input.agent {
            Some(name) => Some(
                self.kinds
                    .iter()
                    .find(|kind| kind.name == *name)
                    .ok_or_else(|| format!("There is no kind of subagent named {name:?}."))?,
            ),
            None => None,
        };
        let id = SessionId::generate();
        let read_only = input.mode == Mode::Explore;
        let worktree = if input.isolate && !read_only {
            Some(Worktree::create(&self.cwd, &self.worktrees, &id).await?)
        } else {
            None
        };
        let cwd = worktree
            .as_ref()
            .map_or_else(|| self.cwd.clone(), |worktree| worktree.path.clone());

        let mut child = (self.factory)(ChildSpec {
            cwd: cwd.clone(),
            read_only,
            depth: self.depth + 1,
        })
        .await?;
        if let Some(kind) = kind {
            child.agent.system.push_str("\n\n");
            child.agent.system.push_str(&kind.instructions);
            // A kind can narrow the child's tools, never widen them.
            if let Some(allowed) = &kind.tools {
                child
                    .agent
                    .tools
                    .retain(|tool| allowed.contains(&tool.spec().name));
            }
        }
        let writer = self
            .store
            .create(CreateOptions {
                id: Some(id.clone()),
                cwd: cwd.to_string_lossy().into_owned(),
            })
            .map_err(|e| e.to_string())?;
        let records = self.store.read(&id).map_err(|e| e.to_string())?;
        let mut session = Session::new(records, writer);

        let outcome = child
            .agent
            .run_turn(&mut session, vec![Block::text(input.task.clone())])
            .await;
        drop(child.keep_alive);

        let answer = session
            .records()
            .iter()
            .rev()
            .find_map(|record| match &record.event {
                Event::AssistantMessage { content, .. } => {
                    let text: String = content
                        .iter()
                        .filter_map(|block| match block {
                            Block::Text { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect();
                    (!text.trim().is_empty()).then_some(text)
                }
                _ => None,
            })
            .unwrap_or_else(|| "(the subagent gave no answer)".to_string());

        let mut report = answer;
        report.push_str(&format!("\n\n[subagent session {id}"));
        if let Some(worktree) = &worktree {
            match worktree.commit(&input.task).await? {
                Some(stat) => report.push_str(&format!(
                    "; its work is committed on branch {} ({stat}); merge it with: git merge {}",
                    worktree.branch, worktree.branch
                )),
                None => report.push_str("; it changed no files"),
            }
        }
        report.push(']');
        match outcome {
            Ok(()) => Ok(report),
            Err(e) => Err(format!(
                "The subagent stopped early: {e}\n\nWhat it said last:\n{report}"
            )),
        }
    }
}
