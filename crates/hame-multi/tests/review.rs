use std::path::Path;
use std::sync::{Arc, Mutex};

use hame_agent::{Agent, Session, ToolPipeline};
use hame_eval::{Recording, ScriptedLlm, ScriptedResponse};
use hame_multi::{Child, ChildFactory, REVIEW_PREFIX, Reviewer};
use hame_session::{
    Block, CreateOptions, Delta, Event, SessionId, SessionStore, StopReason, Usage,
};
use hame_tools::{InProcess, ToolContext, ToolSet};
use serde_json::json;

fn agent(responses: Vec<ScriptedResponse>, cwd: &Path) -> Agent {
    Agent {
        llm: Arc::new(ScriptedLlm::new(Recording { responses })),
        tools: ToolSet::builtin(),
        pipeline: Arc::new(ToolPipeline::new()),
        tool_context: ToolContext::new(cwd),
        runner: Arc::new(InProcess),
        checkpointer: None,
        hooks: Vec::new(),
        system: "You are hame.".into(),
        max_steps: 8,
        max_tokens: None,
        max_tool_calls: 32,
        fuse_actions: false,
        dedupe_results: false,
        deferred: ToolSet::new(),
    }
}

fn write_call() -> ScriptedResponse {
    ScriptedResponse {
        deltas: vec![
            Delta::ToolUseStart {
                id: "w1".into(),
                name: "write".into(),
            },
            Delta::ToolInput {
                json: json!({"path": "cal.py", "content": "def f():\n    return 1\n"}).to_string(),
            },
        ],
        stop: StopReason::ToolUse,
        usage: Usage::default(),
    }
}

/// A reviewer that always answers `report`, and remembers what it was asked.
fn reviewer(report: &'static str, cwd: &Path, asked: Arc<Mutex<Vec<String>>>) -> Reviewer {
    let factory: ChildFactory = Arc::new(move |spec: hame_multi::ChildSpec| {
        let asked = Arc::clone(&asked);
        Box::pin(async move {
            asked.lock().unwrap().push(spec.cwd.display().to_string());
            Ok(Child {
                agent: agent(vec![ScriptedResponse::text(report)], &spec.cwd),
                keep_alive: Box::new(()),
            })
        })
    });
    Reviewer {
        factory,
        store: SessionStore::new(cwd.join(".sessions")),
        cwd: cwd.to_path_buf(),
        depth: 0,
    }
}

fn session(root: &Path) -> Session {
    let store = SessionStore::new(root);
    let writer = store
        .create(CreateOptions {
            id: Some(SessionId::parse("main").unwrap()),
            cwd: root.display().to_string(),
        })
        .unwrap();
    Session::new(
        store.read(&SessionId::parse("main").unwrap()).unwrap(),
        writer,
    )
}

fn user_texts(session: &Session) -> Vec<String> {
    session
        .records()
        .iter()
        .filter_map(|record| match &record.event {
            Event::UserMessage { content } => match content.first() {
                Some(Block::Text { text }) => Some(text.clone()),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn findings_go_back_to_the_agent_once() {
    let work = tempfile::tempdir().unwrap();
    let logs = tempfile::tempdir().unwrap();
    let asked = Arc::new(Mutex::new(Vec::new()));
    let mut main = agent(
        vec![
            write_call(),
            ScriptedResponse::text("Done."),
            // Answers the findings without changing anything more.
            ScriptedResponse::text("Rule 2 was misread; noted."),
        ],
        work.path(),
    );
    main.hooks.push(Arc::new(reviewer(
        "Rule 2: f() returns 1, expected 2.",
        work.path(),
        Arc::clone(&asked),
    )));
    let mut session = session(logs.path());
    main.run_turn(&mut session, vec![Block::text("Implement SPEC.md.")])
        .await
        .unwrap();

    let texts = user_texts(&session);
    let reviews: Vec<&String> = texts
        .iter()
        .filter(|t| t.starts_with(REVIEW_PREFIX))
        .collect();
    assert_eq!(reviews.len(), 1, "{texts:?}");
    assert!(reviews[0].contains("f() returns 1, expected 2"));
    assert_eq!(asked.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_clean_review_lets_the_turn_end() {
    let work = tempfile::tempdir().unwrap();
    let logs = tempfile::tempdir().unwrap();
    let asked = Arc::new(Mutex::new(Vec::new()));
    let mut main = agent(
        vec![write_call(), ScriptedResponse::text("Done.")],
        work.path(),
    );
    main.hooks.push(Arc::new(reviewer(
        "I probed rules 1 to 5.\n\nNO FINDINGS",
        work.path(),
        Arc::clone(&asked),
    )));
    let mut session = session(logs.path());
    main.run_turn(&mut session, vec![Block::text("Implement SPEC.md.")])
        .await
        .unwrap();
    assert!(
        !user_texts(&session)
            .iter()
            .any(|t| t.starts_with(REVIEW_PREFIX))
    );
    assert_eq!(asked.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_turn_that_changed_nothing_is_not_reviewed() {
    let work = tempfile::tempdir().unwrap();
    let logs = tempfile::tempdir().unwrap();
    let asked = Arc::new(Mutex::new(Vec::new()));
    let mut main = agent(vec![ScriptedResponse::text("It does X.")], work.path());
    main.hooks.push(Arc::new(reviewer(
        "Rule 1 broken",
        work.path(),
        Arc::clone(&asked),
    )));
    let mut session = session(logs.path());
    main.run_turn(&mut session, vec![Block::text("What does it do?")])
        .await
        .unwrap();
    assert!(asked.lock().unwrap().is_empty());
}
