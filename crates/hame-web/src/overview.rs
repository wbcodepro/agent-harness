//! What the session list and header show about a session, read from its
//! log: a title, when it last changed, how its last turn ended, what it
//! has cost.

use hame_session::{Block, Event, Record, TurnEndReason, Usage};
use serde::Serialize;

/// The longest title made from a first message, in characters.
const TITLE_CHARS: usize = 80;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// No turn yet, or the last one completed or was stopped.
    Idle,
    /// The last turn failed or was interrupted.
    Failed,
    /// The task's acceptance conditions were checked and hold.
    Verified,
    /// The task's acceptance conditions were checked and do not hold.
    NotVerified,
}

#[derive(Debug, Clone, Serialize)]
pub struct Overview {
    /// From the first message the user typed; `None` before there is one.
    pub title: Option<String>,
    pub created: u64,
    pub last_activity: u64,
    pub turns: u64,
    pub outcome: Outcome,
    pub usage: Usage,
}

/// A title from the first line of `text`, shortened at a word boundary.
pub fn title_from(text: &str) -> Option<String> {
    let line = text.lines().map(str::trim).find(|line| !line.is_empty())?;
    if line.chars().count() <= TITLE_CHARS {
        return Some(line.to_string());
    }
    let cut: String = line.chars().take(TITLE_CHARS).collect();
    let cut = match cut.rfind(' ') {
        Some(space) if space > TITLE_CHARS / 2 => cut[..space].to_string(),
        _ => cut,
    };
    Some(format!("{}…", cut.trim_end()))
}

pub fn overview(records: &[Record]) -> Overview {
    let mut title = None;
    let mut turns = 0;
    let mut last_end = None;
    let mut verified = None;
    let mut usage = Usage::default();
    for record in records {
        match &record.event {
            Event::UserMessage { content } if title.is_none() => {
                let text: String = content
                    .iter()
                    .filter_map(|block| match block {
                        Block::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                // Messages hame itself writes (verification feedback) are
                // not what the session is about.
                if !text.starts_with("[hame") {
                    title = title_from(&text);
                }
            }
            Event::TurnStart { .. } => turns += 1,
            Event::TurnEnd { reason, .. } => last_end = Some(*reason),
            Event::TaskStart { .. } => verified = None,
            Event::TaskVerified { report } => verified = Some(report.passed),
            Event::AssistantMessage { usage: used, .. } | Event::Compaction { usage: used, .. } => {
                usage.input_tokens += used.input_tokens;
                usage.output_tokens += used.output_tokens;
                usage.cache_read_tokens += used.cache_read_tokens;
                usage.cache_write_tokens += used.cache_write_tokens;
            }
            _ => {}
        }
    }
    let outcome = match (last_end, verified) {
        (Some(TurnEndReason::Failed | TurnEndReason::Interrupted), _) => Outcome::Failed,
        (_, Some(true)) => Outcome::Verified,
        (_, Some(false)) => Outcome::NotVerified,
        _ => Outcome::Idle,
    };
    Overview {
        title,
        created: records.first().map_or(0, |record| record.ts),
        last_activity: records.last().map_or(0, |record| record.ts),
        turns,
        outcome,
        usage,
    }
}

/// Overviews of session logs, read again only when a log changes (its
/// length or modification time), so listing many sessions stays cheap.
/// A log's length and modification time, and what was read from it.
type Cached = (u64, Option<std::time::SystemTime>, Overview);

#[derive(Default)]
pub struct OverviewCache {
    entries: std::sync::Mutex<std::collections::HashMap<String, Cached>>,
}

impl OverviewCache {
    /// The overview of the log at `path`, from `read` when it changed.
    pub fn get(
        &self,
        id: &str,
        path: &std::path::Path,
        read: impl FnOnce() -> Vec<Record>,
    ) -> Overview {
        let stamp = std::fs::metadata(path)
            .map(|meta| (meta.len(), meta.modified().ok()))
            .unwrap_or((0, None));
        if let Some((len, modified, seen)) = self.entries.lock().expect("overviews").get(id)
            && (*len, *modified) == stamp
        {
            return seen.clone();
        }
        let seen = overview(&read());
        self.entries
            .lock()
            .expect("overviews")
            .insert(id.to_string(), (stamp.0, stamp.1, seen.clone()));
        seen
    }

    pub fn forget(&self, id: &str) {
        self.entries.lock().expect("overviews").remove(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_log_is_read_again_only_when_it_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log");
        std::fs::write(&path, "one\n").unwrap();
        let cache = OverviewCache::default();
        let reads = std::cell::Cell::new(0);
        let read = || {
            reads.set(reads.get() + 1);
            Vec::new()
        };
        cache.get("a", &path, read);
        cache.get("a", &path, read);
        assert_eq!(reads.get(), 1, "unchanged: served from the cache");
        std::fs::write(&path, "one\ntwo\n").unwrap();
        cache.get("a", &path, read);
        assert_eq!(reads.get(), 2, "a longer log is read again");
        cache.forget("a");
        cache.get("a", &path, read);
        assert_eq!(reads.get(), 3);
    }

    #[test]
    fn titles_come_from_the_first_line_and_are_shortened_at_a_word() {
        assert_eq!(
            title_from("\n  Fix the build \nmore"),
            Some("Fix the build".into())
        );
        assert_eq!(title_from("   \n"), None);
        let long = "word ".repeat(40);
        let title = title_from(&long).unwrap();
        assert!(title.ends_with('…'));
        assert!(title.chars().count() <= TITLE_CHARS + 1);
        assert!(!title.contains("wor…"));
    }
}
