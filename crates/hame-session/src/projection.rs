use serde::{Deserialize, Serialize};

use crate::artifact::ArtifactRef;
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

/// Tool results shorter than this are kept even when masked: the note
/// replacing them would save too little.
pub const MASK_MIN_BYTES: usize = 1000;

/// About how long the note standing in for a masked result is.
pub const MASK_NOTE_BYTES: usize = 280;

/// How many of the most recent images in tool results the model is shown
/// in full. Older ones are replaced by a note naming their artifact: a
/// screenshot is costly on every later request, and what a model needs from
/// an old one is usually what it already said about it.
pub const KEEP_IMAGES: usize = 3;

/// Old images are dropped this many at a time. Dropping exactly the one
/// that falls out of the window would change an earlier part of the prompt
/// on every new image, and a provider's prompt cache only covers the
/// unchanged prefix; dropping in steps keeps the prefix stable between
/// steps, at the price of showing up to `IMAGE_STEP - 1` extra (cached)
/// images.
pub const IMAGE_STEP: usize = 3;

/// Choices in how history is projected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryOptions {
    /// At least this many of the newest images in tool results are shown
    /// in full; see [`KEEP_IMAGES`].
    pub keep_images: usize,
    /// Older images are elided this many at a time; 1 elides each as soon
    /// as it falls out of the window. See [`IMAGE_STEP`].
    pub image_step: usize,
}

impl Default for HistoryOptions {
    fn default() -> Self {
        Self {
            keep_images: KEEP_IMAGES,
            image_step: IMAGE_STEP,
        }
    }
}

/// The text standing in for an image the model is no longer shown. It
/// names the artifact in full so the model can ask for it again.
pub fn image_note(artifact: &ArtifactRef, why: &str) -> Block {
    let size = match (artifact.width, artifact.height) {
        (Some(w), Some(h)) => format!(" {w}x{h}"),
        _ => String::new(),
    };
    let target = artifact
        .capture
        .as_ref()
        .map(|capture| format!(", captured from {}", capture.target))
        .unwrap_or_default();
    Block::text(format!(
        "[Image not shown: {why}. It was {}{size}{target}, artifact {}. Capture it again or ask \
         for that artifact if you need to see it.]",
        artifact.mime, artifact.sha256
    ))
}

/// What the model sees in place of an old tool result. Long text is
/// cleared; images always are, since each costs as much as a long text.
fn mask(content: &[Block]) -> Vec<Block> {
    let bytes: usize = content
        .iter()
        .map(|block| match block {
            Block::Text { text } => text.len(),
            _ => 0,
        })
        .sum();
    let mut masked = Vec::with_capacity(content.len());
    let mut text_cleared = false;
    for block in content {
        match block {
            Block::Image { artifact } => {
                masked.push(image_note(
                    artifact,
                    "older outputs were cleared to save context",
                ));
            }
            Block::Text { .. } if bytes >= MASK_MIN_BYTES => {
                if !text_cleared {
                    text_cleared = true;
                    masked.push(Block::text(format!(
                        "[This output ({bytes} bytes) was cleared to save context. Older outputs \
                         are cleared as the conversation grows, so when you read something you \
                         will need later, write the important facts in your reply at once. Run \
                         the tool again if you need this output.]"
                    )));
                }
            }
            other => masked.push(other.clone()),
        }
    }
    masked
}

