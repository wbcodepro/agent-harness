use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::json;
use zcode_session::{
    AttemptReason, Block, Clock, CreateOptions, Delta, Event, INTERRUPTED_NOT_STARTED,
    INTERRUPTED_RUNNING, Role, SessionError, SessionId, SessionStore, StopReason, TurnEndReason,
    Usage, derive_history, plan_recovery,
};

/// Ticks one millisecond per reading.
struct TickClock(AtomicU64);

impl Clock for TickClock {
    fn now_ms(&self) -> u64 {
        self.0.fetch_add(1, Ordering::Relaxed)
    }
}

fn store(root: &std::path::Path) -> SessionStore {
    SessionStore::with_clock(root, Arc::new(TickClock(AtomicU64::new(1000))))
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

fn text_deltas(parts: &[&str]) -> Vec<Event> {
    parts
        .iter()
        .map(|part| Event::AssistantDelta {
            delta: Delta::Text {
                text: (*part).into(),
            },
        })
        .collect()
}

/// One turn, two steps: the model calls two tools, then answers.
fn scenario() -> Vec<Event> {
    let tool_use = |id: &str, path: &str| Block::ToolUse {
        id: id.into(),
        name: "read".into(),
        input: json!({ "path": path }),
    };
    let mut events = vec![
        Event::SystemMessage {
            text: "You are zcode.".into(),
        },
        Event::TurnStart { turn: 1 },
        Event::StepStart { turn: 1, step: 1 },
        Event::UserMessage {
            content: vec![Block::text("What is in a.rs and b.rs?")],
        },
    ];
    events.extend(text_deltas(&["Let me ", "look."]));
    events.extend([
        Event::AssistantDelta {
            delta: Delta::ToolUseStart {
                id: "c1".into(),
                name: "read".into(),
            },
        },
        Event::AssistantDelta {
            delta: Delta::ToolInput {
                json: "{\"path\":\"a.rs\"}".into(),
            },
        },
        Event::AssistantMessage {
            content: vec![
                Block::text("Let me look."),
                tool_use("c1", "a.rs"),
                tool_use("c2", "b.rs"),
            ],
            stop: StopReason::ToolUse,
            usage: Usage {
                input_tokens: 40,
                output_tokens: 12,
                ..Usage::default()
            },
        },
        Event::ToolCall {
            call_id: "c1".into(),
            name: "read".into(),
            input: json!({ "path": "a.rs" }),
        },
        Event::ToolResult {
            call_id: "c1".into(),
            content: vec![Block::text("fn a() {}")],
            is_error: false,
        },
        Event::ToolCall {
            call_id: "c2".into(),
            name: "read".into(),
            input: json!({ "path": "b.rs" }),
        },
        Event::ToolResult {
            call_id: "c2".into(),
            content: vec![Block::text("fn b() {}")],
            is_error: false,
        },
        Event::StepEnd { turn: 1, step: 1 },
        Event::StepStart { turn: 1, step: 2 },
    ]);
    events.extend(text_deltas(&["Two ", "functions."]));
    events.extend([
        Event::AssistantMessage {
            content: vec![Block::text("Two functions.")],
            stop: StopReason::EndTurn,
            usage: Usage::default(),
        },
        Event::StepEnd { turn: 1, step: 2 },
        Event::TurnEnd {
            turn: 1,
            reason: TurnEndReason::Completed,
        },
    ]);
    events
}

fn write_scenario(store: &SessionStore, raw_id: &str) {
    let mut writer = store.create(options(raw_id)).unwrap();
    for event in scenario() {
        writer.append(event).unwrap();
    }
}

#[test]
fn a_written_session_reads_back_and_projects_model_history() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    write_scenario(&store, "s1");

    let records = store.read(&id("s1")).unwrap();
    assert_eq!(records.len(), scenario().len() + 1);
    assert!(records.iter().enumerate().all(|(i, r)| r.seq == i as u64));
    assert!(records.windows(2).all(|pair| pair[0].ts < pair[1].ts));
    assert_eq!(
        records[1..]
            .iter()
            .map(|r| r.event.clone())
            .collect::<Vec<_>>(),
        scenario()
    );

    let history = derive_history(&records);
    assert_eq!(history.system.as_deref(), Some("You are zcode."));
    assert_eq!(
        history.messages.iter().map(|m| m.role).collect::<Vec<_>>(),
        vec![Role::User, Role::Assistant, Role::Tool, Role::Assistant]
    );
    // Both results travel in one tool message; deltas are not model history.
    assert_eq!(history.messages[2].content.len(), 2);
    assert!(history.dangling_tool_calls().is_empty());
    assert!(plan_recovery(&records).is_empty());
}

