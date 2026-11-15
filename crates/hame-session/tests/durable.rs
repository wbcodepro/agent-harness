//! Artifacts, action receipts and what survives crashes, forks and
//! garbage collection.

use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use hame_session::artifact::{ArtifactError, ArtifactRef};
use hame_session::contract::{
    ActionJournal, ActionReceipt, ActionStatus, CONTRACT_VERSION, Capability, Channel, Target,
};
use hame_session::{
    Block, Clock, CreateOptions, Event, INTERRUPTED_ACTIONS, INTERRUPTED_RUNNING, MASK_MIN_BYTES,
    Record, Role, SessionId, SessionStore, StopReason, TurnEndReason, Usage, derive_history,
    plan_recovery,
};
use serde_json::json;

struct TickClock(AtomicU64);

impl Clock for TickClock {
    fn now_ms(&self) -> u64 {
        self.0.fetch_add(1, Ordering::Relaxed)
    }
}

/// A store under `<dir>/sessions`, so its artifacts are in `<dir>/artifacts`.
fn store(dir: &std::path::Path) -> SessionStore {
    SessionStore::with_clock(
        dir.join("sessions"),
        Arc::new(TickClock(AtomicU64::new(1000))),
    )
}

fn id(raw: &str) -> SessionId {
    SessionId::parse(raw).unwrap()
}

fn options(raw: &str) -> CreateOptions {
    CreateOptions {
        id: Some(id(raw)),
        cwd: "/work".into(),
    }
}

/// A PNG header of the given size: enough for the store to read its
/// dimensions, and distinct bytes per size.
fn png(width: u8, height: u8) -> Vec<u8> {
    vec![
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0x0d, 0x49, 0x48, 0x44, 0x52, 0,
        0, 0, width, 0, 0, 0, height, 8, 6, 0, 0, 0,
    ]
}

fn receipt(op: &str, status: ActionStatus) -> ActionReceipt {
    ActionReceipt {
        version: CONTRACT_VERSION,
        op_id: op.into(),
        action: "browser.click".into(),
        target: Target {
            channel: Some(Channel::Browser),
            origin: Some("https://shop.example".into()),
            ..Target::default()
        },
        observation_id: None,
        capability: Capability::Commit,
        status,
        idempotency_key: Some(format!("key-{op}")),
        emitted: vec![],
        completed_prefix: 0,
        resulting_observation: None,
        evidence: vec![],
        error: None,
        ts: 1,
    }
}

/// A turn whose model called one tool, `c1`, that has started.
fn open_turn_with_running_call() -> Vec<Event> {
    vec![
        Event::TurnStart { turn: 1 },
        Event::UserMessage {
            content: vec![Block::text("Buy the mug")],
        },
        Event::StepStart { turn: 1, step: 1 },
        Event::AssistantMessage {
            content: vec![Block::ToolUse {
                id: "c1".into(),
                name: "browser".into(),
                input: json!({"op": "click"}),
            }],
            stop: StopReason::ToolUse,
            usage: Usage::default(),
        },
        Event::ToolCall {
            call_id: "c1".into(),
            name: "browser".into(),
            input: json!({"op": "click"}),
        },
    ]
}

fn receipts_in(records: &[Record]) -> Vec<ActionReceipt> {
    records
        .iter()
        .filter_map(|record| match &record.event {
            Event::ActionReceipt { receipt } => Some((**receipt).clone()),
            _ => None,
        })
        .collect()
}

fn tool_result_text(records: &[Record], call: &str) -> String {
    records
        .iter()
        .find_map(|record| match &record.event {
            Event::ToolResult {
                call_id, content, ..
            } if call_id == call => Some(hame_session::blocks_text(content)),
            _ => None,
        })
        .expect("a result for the call")
}

