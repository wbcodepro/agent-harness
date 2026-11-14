//! Not letting a turn end on a failing check without a word about it.

use regex::Regex;
use zcode_agent::TurnHook;
use zcode_kernel::BoxFuture;
use zcode_session::{Block, Event, Record};

/// Starts the message sent when the model stops with its last check
/// failing.
pub const CHECK_PREFIX: &str = "[zcode check]";

/// How many times in one turn the model is sent back.
const MAX_NUDGES: usize = 2;

/// Sends the model back to work when the last command it ran was a check
/// that failed and it is stopping anyway.
///
/// This needs no configuration, unlike the project's verify commands: the
/// check is whatever the model itself chose to run. A command counts as a
/// check when it looks like a test, build or lint run, when the user's
/// request names it, or when the model ran it more than once this turn.
/// Weaker models in particular run the tests, see them fail, describe what
/// they would do next, and stop.
#[derive(Default)]
pub struct RedCheck;

fn looks_like_a_check(command: &str) -> bool {
    static PATTERN: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    PATTERN
        .get_or_init(|| {
            Regex::new(
                r"(?i)\b(tests?|pytest|unittest|jest|vitest|mocha|rspec|check|lint|tsc|mypy|ruff|clippy|build|compile|make)\b",
            )
            .expect("valid pattern")
        })
        .is_match(command)
}

/// File types whose content can be run or tested.
const SOURCE_EXTENSIONS: [&str; 16] = [
    "py", "rs", "js", "jsx", "ts", "tsx", "go", "java", "kt", "c", "cc", "cpp", "rb", "php", "sh",
    "swift",
];

/// Whether a tool call wrote to a source file.
fn changes_source(name: &str, input: &serde_json::Value) -> bool {
    let is_source = |path: &str| {
        path.trim()
            .rsplit_once('.')
            .is_some_and(|(_, extension)| SOURCE_EXTENSIONS.contains(&extension))
    };
    match name {
        "edit" | "write" => input["path"].as_str().is_some_and(is_source),
        "apply_patch" => input["patch"].as_str().is_some_and(|patch| {
            patch.lines().any(|line| {
                line.strip_prefix("*** Update File: ")
                    .or(line.strip_prefix("*** Add File: "))
                    .is_some_and(is_source)
            })
        }),
        _ => false,
    }
}