#[test]
fn a_log_cut_at_any_byte_reopens_as_a_well_formed_session() {
    let dir = tempfile::tempdir().unwrap();
    let source = store(&dir.path().join("source"));
    write_scenario(&source, "s1");
    let full = fs::read(dir.path().join("source/s1/session.v1.jsonl")).unwrap();
    let header_len = full.iter().position(|b| *b == b'\n').unwrap() + 1;
    let complete = source.read(&id("s1")).unwrap();

    for cut in 0..=full.len() {
        let root = dir.path().join(format!("cut-{cut}"));
        fs::create_dir_all(root.join("s1")).unwrap();
        let path = root.join("s1/session.v1.jsonl");
        fs::write(&path, &full[..cut]).unwrap();
        let store = store(&root);

        if cut < header_len {
            assert!(
                matches!(store.open(&id("s1")), Err(SessionError::Corrupt { .. })),
                "cut {cut}: a log without a complete header must not open"
            );
            continue;
        }

        let (records, writer) = store.open(&id("s1")).unwrap();
        drop(writer);

        // Every record that was completely on disk survives, in order.
        let surviving = full[..cut].iter().filter(|b| **b == b'\n').count();
        assert_eq!(records[..surviving], complete[..surviving], "cut {cut}");

        // Nothing is left open and the model is owed nothing.
        assert!(plan_recovery(&records).is_empty(), "cut {cut}");
        let history = derive_history(&records);
        assert!(history.dangling_tool_calls().is_empty(), "cut {cut}");

        // What open returned is exactly what is now on disk, and a second
        // open changes nothing.
        assert_eq!(store.read(&id("s1")).unwrap(), records, "cut {cut}");
        let (again, _writer) = store.open(&id("s1")).unwrap();
        assert_eq!(again, records, "cut {cut}");
    }
}

#[test]
fn recovery_says_which_tool_calls_had_started() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let mut writer = store.create(options("s1")).unwrap();
    // Stop right after the first tool call started: c1 running, c2 not yet.
    let events = scenario();
    let stop = events
        .iter()
        .position(|e| matches!(e, Event::ToolCall { .. }))
        .unwrap();
    for event in &events[..=stop] {
        writer.append(event.clone()).unwrap();
    }
    drop(writer);

    let (records, _writer) = store.open(&id("s1")).unwrap();
    let recovered: Vec<Event> = records[stop + 2..]
        .iter()
        .map(|r| r.event.clone())
        .collect();
    let interrupted = |call_id: &str, text: &str| Event::ToolResult {
        call_id: call_id.into(),
        content: vec![Block::text(text)],
        is_error: true,
    };
    assert_eq!(
        recovered,
        vec![
            interrupted("c1", INTERRUPTED_RUNNING),
            interrupted("c2", INTERRUPTED_NOT_STARTED),
            Event::StepEnd { turn: 1, step: 1 },
            Event::TurnEnd {
                turn: 1,
                reason: TurnEndReason::Interrupted
            },
        ]
    );
}

#[test]
fn a_stream_cut_mid_response_is_kept_as_an_attempt_outside_history() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let mut writer = store.create(options("s1")).unwrap();
    writer.append(Event::TurnStart { turn: 1 }).unwrap();
    writer
        .append(Event::StepStart { turn: 1, step: 1 })
        .unwrap();
    writer
        .append(Event::UserMessage {
            content: vec![Block::text("hi")],
        })
        .unwrap();
    for event in text_deltas(&["Hel", "lo th"]) {
        writer.append(event).unwrap();
    }
    drop(writer);

    let (records, _writer) = store.open(&id("s1")).unwrap();
    assert!(records.iter().any(|r| r.event
        == Event::AssistantAttempt {
            content: vec![Block::text("Hello th")],
            reason: AttemptReason::Interrupted,
        }));
    let history = derive_history(&records);
    assert_eq!(history.messages.len(), 1);
    assert_eq!(history.messages[0].role, Role::User);
}

#[test]
fn only_one_writer_can_hold_a_session() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let writer = store.create(options("s1")).unwrap();

    assert!(matches!(
        store.open(&id("s1")),
        Err(SessionError::Locked(_))
    ));
    // Readers are never blocked.
    assert_eq!(store.read(&id("s1")).unwrap().len(), 1);

    drop(writer);
    assert!(store.open(&id("s1")).is_ok());
    assert!(matches!(
        store.create(options("s1")),
        Err(SessionError::AlreadyExists(_))
    ));
    assert!(matches!(
        store.open(&id("missing")),
        Err(SessionError::NotFound(_))
    ));
}

