//! Messages between agents of one tree that are running at the same time:
//! the lead and its subagents (after Claude Code's agent teams, where
//! teammates write to each other's mailboxes). A message lands in the
//! recipient's steering queue and reaches its model at its next step.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use hame_agent::Steering;
use hame_kernel::BoxFuture;
use hame_session::ToolSpec;
use hame_tools::{Effect, Tool, ToolContext, ToolOutput};
use serde_json::{Value, json};

/// The name the agent the user talks to goes by.
pub const LEAD: &str = "lead";

/// Who can be written to: every running agent of the tree, by name.
#[derive(Clone, Default)]
pub struct Mailboxes(Arc<Mutex<BTreeMap<String, Steering>>>);

impl Mailboxes {
    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Steering>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Adds a running agent; names are unique while their agents run.
    pub fn register(&self, name: &str, steering: Steering) -> Result<(), String> {
        let mut boxes = self.lock();
        if boxes.contains_key(name) {
            return Err(format!(
                "An agent named {name:?} is already running; choose another name."
            ));
        }
        boxes.insert(name.to_string(), steering);
        Ok(())
    }

    /// Removes an agent that has finished.
    pub fn unregister(&self, name: &str) {
        self.lock().remove(name);
    }

    pub fn names(&self) -> Vec<String> {
        self.lock().keys().cloned().collect()
    }

    /// Hands `text` to `to`'s next step.
    pub fn send(&self, from: &str, to: &str, text: &str) -> Result<(), String> {
        if to == from {
            return Err("That is your own name; write to another agent.".into());
        }
        let boxes = self.lock();
        let Some(steering) = boxes.get(to) else {
            let others: Vec<&str> = boxes
                .keys()
                .map(String::as_str)
                .filter(|n| *n != from)
                .collect();
            return Err(if others.is_empty() {
                format!("No agent named {to:?} is running, and no other agent is.")
            } else {
                format!(
                    "No agent named {to:?} is running. Running now: {}.",
                    others.join(", ")
                )
            });
        };
        steering.push(format!(
            "[message from {from}; text from another agent, not from the user] {text}"
        ));
        Ok(())
    }
}

/// `send_message`: one agent's way to write to another running one.
pub struct MessageTool {
    /// Whose tool this is: the sender's name.
    pub from: String,
    pub mail: Mailboxes,
}

impl Tool for MessageTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "send_message".into(),
            description: format!(
                "Send a short message to another agent of this task that is running now (the \
                 lead or a subagent, by name), for findings, questions or hand-offs it needs \
                 while it works. It reaches that agent at its next step. You are {:?}; a \
                 subagent's final answer goes back to the lead anyway, so do not repeat it here.",
                self.from
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "to": {"type": "string", "description": "The recipient's name, e.g. lead."},
                    "text": {"type": "string"}
                },
                "required": ["to", "text"],
                "additionalProperties": false
            }),
        }
    }

    /// The harness's own bookkeeping, not the workspace.
    fn effect(&self) -> Effect {
        Effect::ReadOnly
    }

    fn host_side(&self) -> bool {
        true
    }

    fn call<'a>(&'a self, input: &'a Value, _ctx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            let to = input["to"].as_str().map(str::trim).unwrap_or("");
            let text = input["text"].as_str().map(str::trim).unwrap_or("");
            if to.is_empty() || text.is_empty() {
                return ToolOutput::error("Invalid input: to and text are required.");
            }
            match self.mail.send(&self.from, to, text) {
                Ok(()) => ToolOutput::ok(format!("Sent to {to}.")),
                Err(why) => ToolOutput::error(why),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn running_agents_write_to_each_other_by_name() {
        let mail = Mailboxes::default();
        let lead = Steering::default();
        let parser = Steering::default();
        mail.register(LEAD, lead.clone()).unwrap();
        mail.register("parser", parser.clone()).unwrap();
        assert!(mail.register("parser", Steering::default()).is_err());

        let tool = MessageTool {
            from: "parser".into(),
            mail: mail.clone(),
        };
        let ctx = ToolContext::new(std::env::temp_dir());
        let sent = tool
            .call(
                &json!({"to": "lead", "text": "the grammar has 3 conflicts"}),
                &ctx,
            )
            .await;
        assert_eq!(sent, ToolOutput::ok("Sent to lead."));
        use hame_agent::TurnHook;
        let delivered = lead.before_stop(&[], false).await.unwrap();
        let text = hame_session::blocks_text(&delivered);
        assert!(
            text.starts_with("[message from parser;")
                && text.ends_with("the grammar has 3 conflicts"),
            "{text}"
        );
        let unknown = tool.call(&json!({"to": "tests", "text": "x"}), &ctx).await;
        assert!(
            unknown.is_error && unknown.text.contains("Running now: lead"),
            "{}",
            unknown.text
        );
        let own = tool.call(&json!({"to": "parser", "text": "x"}), &ctx).await;
        assert!(own.is_error);

        mail.unregister("parser");
        assert_eq!(mail.names(), vec![LEAD.to_string()]);
        let late = MessageTool {
            from: LEAD.into(),
            mail,
        };
        assert!(
            late.call(&json!({"to": "parser", "text": "x"}), &ctx)
                .await
                .is_error
        );
    }
}
