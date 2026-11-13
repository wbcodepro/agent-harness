use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use zcode_kernel::BoxFuture;
use zcode_session::ToolSpec;
use zcode_tools::{Effect, Tool, ToolContext, ToolOutput};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Task {
    id: u64,
    title: String,
    status: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    note: String,
}

const STATUSES: &[&str] = &["pending", "in_progress", "done", "blocked"];

/// A task list in one file, shared by every agent working in a workspace.
/// Each change is a read-modify-write under an exclusive file lock, so
/// agents in different processes can use it at once.
#[derive(Debug, Clone)]
pub struct TaskBoard {
    path: PathBuf,
}

impl TaskBoard {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    fn update<T>(
        &self,
        change: impl FnOnce(&mut Vec<Task>) -> Result<T, String>,
    ) -> Result<T, String> {
        let io = |e: std::io::Error| format!("cannot use the task board: {e}");
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(io)?;
        }
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&self.path)
            .map_err(io)?;
        file.lock().map_err(io)?;
        let mut text = String::new();
        file.read_to_string(&mut text).map_err(io)?;
        let mut tasks: Vec<Task> = if text.trim().is_empty() {
            Vec::new()
        } else {
            serde_json::from_str(&text).map_err(|e| format!("the task board is corrupt: {e}"))?
        };
        let before = tasks.clone();
        let result = change(&mut tasks)?;
        if tasks != before {
            let updated = serde_json::to_string_pretty(&tasks).map_err(|e| e.to_string())?;
            file.set_len(0).map_err(io)?;
            file.seek(SeekFrom::Start(0)).map_err(io)?;
            file.write_all(updated.as_bytes()).map_err(io)?;
            file.sync_data().map_err(io)?;
        }
        Ok(result)
    }

    fn render(tasks: &[Task]) -> String {
        if tasks.is_empty() {
            return "The task board is empty.".into();
        }
        tasks
            .iter()
            .map(|task| {
                let note = if task.note.is_empty() {
                    String::new()
                } else {
                    format!(" — {}", task.note)
                };
                format!("#{} [{}] {}{note}", task.id, task.status, task.title)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

enum Action {
    Create,
    Update,
    List,
}

struct BoardTool {
    board: TaskBoard,
    action: Action,
}

/// `task_create`, `task_update` and `task_list` over one board.
pub fn board_tools(board: &TaskBoard) -> Vec<Arc<dyn Tool>> {
    [Action::Create, Action::Update, Action::List]
        .into_iter()
        .map(|action| {
            Arc::new(BoardTool {
                board: board.clone(),
                action,
            }) as Arc<dyn Tool>
        })
        .collect()
}

impl Tool for BoardTool {
    fn spec(&self) -> ToolSpec {
        let (name, description, input_schema) = match self.action {
            Action::Create => (
                "task_create",
                "Add a task to the board shared with your subagents and later sessions in this \
                 workspace. Use the board to plan work that has several parts.",
                json!({"type": "object", "properties": {"title": {"type": "string"}},
                    "required": ["title"], "additionalProperties": false}),
            ),
            Action::Update => (
                "task_update",
                "Change a task's status (pending, in_progress, done, blocked) and optionally \
                 leave a note for whoever reads the board next.",
                json!({"type": "object", "properties": {
                    "id": {"type": "integer"},
                    "status": {"type": "string", "enum": STATUSES},
                    "note": {"type": "string"}},
                    "required": ["id", "status"], "additionalProperties": false}),
            ),
            Action::List => (
                "task_list",
                "Show the shared task board.",
                json!({"type": "object", "properties": {}, "additionalProperties": false}),
            ),
        };
        ToolSpec {
            name: name.into(),
            description: description.into(),
            input_schema,
        }
    }

    /// The board is the harness's own bookkeeping, not the workspace.
    fn effect(&self) -> Effect {
        Effect::ReadOnly
    }

    fn host_side(&self) -> bool {
        true
    }

    /// Changes must land in the order the model wrote them.
    fn concurrent(&self) -> bool {
        false
    }

    fn call<'a>(&'a self, input: &'a Value, _ctx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            let result = match self.action {
                Action::Create => {
                    let Some(title) = input["title"].as_str().filter(|t| !t.trim().is_empty())
                    else {
                        return ToolOutput::error("Invalid input: title is required.");
                    };
                    self.board.update(|tasks| {
                        let id = tasks.iter().map(|task| task.id).max().unwrap_or(0) + 1;
                        tasks.push(Task {
                            id,
                            title: title.trim().to_string(),
                            status: "pending".into(),
                            note: String::new(),
                        });
                        Ok(format!("Created task #{id}."))
                    })
                }
                Action::Update => {
                    let (Some(id), Some(status)) = (input["id"].as_u64(), input["status"].as_str())
                    else {
                        return ToolOutput::error("Invalid input: id and status are required.");
                    };
                    if !STATUSES.contains(&status) {
                        return ToolOutput::error(format!(
                            "Invalid status {status:?}: use one of {}.",
                            STATUSES.join(", ")
                        ));
                    }
                    self.board.update(|tasks| {
                        let task = tasks
                            .iter_mut()
                            .find(|task| task.id == id)
                            .ok_or(format!("There is no task #{id}."))?;
                        task.status = status.to_string();
                        if let Some(note) = input["note"].as_str() {
                            task.note = note.to_string();
                        }
                        Ok(format!("Task #{id} is now {status}."))
                    })
                }
                Action::List => self.board.update(|tasks| Ok(TaskBoard::render(tasks))),
            };
            match result {
                Ok(text) => ToolOutput::ok(text),
                Err(why) => ToolOutput::error(why),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn agents_share_one_board_through_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path());
        // Two agents, each with its own handle on the same file.
        let first = board_tools(&TaskBoard::new(dir.path().join("b/tasks.json")));
        let second = board_tools(&TaskBoard::new(dir.path().join("b/tasks.json")));
        let (create, update, list) = (&first[0], &second[1], &second[2]);

        assert_eq!(
            list.call(&json!({}), &ctx).await.text,
            "The task board is empty."
        );
        create
            .call(&json!({"title": "Write the parser"}), &ctx)
            .await;
        create
            .call(&json!({"title": "Write the tests"}), &ctx)
            .await;
        let updated = update
            .call(
                &json!({"id": 2, "status": "in_progress", "note": "worker B"}),
                &ctx,
            )
            .await;
        assert_eq!(updated, ToolOutput::ok("Task #2 is now in_progress."));
        assert_eq!(
            first[2].call(&json!({}), &ctx).await.text,
            "#1 [pending] Write the parser\n#2 [in_progress] Write the tests — worker B"
        );

        assert!(
            update
                .call(&json!({"id": 9, "status": "done"}), &ctx)
                .await
                .is_error
        );
        assert!(
            update
                .call(&json!({"id": 1, "status": "finished"}), &ctx)
                .await
                .is_error
        );
        assert!(create.call(&json!({"title": " "}), &ctx).await.is_error);
    }
}
