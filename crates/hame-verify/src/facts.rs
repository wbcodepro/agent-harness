//! What earlier sessions in a project found out the hard way, kept for the
//! next one: the check that passed after a change, and commands that are
//! not installed here.
//!
//! Only things the log proves are kept, never a model's own conclusions,
//! and each is dropped when a later session shows it no longer holds. A
//! session that starts knowing how the project is checked does not spend
//! its first requests finding out.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use hame_agent::TurnHook;
use hame_kernel::BoxFuture;
use hame_session::{Block, Event, Record};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::redcheck::{Evidence, evidence};

/// A check command longer than this is a one-off script, not the
/// project's way of checking itself.
const MAX_CHECK_CHARS: usize = 160;
const MAX_MISSING: usize = 12;

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Facts {
    /// The last command that passed after source was changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    passing_check: Option<String>,
    /// Commands the shell could not find.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    missing_commands: Vec<String>,
}

fn load(path: &Path) -> Facts {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// The facts on file as a paragraph for the system prompt; empty when
/// there are none.
pub fn facts_note(path: &Path) -> String {
    let facts = load(path);
    let mut note = String::new();
    if let Some(check) = &facts.passing_check {
        note.push_str(&format!(
            " The command that last passed after a change was `{check}`."
        ));
    }
    if !facts.missing_commands.is_empty() {
        note.push_str(&format!(
            " These commands were not found on this machine: {}.",
            facts.missing_commands.join(", ")
        ));
    }
    if note.is_empty() {
        return note;
    }
    format!(
        "\nFrom earlier sessions in this project (recorded from what was run, and possibly \
         out of date):{note}"
    )
}

/// The name of the command a shell said it could not find.
fn not_found(output: &str) -> Option<&str> {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    let pattern = PATTERN.get_or_init(|| {
        Regex::new(r"(?m)^(?:\S+: )?(?:line \d+: )?([A-Za-z0-9_.+-]{1,40}): command not found\s*$")
            .expect("valid pattern")
    });
    pattern
        .captures(output)
        .and_then(|found| found.get(1))
        .map(|name| name.as_str())
}

/// Brings `facts` up to date with what the last turn in `records` shows.
fn learn(facts: &mut Facts, records: &[Record]) {
    let start = records
        .iter()
        .rposition(|record| matches!(record.event, Event::TurnStart { .. }))
        .unwrap_or(0);
    let turn = &records[start..];
    for record in turn {
        let Event::ToolCall {
            call_id,
            name,
            input,
        } = &record.event
        else {
            continue;
        };
        let Some(command) = (name == "bash")
            .then(|| input["command"].as_str())
            .flatten()
        else {
            continue;
        };
        let result = turn.iter().find_map(|record| match &record.event {
            Event::ToolResult {
                call_id: id,
                content,
                is_error,
            } if id == call_id => Some((content, *is_error)),
            _ => None,
        });
        let Some((content, failed)) = result else {
            continue;
        };
        let first_word = command.split_whitespace().next().unwrap_or("");
        if !failed {
            // It is there after all: installed since, or a wrong guess.
            facts.missing_commands.retain(|name| name != first_word);
            continue;
        }
        let text: String = content
            .iter()
            .filter_map(|block| match block {
                Block::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        if text.contains("[exit code 127")
            && let Some(name) = not_found(&text)
            && !facts.missing_commands.iter().any(|known| known == name)
            && facts.missing_commands.len() < MAX_MISSING
        {
            facts.missing_commands.push(name.to_string());
        }
    }
    match evidence(records) {
        Evidence::Passed(check)
            if crate::redcheck::looks_like_a_check(&check)
                && check.len() <= MAX_CHECK_CHARS
                && !check.contains('\n') =>
        {
            facts.passing_check = Some(check);
        }
        // The remembered check now fails after a change: it may still be
        // the right one, so it stays; only a newer passing one replaces it.
        _ => {}
    }
}

/// Keeps a project's facts file up to date as turns end.
pub struct ProjectFacts {
    pub path: PathBuf,
}

impl TurnHook for ProjectFacts {
    fn before_stop<'a>(
        &'a self,
        records: &'a [Record],
        _changed: bool,
    ) -> BoxFuture<'a, Option<Vec<Block>>> {
        Box::pin(async move {
            let before = load(&self.path);
            let mut facts = before.clone();
            learn(&mut facts, records);
            if facts != before
                && let Ok(text) = serde_json::to_string_pretty(&facts)
            {
                if let Some(dir) = self.path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                // Losing a fact is not worth failing a turn over.
                let _ = std::fs::write(&self.path, text);
            }
            None
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn turn(steps: &[(&str, serde_json::Value, &str, bool)]) -> Vec<Record> {
        let mut events = vec![Event::TurnStart { turn: 1 }];
        for (n, (name, input, output, failed)) in steps.iter().enumerate() {
            events.push(Event::ToolCall {
                call_id: format!("c{n}"),
                name: (*name).into(),
                input: input.clone(),
            });
            events.push(Event::ToolResult {
                call_id: format!("c{n}"),
                content: vec![Block::text(*output)],
                is_error: *failed,
            });
        }
        events
            .into_iter()
            .enumerate()
            .map(|(seq, event)| Record {
                seq: seq as u64,
                ts: 0,
                event,
            })
            .collect()
    }

    #[tokio::test]
    async fn facts_come_from_what_was_run_and_are_dropped_when_disproved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("facts").join("p.json");
        let hook = ProjectFacts { path: path.clone() };
        assert_eq!(facts_note(&path), "");

        let bash = |command: &str| json!({ "command": command });
        let first = turn(&[
            (
                "bash",
                bash("pytest -q"),
                "[stderr]\nbash: line 1: pytest: command not found\n[exit code 127]",
                true,
            ),
            (
                "write",
                json!({"path": "a.py", "content": "x"}),
                "Wrote",
                false,
            ),
            ("bash", bash("python3 -m pytest -q"), "2 passed", false),
            ("bash", bash("git status"), "clean", false),
        ]);
        assert!(hook.before_stop(&first, true).await.is_none());
        let note = facts_note(&path);
        assert!(note.contains("`python3 -m pytest -q`"), "{note}");
        assert!(note.contains("not found on this machine: pytest"), "{note}");

        // Later the command exists: the fact goes, the check stays.
        let later = turn(&[("bash", bash("pytest --version"), "pytest 9", false)]);
        hook.before_stop(&later, false).await;
        let note = facts_note(&path);
        assert!(!note.contains("not found"), "{note}");
        assert!(note.contains("`python3 -m pytest -q`"));
    }

    #[test]
    fn only_a_shell_saying_so_counts_as_not_found() {
        assert_eq!(not_found("bash: foo: command not found\n"), Some("foo"));
        assert_eq!(
            not_found("x\n/bin/bash: line 3: cargo-nextest: command not found"),
            Some("cargo-nextest")
        );
        assert_eq!(not_found("error: the command not found in docs"), None);
    }
}