#[test]
fn an_action_dispatched_before_a_crash_is_reported_as_outcome_unknown_and_never_replayed() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let mut writer = store.create(options("s1")).unwrap();
    for event in open_turn_with_running_call() {
        writer.append(event).unwrap();
    }
    // The tool journalled an earlier action that completed, then dispatched
    // a commit, and the process died before it settled.
    let journal = Arc::clone(writer.journal());
    journal
        .record(&receipt("op-type", ActionStatus::NotStarted))
        .unwrap();
    journal
        .record(&receipt("op-type", ActionStatus::Completed))
        .unwrap();
    journal
        .record(&receipt("op-submit", ActionStatus::NotStarted))
        .unwrap();
    journal
        .record(&receipt("op-submit", ActionStatus::Dispatched))
        .unwrap();
    drop(journal);
    drop(writer);

    let (records, writer) = store.open(&id("s1")).unwrap();
    // Both settled receipts are in the log, the unsettled one as unknown.
    let logged = receipts_in(&records);
    let statuses: Vec<(&str, ActionStatus)> = logged
        .iter()
        .map(|r| (r.op_id.as_str(), r.status))
        .collect();
    assert_eq!(
        statuses,
        [
            ("op-type", ActionStatus::Completed),
            ("op-submit", ActionStatus::OutcomeUnknown)
        ]
    );
    // The journal says the same, so an adapter refuses to repeat it.
    let journal = ActionJournal::read(writer.journal().path()).unwrap();
    assert!(ActionJournal::unsettled(&journal).is_empty());
    assert_eq!(
        ActionJournal::prior_effect(&journal, "key-op-submit")
            .unwrap()
            .status,
        ActionStatus::OutcomeUnknown
    );
    // The interrupted call's result names both actions and asks for a
    // reconciliation.
    let text = tool_result_text(&records, "c1");
    assert!(text.starts_with(INTERRUPTED_RUNNING), "{text}");
    assert!(text.contains(INTERRUPTED_ACTIONS), "{text}");
    assert!(text.contains(
        "- browser.click on browser origin=https://shop.example gen=0 (op op-submit): outcome_unknown"
    ));
    assert!(text.contains("(op op-type): completed"));
    assert!(text.contains("reconcile"));
    assert!(plan_recovery(&records).is_empty());
    // The model sees the explanation; nothing was replayed.
    let history = derive_history(&records);
    assert!(history.dangling_tool_calls().is_empty());
    assert!(!records.iter().any(|r| matches!(
        &r.event,
        Event::ToolCall { call_id, .. } if call_id != "c1"
    )));
    drop(writer);

    // Reopening again adds nothing: each receipt is logged once.
    let (again, _writer) = store.open(&id("s1")).unwrap();
    assert_eq!(again, records);
}

#[test]
fn receipts_settled_after_the_last_look_are_taken_once() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let mut writer = store.create(options("s1")).unwrap();
    let journal = Arc::clone(writer.journal());
    journal
        .record(&receipt("a", ActionStatus::Dispatched))
        .unwrap();
    assert!(writer.take_settled_receipts().unwrap().is_empty());
    journal
        .record(&receipt("a", ActionStatus::Verified))
        .unwrap();
    journal.record(&receipt("b", ActionStatus::Failed)).unwrap();
    let taken: Vec<String> = writer
        .take_settled_receipts()
        .unwrap()
        .into_iter()
        .map(|r| r.op_id)
        .collect();
    assert_eq!(taken, ["a", "b"]);
    assert!(writer.take_settled_receipts().unwrap().is_empty());
}