#[test]
fn corruption_before_the_tail_is_reported_not_repaired() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    write_scenario(&store, "s1");
    let path = dir.path().join("s1/session.v1.jsonl");
    let original = fs::read_to_string(&path).unwrap();

    let mut lines: Vec<&str> = original.lines().collect();
    lines[3] = "{not json}";
    fs::write(&path, lines.join("\n") + "\n").unwrap();
    assert!(matches!(
        store.open(&id("s1")),
        Err(SessionError::Corrupt { line: 4, .. })
    ));

    let mut lines: Vec<&str> = original.lines().collect();
    lines.remove(3);
    fs::write(&path, lines.join("\n") + "\n").unwrap();
    assert!(matches!(
        store.read(&id("s1")),
        Err(SessionError::Corrupt { line: 4, .. })
    ));

    let newer = original.replacen("\"version\":1", "\"version\":2", 1);
    fs::write(&path, newer).unwrap();
    assert!(matches!(
        store.read(&id("s1")),
        Err(SessionError::UnsupportedVersion {
            found: 2,
            supported: 1
        })
    ));
    assert!(store.list().unwrap().is_empty());
}

#[test]
fn compaction_hides_covered_messages_without_touching_the_log() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    write_scenario(&store, "s1");
    let (before, mut writer) = store.open(&id("s1")).unwrap();
    let last_seq = before.last().unwrap().seq;

    assert!(matches!(
        writer.append(Event::Compaction {
            summary: "too far".into(),
            up_to_seq: last_seq + 1,
        }),
        Err(SessionError::InvalidEvent(_))
    ));
    writer
        .append(Event::Compaction {
            summary: "Earlier: read a.rs and b.rs; each has one function.".into(),
            up_to_seq: last_seq,
        })
        .unwrap();
    writer.append(Event::TurnStart { turn: 2 }).unwrap();
    writer
        .append(Event::UserMessage {
            content: vec![Block::text("And c.rs?")],
        })
        .unwrap();
    writer
        .append(Event::TurnEnd {
            turn: 2,
            reason: TurnEndReason::Cancelled,
        })
        .unwrap();
    drop(writer);

    let after = store.read(&id("s1")).unwrap();
    assert_eq!(after[..before.len()], before[..]);

    let history = derive_history(&after);
    assert_eq!(history.system.as_deref(), Some("You are zcode."));
    assert_eq!(
        history.messages.iter().map(|m| m.role).collect::<Vec<_>>(),
        vec![Role::User, Role::User]
    );
    assert_eq!(
        history.messages[0].content,
        vec![Block::text(
            "Earlier: read a.rs and b.rs; each has one function."
        )]
    );
}

#[test]
fn a_fork_copies_a_prefix_and_closes_a_turn_cut_in_the_middle() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    write_scenario(&store, "s1");
    let source = store.read(&id("s1")).unwrap();
    // Fork right after the assistant asked for two tools.
    let at_seq = source
        .iter()
        .find(|r| matches!(r.event, Event::AssistantMessage { .. }))
        .unwrap()
        .seq;

    let (fork, writer) = store.fork(&id("s1"), at_seq, options("s2")).unwrap();
    drop(writer);

    let Event::Session(header) = &fork[0].event else {
        panic!("fork must start with a header");
    };
    let parent = header.parent.as_ref().unwrap();
    assert_eq!((parent.session.as_str(), parent.seq), ("s1", at_seq));
    assert_eq!(fork[1..=at_seq as usize], source[1..=at_seq as usize]);
    assert!(plan_recovery(&fork).is_empty());
    assert!(derive_history(&fork).dangling_tool_calls().is_empty());
    assert_eq!(store.read(&id("s2")).unwrap(), fork);

    // The source is untouched, and both sessions are listed.
    assert_eq!(store.read(&id("s1")).unwrap(), source);
    let listed: Vec<String> = store
        .list()
        .unwrap()
        .into_iter()
        .map(|h| h.id.to_string())
        .collect();
    assert_eq!(listed, vec!["s1", "s2"]);

    assert!(matches!(
        store.fork(&id("s1"), 10_000, options("s3")),
        Err(SessionError::ForkOutOfRange { .. })
    ));
}