/// Replaces every image in tool results but the `keep` most recent with a
/// note.
fn retain_images(messages: &mut [Message], keep: usize, step: usize) {
    let is_result_image = |block: &Block| matches!(block, Block::Image { .. });
    let total: usize = messages
        .iter()
        .filter(|message| message.role == Role::Tool)
        .flat_map(|message| &message.content)
        .map(|block| match block {
            Block::ToolResult { content, .. } => {
                content.iter().filter(|b| is_result_image(b)).count()
            }
            _ => 0,
        })
        .sum();
    let step = step.max(1);
    // The oldest images go, a whole number of steps at a time.
    let mut drop = total.saturating_sub(keep) / step * step;
    if drop == 0 {
        return;
    }
    for message in messages.iter_mut() {
        if message.role != Role::Tool {
            continue;
        }
        for block in message.content.iter_mut() {
            let Block::ToolResult { content, .. } = block else {
                continue;
            };
            for inner in content.iter_mut() {
                if drop == 0 {
                    return;
                }
                if let Block::Image { artifact } = inner {
                    *inner = image_note(
                        artifact,
                        &format!("only the {keep} or so most recent images are kept in context"),
                    );
                    drop -= 1;
                }
            }
        }
    }
}

/// Every artifact the log refers to, once each, in order of first
/// reference.
pub fn session_artifacts(records: &[Record]) -> Vec<ArtifactRef> {
    let mut seen = std::collections::HashSet::new();
    let mut found = Vec::new();
    for record in records {
        let content = match &record.event {
            Event::UserMessage { content }
            | Event::AssistantMessage { content, .. }
            | Event::AssistantAttempt { content, .. }
            | Event::ToolResult { content, .. } => content,
            _ => continue,
        };
        for artifact in content.iter().flat_map(Block::artifacts) {
            if seen.insert(artifact.sha256.clone()) {
                found.push(artifact.clone());
            }
        }
    }
    found
}

/// Projects the model-visible history out of a session log.
///
/// The log is never rewritten: the latest `compaction` event hides the
/// messages it covers and stands in for them with its summary, and the
/// latest `system/message` and `tools/declared` win. A `context/masked`
/// event replaces long tool results up to its mark with a short note, and
/// only the most recent images in tool results are shown in full.
///
/// Never reads artifact bytes: a missing artifact does not stop history
/// from being derived; the provider adapter says it is missing.
pub fn derive_history(records: &[Record]) -> History {
    derive_history_with(records, HistoryOptions::default())
}

/// [`derive_history`] with explicit options.
pub fn derive_history_with(records: &[Record], options: HistoryOptions) -> History {
    let compaction = records.iter().rev().find_map(|record| match &record.event {
        Event::Compaction {
            summary, up_to_seq, ..
        } => Some((summary, *up_to_seq)),
        _ => None,
    });

    // A signed thinking block is only valid in the conversation that
    // produced it: same system prompt, same tools, same earlier messages.
    // Once any of those changed, blocks from before the change would be
    // rejected by the provider, so they are left out from then on.
    let context_changed_at = records
        .iter()
        .rev()
        .find(|record| {
            matches!(
                record.event,
                Event::Compaction { .. }
                    | Event::Masked { .. }
                    | Event::ModelSwitched { .. }
                    | Event::SystemMessage { .. }
                    | Event::ToolsDeclared { .. }
            )
        })
        .map_or(0, |record| record.seq);

    let masked_up_to = records
        .iter()
        .filter_map(|record| match record.event {
            Event::Masked { up_to_seq } => Some(up_to_seq),
            _ => None,
        })
        .max();

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
            Event::AssistantMessage { content, .. } => {
                let stale = record.seq < context_changed_at;
                let content: Vec<Block> = content
                    .iter()
                    .filter(|block| !(stale && matches!(block, Block::Thinking { .. })))
                    .cloned()
                    .collect();
                // An assistant turn that was nothing but stale thinking has
                // nothing left to show.
                if !content.is_empty() {
                    history.messages.push(Message {
                        role: Role::Assistant,
                        content,
                    });
                }
            }
            Event::ToolResult {
                call_id,
                content,
                is_error,
            } => {
                let block = Block::ToolResult {
                    call_id: call_id.clone(),
                    content: if masked_up_to.is_some_and(|up_to| record.seq <= up_to) {
                        mask(content)
                    } else {
                        content.clone()
                    },
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
    retain_images(
        &mut history.messages,
        options.keep_images,
        options.image_step,
    );
    history
}