fn text_of(content: &[Block]) -> String {
    content
        .iter()
        .filter_map(|block| match block {
            Block::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

impl TurnHook for RedCheck {
    fn before_stop<'a>(
        &'a self,
        records: &'a [Record],
        _changed: bool,
    ) -> BoxFuture<'a, Option<Vec<Block>>> {
        Box::pin(async move {
            let start = records
                .iter()
                .rposition(|record| matches!(record.event, Event::TurnStart { .. }))?;
            let turn = &records[start..];
            let user_messages: Vec<String> = turn
                .iter()
                .filter_map(|record| match &record.event {
                    Event::UserMessage { content } => Some(text_of(content)),
                    _ => None,
                })
                .collect();
            let nudges = user_messages
                .iter()
                .filter(|text| text.starts_with(CHECK_PREFIX))
                .count();
            if nudges >= MAX_NUDGES {
                return None;
            }

            let shell_calls: Vec<(&str, &str)> = turn
                .iter()
                .filter_map(|record| match &record.event {
                    Event::ToolCall {
                        call_id,
                        name,
                        input,
                    } if name == "bash" => Some((call_id.as_str(), input["command"].as_str()?)),
                    _ => None,
                })
                .collect();
            // Code was changed and nothing was run after the last change:
            // the work is unchecked. Said once a turn.
            let last_change = turn.iter().rposition(|record| {
                matches!(&record.event, Event::ToolCall { name, input, .. } if changes_source(name, input))
            });
            let last_run = turn.iter().rposition(
                |record| matches!(&record.event, Event::ToolCall { name, .. } if name == "bash"),
            );
            let unchecked =
                last_change.is_some_and(|change| last_run.is_none_or(|run| run < change));
            let already_said = user_messages
                .iter()
                .any(|text| text.starts_with(CHECK_PREFIX) && text.contains("ran nothing"));
            if unchecked && !already_said {
                return Some(vec![Block::text(format!(
                    "{CHECK_PREFIX} You changed code and ran nothing afterwards, so it is \
                     unchecked. Before you finish, run it against what was asked: execute \
                     it, or write and run a short check of each requirement, edge cases \
                     included. Fix what that turns up, then say what you verified."
                ))]);
            }

            let failed_call = |id: &str| {
                turn.iter().any(|record| {
                    matches!(&record.event, Event::ToolResult { call_id, is_error: true, .. }
                        if call_id == id)
                })
            };
            let (last_id, command) = *shell_calls.last()?;
            if !failed_call(last_id) {
                // The last command passed, but it may be an unrelated one
                // (`git status`) run after a check that failed and was
                // never seen to pass.
                let mut red = None;
                for (id, run) in &shell_calls {
                    if !looks_like_a_check(run) {
                        continue;
                    }
                    red = failed_call(id).then_some(*run);
                }
                let run = red?;
                let shown: String = run.chars().take(200).collect();
                return Some(vec![Block::text(format!(
                    "{CHECK_PREFIX} `{shown}` failed earlier in this turn and no check has \
                     passed since. Run it again: if it still fails, fix the cause and rerun \
                     until it passes. If it cannot pass, or its failure does not matter, say \
                     so explicitly and why."
                ))]);
            }
            let times = shell_calls
                .iter()
                .filter(|(_, run)| *run == command)
                .count();
            let requested = user_messages
                .first()
                .is_some_and(|request| request.contains(command.trim()));
            if !(times >= 2 || requested || looks_like_a_check(command)) {
                return None;
            }
            let shown: String = command.chars().take(200).collect();
            Some(vec![Block::text(format!(
                "{CHECK_PREFIX} The last command you ran, `{shown}`, failed, and you are \
                 stopping without having run it successfully. Do not describe what should be \
                 done next: do it, and run the command again until it passes. If it cannot \
                 pass, or its failure does not matter, say so explicitly and why."
            ))])
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn log(prompt: &str, runs: &[(&str, bool)]) -> Vec<Record> {
        let mut events = vec![
            Event::TurnStart { turn: 1 },
            Event::UserMessage {
                content: vec![Block::text(prompt)],
            },
        ];
        for (n, (command, failed)) in runs.iter().enumerate() {
            events.push(Event::ToolCall {
                call_id: format!("c{n}"),
                name: "bash".into(),
                input: json!({ "command": command }),
            });
            events.push(Event::ToolResult {
                call_id: format!("c{n}"),
                content: vec![Block::text("output")],
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
    async fn stopping_on_a_failing_check_sends_the_model_back_twice_at_most() {
        let objects = |records: Vec<Record>| async move {
            RedCheck.before_stop(&records, true).await.is_some()
        };
        // A test run that failed last.
        assert!(objects(log("Fix it", &[("python3 -m pytest -q", true)])).await);
        // Fixed since: nothing to say.
        assert!(
            !objects(log(
                "Fix it",
                &[("cargo test", true), ("cargo test", false)]
            ))
            .await
        );
        // A command the user named, and one the model kept coming back to.
        assert!(
            objects(log(
                "python3 verify.py must pass",
                &[("python3 verify.py", true)]
            ))
            .await
        );
        assert!(
            objects(log(
                "Fix it",
                &[("./run.sh", true), ("ls", false), ("./run.sh", true)]
            ))
            .await
        );
        // An unrelated command after a failed check does not clear it;
        // a later check that passes does.
        assert!(
            objects(log(
                "Fix it",
                &[("cargo test", true), ("git status", false)]
            ))
            .await
        );
        assert!(
            !objects(log(
                "Fix it",
                &[
                    ("cargo test", true),
                    ("git status", false),
                    ("cargo test -q", false)
                ]
            ))
            .await
        );
        // A failed lookup at the end of a turn is not a failing check.
        assert!(!objects(log("Where is it?", &[("ls missing-dir", true)])).await);
        assert!(!objects(log("Explain", &[])).await);

        // Code written and nothing run: said once, whatever was run before.
        let wrote = |records: &mut Vec<Record>, path: &str| {
            let seq = records.len() as u64;
            records.push(Record {
                seq,
                ts: 0,
                event: Event::ToolCall {
                    call_id: format!("w{seq}"),
                    name: "write".into(),
                    input: json!({"path": path, "content": "x"}),
                },
            });
        };
        let mut records = log("Implement merge", &[("ls", false)]);
        wrote(&mut records, "merge.py");
        let said = RedCheck.before_stop(&records, true).await.unwrap();
        assert!(text_of(&said).contains("ran nothing afterwards"));
        records.push(Record {
            seq: records.len() as u64,
            ts: 0,
            event: Event::UserMessage { content: said },
        });
        assert!(RedCheck.before_stop(&records, true).await.is_none());
        // Notes and documents are not code to run.
        let mut records = log("Write the notes", &[]);
        wrote(&mut records, "NOTES.md");
        assert!(RedCheck.before_stop(&records, true).await.is_none());

        // Sent back twice, then let go.
        let mut records = log("Fix it", &[("make test", true)]);
        for round in 0..3 {
            let said = RedCheck.before_stop(&records, true).await;
            assert_eq!(said.is_some(), round < 2, "round {round}");
            let Some(content) = said else { break };
            assert!(text_of(&content).starts_with(CHECK_PREFIX));
            records.push(Record {
                seq: records.len() as u64,
                ts: 0,
                event: Event::UserMessage { content },
            });
        }
    }
}