#[test]
fn a_missing_or_corrupt_artifact_is_detected_and_history_still_derives() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let mut writer = store.create(options("s1")).unwrap();
    let artifacts = writer.artifacts().clone();
    let kept = artifacts.put(&png(1, 1), "image/png").unwrap();
    let lost = artifacts.put(&png(2, 2), "image/png").unwrap();
    let spoiled = artifacts
        .put(b"%PDF-1.4 report", "application/pdf")
        .unwrap();
    let image = |artifact: &ArtifactRef| Block::Image {
        artifact: artifact.clone(),
    };
    writer
        .append(Event::UserMessage {
            content: vec![Block::text("Compare"), image(&kept), image(&lost)],
        })
        .unwrap();
    writer
        .append(Event::AssistantMessage {
            content: vec![Block::text("Here is the report")],
            stop: StopReason::EndTurn,
            usage: Usage::default(),
        })
        .unwrap();
    writer
        .append(Event::UserMessage {
            content: vec![Block::Artifact {
                artifact: spoiled.clone(),
            }],
        })
        .unwrap();
    drop(writer);
    fs::remove_file(artifacts.path_of(&lost.sha256).unwrap()).unwrap();
    fs::write(artifacts.path_of(&spoiled.sha256).unwrap(), b"changed").unwrap();

    let problems = store.check_artifacts(&id("s1")).unwrap();
    assert_eq!(problems.len(), 2);
    assert!(matches!(&problems[0], (a, ArtifactError::Missing(_)) if a.sha256 == lost.sha256));
    assert!(
        matches!(&problems[1], (a, ArtifactError::Corrupt { .. }) if a.sha256 == spoiled.sha256)
    );

    // Resuming works and the history is what it was.
    let (records, _writer) = store.open(&id("s1")).unwrap();
    let history = derive_history(&records);
    assert_eq!(history.messages.len(), 3);
    assert_eq!(
        history.messages[0].content,
        vec![Block::text("Compare"), image(&kept), image(&lost)]
    );
}

fn set_age(path: &std::path::Path, age: Duration) {
    fs::File::options()
        .append(true)
        .open(path)
        .unwrap()
        .set_modified(SystemTime::now() - age)
        .unwrap();
}

#[test]
fn garbage_collection_removes_old_orphans_only() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let day = Duration::from_secs(24 * 3600);
    let mut writer = store.create(options("s1")).unwrap();
    let artifacts = writer.artifacts().clone();
    let referenced = artifacts.put(&png(1, 1), "image/png").unwrap();
    writer
        .append(Event::ToolResult {
            call_id: "c1".into(),
            content: vec![Block::Image {
                artifact: referenced.clone(),
            }],
            is_error: false,
        })
        .unwrap();
    // A crash after the write and before the event: an orphan.
    let orphan = artifacts.put(&png(9, 9), "image/png").unwrap();
    drop(writer);
    // Referenced only by a fork's log, or by a log this build cannot read.
    let forked = artifacts.put(&png(3, 3), "image/png").unwrap();
    let (_, mut fork) = store.fork(&id("s1"), 1, options("s2")).unwrap();
    fork.append(Event::UserMessage {
        content: vec![Block::Image {
            artifact: forked.clone(),
        }],
    })
    .unwrap();
    drop(fork);
    let future = artifacts.put(&png(4, 4), "image/png").unwrap();
    fs::create_dir_all(store.root().join("s9")).unwrap();
    fs::write(
        store.root().join("s9").join("session.v1.jsonl"),
        format!(
            "{{\"seq\":0,\"type\":\"session\",\"data\":{{\"version\":99}},\"x\":\"{}\"}}\n",
            future.sha256
        ),
    )
    .unwrap();
    // A temporary file left by a write that crashed.
    let shard = artifacts.path_of(&orphan.sha256).unwrap();
    let temp = shard
        .parent()
        .unwrap()
        .join(format!(".{}.abc.tmp", orphan.sha256));
    fs::write(&temp, b"partial").unwrap();

    // Everything is new: nothing is collected yet.
    let report = store.gc_artifacts(day).unwrap();
    assert_eq!(
        (report.removed, report.temp_removed, report.kept),
        (0, 0, 4)
    );
    assert!(artifacts.contains(&orphan.sha256));

    // Once old, the orphan and the temporary file go, nothing else.
    for artifact in [&referenced, &orphan, &forked, &future] {
        set_age(&artifacts.path_of(&artifact.sha256).unwrap(), 2 * day);
    }
    set_age(&temp, 2 * day);
    let dry = store.gc_artifacts_dry_run(day).unwrap();
    assert_eq!((dry.removed, dry.temp_removed), (1, 1));
    assert!(
        artifacts.contains(&orphan.sha256),
        "a dry run deletes nothing"
    );
    let report = store.gc_artifacts(day).unwrap();
    assert_eq!(
        (report.removed, report.temp_removed, report.kept),
        (1, 1, 3)
    );
    assert_eq!(report.bytes_freed, png(9, 9).len() as u64);
    assert!(!artifacts.contains(&orphan.sha256));
    assert!(!temp.exists());
    for artifact in [&referenced, &forked, &future] {
        assert!(artifacts.get(artifact).is_ok());
    }

    // Storing the same bytes again makes an old orphan new again, so it
    // survives until its session has had time to refer to it.
    let again = artifacts.put(&png(9, 9), "image/png").unwrap();
    set_age(&artifacts.path_of(&again.sha256).unwrap(), 2 * day);
    artifacts.put(&png(9, 9), "image/png").unwrap();
    assert_eq!(store.gc_artifacts(day).unwrap().removed, 0);
    assert!(artifacts.contains(&again.sha256));
}

