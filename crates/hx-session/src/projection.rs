use serde::{Deserialize, Serialize};

use crate::model::{Block, Event, Record, ToolSpec};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
    /// Carries only `Block::ToolResult` blocks.
    Tool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<Block>,
}

/// What a model request is built from.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct History {
    pub system: Option<String>,
    pub tools: Vec<ToolSpec>,
    pub messages: Vec<Message>,
}

impl History {
    /// Ids of assistant tool calls with no result before the next assistant
    /// message. Providers reject such a history, so this must be empty before
    /// a request is sent.
    pub fn dangling_tool_calls(&self) -> Vec<String> {
        let mut dangling = Vec::new();
        let mut owed: Vec<String> = Vec::new();
        for message in &self.messages {
            match message.role {
                Role::Assistant => {
                    dangling.append(&mut owed);
                    owed.extend(message.content.iter().filter_map(|block| match block {
                        Block::ToolUse { id, .. } => Some(id.clone()),
                        _ => None,
                    }));
                }
                Role::Tool => {
                    for block in &message.content {
                        if let Block::ToolResult { call_id, .. } = block {
                            owed.retain(|id| id != call_id);
                        }
                    }
                }
                Role::User => {}
            }
        }
        dangling.append(&mut owed);
        dangling
    }
}

/// Projects the model-visible history out of a session log.
///
/// The log is never rewritten: the latest `compaction` event hides the
/// messages it covers and stands in for them with its summary, and the
/// latest `system/message` and `tools/declared` win.
pub fn derive_history(records: &[Record]) -> History {
    let compaction = records.iter().rev().find_map(|record| match &record.event {
        Event::Compaction { summary, up_to_seq } => Some((summary, *up_to_seq)),
        _ => None,
    });

    let mut history = History::default();
    if let Some((summary, _)) = compaction {
        history.messages.push(Message {
            role: Role::User,
            content: vec![Block::text(summary.clone())],
        });
    }

    for record in records {
        if let Event::SystemMessage { text } = &record.event {
            history.system = (!text.is_empty()).then(|| text.clone());
            continue;
        }
        if let Event::ToolsDeclared { tools } = &record.event {
            history.tools = tools.clone();
            continue;
        }
        if compaction.is_some_and(|(_, up_to_seq)| record.seq <= up_to_seq) {
            continue;
        }
        match &record.event {
            Event::UserMessage { content } => history.messages.push(Message {
                role: Role::User,
                content: content.clone(),
            }),
            Event::AssistantMessage { content, .. } => history.messages.push(Message {
                role: Role::Assistant,
                content: content.clone(),
            }),
            Event::ToolResult {
                call_id,
                content,
                is_error,
            } => {
                let block = Block::ToolResult {
                    call_id: call_id.clone(),
                    content: content.clone(),
                    is_error: *is_error,
                };
                match history.messages.last_mut() {
                    Some(last) if last.role == Role::Tool => last.content.push(block),
                    _ => history.messages.push(Message {
                        role: Role::Tool,
                        content: vec![block],
                    }),
                }
            }
            _ => {}
        }
    }
    history
}
