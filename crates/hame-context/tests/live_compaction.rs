//! Checks against the real Anthropic API that a summary written by the
//! session's own model is served from the conversation's cache. Costs two
//! small requests, so it runs only when asked:
//!
//! HAME_LIVE_ANTHROPIC=1 ANTHROPIC_API_KEY=... (or CLAUDE_API_KEY) cargo test -p hame-context --test live_compaction

use std::sync::Arc;

use hame_agent::{Session, TurnHook};
use hame_context::Compactor;
use hame_llm::{Anthropic, Llm, Request, SinkError};
use hame_session::{Block, CreateOptions, Delta, Event, SessionStore, StopReason, ToolSpec, Usage};
use serde_json::json;

#[tokio::test]
async fn a_summary_by_the_session_model_is_read_from_the_cache() {
    if std::env::var("HAME_LIVE_ANTHROPIC").is_err() {
        return;
    }
    let key = std::env::var("ANTHROPIC_API_KEY")
        .or_else(|_| std::env::var("CLAUDE_API_KEY"))
        .expect("ANTHROPIC_API_KEY or CLAUDE_API_KEY");
    let llm: Arc<dyn Llm> = Arc::new(Anthropic::new(
        "https://api.anthropic.com",
        key,
        "claude-haiku-5-5",
    ));

    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new(dir.path());
    let writer = store.create(CreateOptions::default()).unwrap();
    let records = store.read(writer.id()).unwrap();
    let mut session = Session::new(records, writer);
    session.append(Event::TurnStart { turn: 1 }).unwrap();
    session
        .append(Event::SystemMessage {
            text: "You are a coding agent.".into(),
        })
        .unwrap();
    session
        .append(Event::ToolsDeclared {
            tools: vec![ToolSpec {
                name: "read".into(),
                description: "Read a file.".into(),
                input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
            }],
        })
        .unwrap();
    session
        .append(Event::UserMessage {
            content: vec![Block::text(
                "Read the five modules and find the bug in parse().",
            )],
        })
        .unwrap();
    for step in 1..=6u64 {
        session.append(Event::StepStart { turn: 1, step }).unwrap();
        let id = format!("c{step}");
        session
            .append(Event::AssistantMessage {
                content: vec![Block::ToolUse {
                    id: id.clone(),
                    name: "read".into(),
                    input: json!({"path": format!("m{step}.py")}),
                }],
                stop: StopReason::ToolUse,
                usage: Usage::default(),
            })
            .unwrap();
        let body: String = (0..400)
            .map(|line| format!("def f{step}_{line}(x):\n    return x + {line}\n"))
            .collect();
        session
            .append(Event::ToolResult {
                call_id: id,
                content: vec![Block::text(body)],
                is_error: false,
            })
            .unwrap();
        session.append(Event::StepEnd { turn: 1, step }).unwrap();
    }
    session
        .append(Event::StepStart { turn: 1, step: 7 })
        .unwrap();

    // The agent's own last request, which writes the cache entry.
    let mut request = Request::from(session.history());
    request.cache_key = Some(session.id().to_string());
    let mut ignore = |_: Delta| -> Result<(), SinkError> { Ok(()) };
    let warm = llm.stream(&request, &mut ignore).await.unwrap();
    session
        .append(Event::AssistantMessage {
            content: vec![Block::text("Still reading.")],
            stop: StopReason::EndTurn,
            usage: warm.usage,
        })
        .unwrap();
    session
        .append(Event::StepStart { turn: 1, step: 8 })
        .unwrap();

    let compactor = Compactor {
        llm,
        threshold_tokens: 20_000,
        clear_tool_results: false,
        reuse_prefix: true,
    };
    compactor.before_step(&mut session).await.unwrap();
    let usage = session
        .records()
        .iter()
        .find_map(|record| match &record.event {
            Event::Compaction { usage, .. } => Some(*usage),
            _ => None,
        })
        .expect("a compaction");
    eprintln!("warm request: {:?}\nsummary: {usage:?}", warm.usage);
    assert!(
        usage.cache_read_tokens > usage.input_tokens,
        "the summary should be read mostly from the cache: {usage:?}"
    );
}