#[test]
fn a_fork_shares_artifacts_and_keeps_every_receipt() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let mut writer = store.create(options("s1")).unwrap();
    let image = writer.artifacts().put(&png(5, 5), "image/png").unwrap();
    writer
        .append(Event::UserMessage {
            content: vec![Block::Image {
                artifact: image.clone(),
            }],
        })
        .unwrap();
    writer
        .journal()
        .record(&receipt("before", ActionStatus::Completed))
        .unwrap();
    for receipt in writer.take_settled_receipts().unwrap() {
        writer
            .append(Event::ActionReceipt {
                receipt: Box::new(receipt),
            })
            .unwrap();
    }
    let fork_at = writer.next_seq() - 1;
    // After the fork point: an effect the fork's history does not include,
    // but which happened all the same.
    writer
        .journal()
        .record(&receipt("after", ActionStatus::Verified))
        .unwrap();
    writer
        .journal()
        .record(&receipt("pending", ActionStatus::Dispatched))
        .unwrap();
    drop(writer);

    let (records, fork) = store.fork(&id("s1"), fork_at, options("s2")).unwrap();
    // The image is readable from the fork: the store is shared.
    let images = hame_session::session_artifacts(&records);
    assert_eq!(images, vec![image.clone()]);
    assert_eq!(fork.artifacts().get(&image).unwrap(), png(5, 5));
    assert_eq!(fork.artifacts().root(), store.artifacts().root());
    // Both settled receipts carried over, in the fork's log and journal;
    // the unsettled one is the source's to settle.
    let logged: Vec<String> = receipts_in(&records).into_iter().map(|r| r.op_id).collect();
    assert_eq!(logged, ["before", "after"]);
    let journal = ActionJournal::read(fork.journal().path()).unwrap();
    assert!(ActionJournal::prior_effect(&journal, "key-after").is_some());
    assert!(ActionJournal::prior_effect(&journal, "key-pending").is_none());
    drop(fork);
    // Reopening the fork does not log them twice.
    let (again, _writer) = store.open(&id("s2")).unwrap();
    assert_eq!(again, records);
}

fn tool_image(call: &str, artifact: &ArtifactRef) -> Vec<Event> {
    vec![
        Event::AssistantMessage {
            content: vec![Block::ToolUse {
                id: call.into(),
                name: "screenshot".into(),
                input: json!({}),
            }],
            stop: StopReason::ToolUse,
            usage: Usage::default(),
        },
        Event::ToolResult {
            call_id: call.into(),
            content: vec![
                Block::text(format!("shot {call}")),
                Block::Image {
                    artifact: artifact.clone(),
                },
            ],
            is_error: false,
        },
    ]
}

#[test]
fn only_recent_images_are_shown_and_masking_clears_images_too() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let mut writer = store.create(options("s1")).unwrap();
    let shots: Vec<ArtifactRef> = (1..=5)
        .map(|n| writer.artifacts().put(&png(n, n), "image/png").unwrap())
        .collect();
    writer
        .append(Event::UserMessage {
            content: vec![
                Block::text("Watch"),
                Block::Image {
                    artifact: shots[0].clone(),
                },
            ],
        })
        .unwrap();
    for (n, shot) in shots.iter().enumerate() {
        for event in tool_image(&format!("c{n}"), shot) {
            writer.append(event).unwrap();
        }
    }
    drop(writer);
    let records = store.read(&id("s1")).unwrap();

    let exact = hame_session::HistoryOptions {
        keep_images: 3,
        image_step: 1,
    };
    let results: Vec<Vec<Block>> = hame_session::derive_history_with(&records, exact)
        .messages
        .into_iter()
        .filter(|m| m.role == Role::Tool)
        .map(|m| match &m.content[0] {
            Block::ToolResult { content, .. } => content.clone(),
            _ => unreachable!(),
        })
        .collect();
    let shown: Vec<bool> = results
        .iter()
        .map(|content| matches!(content[1], Block::Image { .. }))
        .collect();
    assert_eq!(shown, [false, false, true, true, true]);
    // The older ones name their artifact, so the model can ask again.
    let note = results[0][1].as_text().unwrap();
    assert!(note.contains(&shots[0].sha256), "{note}");
    assert!(note.contains("image/png 1x1"));
    assert_eq!(results[0][0], Block::text("shot c0"));
    // A user's own image is not a tool observation and stays.
    let history = derive_history(&records);
    assert!(matches!(
        history.messages[0].content[1],
        Block::Image { .. }
    ));
    // Fewer to keep, fewer shown.
    let one = hame_session::derive_history_with(
        &records,
        hame_session::HistoryOptions {
            keep_images: 1,
            image_step: 1,
        },
    );
    let kept = one
        .messages
        .iter()
        .flat_map(|m| m.content.iter())
        .filter(|b| matches!(b, Block::ToolResult { content, .. } if content.iter().any(|c| matches!(c, Block::Image { .. }))))
        .count();
    assert_eq!(kept, 1);

    // By default old images go three at a time, so the start of the prompt
    // stays the same (and cached) while images 4 and 5 arrive: all five
    // are still shown, and the oldest three go together with the sixth.
    let shown_by_default = |records: &[hame_session::Record]| -> Vec<bool> {
        derive_history(records)
            .messages
            .into_iter()
            .filter(|m| m.role == Role::Tool)
            .map(|m| match &m.content[0] {
                Block::ToolResult { content, .. } => matches!(content[1], Block::Image { .. }),
                _ => unreachable!(),
            })
            .collect()
    };
    assert_eq!(shown_by_default(&records), [true; 5]);

    // Masking clears images up to its mark, short results included, and
    // leaves short text alone.
    let (_, mut writer) = store.open(&id("s1")).unwrap();
    let last_result = records
        .iter()
        .rev()
        .find(|r| matches!(r.event, Event::ToolResult { .. }))
        .unwrap()
        .seq;
    writer
        .append(Event::Masked {
            up_to_seq: last_result - 2,
        })
        .unwrap();
    drop(writer);
    let records = store.read(&id("s1")).unwrap();
    let history = derive_history(&records);
    let images: Vec<bool> = history
        .messages
        .iter()
        .filter(|m| m.role == Role::Tool)
        .map(|m| match &m.content[0] {
            Block::ToolResult { content, .. } => {
                assert!(content[0].as_text().unwrap().starts_with("shot"));
                assert!("shot c0".len() < MASK_MIN_BYTES);
                matches!(content[1], Block::Image { .. })
            }
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(images, [false, false, false, false, true]);
}

#[test]
fn a_cancelled_turn_settles_abandoned_actions_like_a_crash() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let mut writer = store.create(options("s1")).unwrap();
    for event in open_turn_with_running_call() {
        writer.append(event).unwrap();
    }
    writer
        .journal()
        .record(&receipt("op-submit", ActionStatus::Dispatched))
        .unwrap();
    let settled = writer.settle_abandoned_actions().unwrap();
    assert_eq!(settled.len(), 1);
    assert_eq!(settled[0].status, ActionStatus::OutcomeUnknown);
    for receipt in settled {
        writer
            .append(Event::ActionReceipt {
                receipt: Box::new(receipt),
            })
            .unwrap();
    }
    let records = store.read(&id("s1")).unwrap();
    let plan = hame_session::plan_close(&records, TurnEndReason::Cancelled);
    let Event::ToolResult { content, .. } = &plan[0] else {
        panic!("the running call is answered first: {plan:?}");
    };
    assert!(hame_session::blocks_text(content).contains("(op op-submit): outcome_unknown"));
}
