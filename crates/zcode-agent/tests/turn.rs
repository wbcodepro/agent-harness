use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use zcode_agent::{Agent, AgentError, Session, Steering, ToolPipeline, ToolRequest};
use zcode_eval::{Recording, ScriptedLlm, ScriptedResponse};
use zcode_kernel::{BoxFuture, Layer, Next};
use zcode_llm::{Completion, DeltaSink, Llm, LlmError, Request};
use zcode_session::{
    AttemptReason, Block, Clock, CreateOptions, Delta, Event, Role, SessionId, SessionStore,
    StopReason, TurnEndReason, Usage, plan_recovery,
};
use zcode_tools::{Effect, InProcess, ToolContext, ToolOutput, ToolSet};

struct TickClock(AtomicU64);

impl Clock for TickClock {
    fn now_ms(&self) -> u64 {
        self.0.fetch_add(1, Ordering::Relaxed)
    }
}

/// Wraps a model and remembers every request it was sent.
struct Spy<L> {
    inner: L,
    requests: Mutex<Vec<Request>>,
}

impl<L: Llm> Llm for Spy<L> {
    fn stream<'a>(
        &'a self,
        request: &'a Request,
        sink: &'a mut dyn DeltaSink,
    ) -> BoxFuture<'a, Result<Completion, LlmError>> {
        self.requests.lock().unwrap().push(request.clone());
        self.inner.stream(request, sink)
    }
}

fn tool_call(id: &str, name: &str, input: Value) -> ScriptedResponse {
    ScriptedResponse {
        deltas: vec![
            Delta::ToolUseStart {
                id: id.into(),
                name: name.into(),
            },
            Delta::ToolInput {
                json: input.to_string(),
            },
        ],
        stop: StopReason::ToolUse,
        usage: Usage::default(),
    }
}

fn agent(llm: Arc<dyn Llm>, workspace: &Path) -> Agent {
    Agent {
        llm,
        tools: ToolSet::builtin(),
        pipeline: Arc::new(ToolPipeline::new()),
        tool_context: ToolContext::new(workspace),
        runner: Arc::new(InProcess),
        checkpointer: None,
        hooks: Vec::new(),
        system: "You are zcode.".into(),
        max_steps: 8,
        max_tokens: None,
        max_tool_calls: 32,
        fuse_actions: true,
        deferred: ToolSet::new(),
    }
}

fn new_session(root: &Path) -> Session {
    let store = SessionStore::with_clock(root, Arc::new(TickClock(AtomicU64::new(0))));
    let writer = store
        .create(CreateOptions {
            id: Some(SessionId::parse("s1").unwrap()),
            cwd: "/work".into(),
        })
        .unwrap();
    let records = store.read(writer.id()).unwrap();
    Session::new(records, writer)
}

fn user(text: &str) -> Vec<Block> {
    vec![Block::text(text)]
}

fn kinds(session: &Session) -> Vec<&'static str> {
    session
        .records()
        .iter()
        .map(|record| match record.event {
            Event::Session(_) => "session",
            Event::TurnStart { .. } => "turn/start",
            Event::TurnEnd { .. } => "turn/end",
            Event::StepStart { .. } => "step/start",
            Event::StepEnd { .. } => "step/end",
            Event::SystemMessage { .. } => "system",
            Event::ToolsDeclared { .. } => "tools",
            Event::UserMessage { .. } => "user",
            Event::AssistantDelta { .. } => "delta",
            Event::AssistantMessage { .. } => "assistant",
            Event::AssistantAttempt { .. } => "attempt",
            Event::ToolCall { .. } => "tool/call",
            Event::ToolResult { .. } => "tool/result",
            Event::Compaction { .. } => "compaction",
            Event::Checkpoint { .. } => "checkpoint",
            Event::Masked { .. } => "masked",
            Event::ModelSwitched { .. } => "model",
        })
        .collect()
}

fn last_tool_result(session: &Session) -> (String, bool) {
    session
        .records()
        .iter()
        .rev()
        .find_map(|record| match &record.event {
            Event::ToolResult {
                content, is_error, ..
            } => match &content[0] {
                Block::Text { text } => Some((text.clone(), *is_error)),
                _ => None,
            },
            _ => None,
        })
        .expect("a tool result was logged")
}

#[tokio::test]
async fn a_turn_runs_tools_and_feeds_their_results_back_to_the_model() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("work");
    fs::create_dir(&workspace).unwrap();
    let spy = Arc::new(Spy {
        inner: ScriptedLlm::new(Recording {
            responses: vec![
                tool_call("c1", "write", json!({"path": "a.txt", "content": "hello"})),
                ScriptedResponse::text("Created a.txt."),
                ScriptedResponse::text("You're welcome."),
            ],
        }),
        requests: Mutex::new(Vec::new()),
    });
    let agent = agent(spy.clone(), &workspace);
    let mut session = new_session(&dir.path().join("sessions"));

    agent
        .run_turn(&mut session, user("Make a.txt"))
        .await
        .unwrap();

    assert_eq!(
        fs::read_to_string(workspace.join("a.txt")).unwrap(),
        "hello"
    );
    assert_eq!(
        kinds(&session),
        [
            "session",
            "turn/start",
            "system",
            "tools",
            "user",
            "step/start",
            "delta",
            "delta",
            "assistant",
            "tool/call",
            "tool/result",
            "step/end",
            "step/start",
            "delta",
            "assistant",
            "step/end",
            "turn/end",
        ]
    );

    agent.run_turn(&mut session, user("Thanks")).await.unwrap();

    // The prompt and tool list are declared once, not per turn.
    assert_eq!(
        kinds(&session).iter().filter(|k| **k == "system").count(),
        1
    );
    assert_eq!(kinds(&session).iter().filter(|k| **k == "tools").count(), 1);

    let requests = spy.requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0].system.as_deref(), Some("You are zcode."));
    assert_eq!(requests[0].tools.len(), 7);
    assert_eq!(requests[0].messages.len(), 1);
    // The second request carries the call and its result.
    let roles: Vec<Role> = requests[1].messages.iter().map(|m| m.role).collect();
    assert_eq!(roles, [Role::User, Role::Assistant, Role::Tool]);
    assert!(matches!(
        &requests[1].messages[2].content[0],
        Block::ToolResult { call_id, is_error: false, .. } if call_id == "c1"
    ));
    assert_eq!(requests[2].messages.len(), 5);
    assert!(plan_recovery(session.records()).is_empty());
}

struct DenyWrites;

impl Layer<ToolRequest, ToolOutput> for DenyWrites {
    fn call<'a>(
        &'a self,
        request: ToolRequest,
        next: Next<'a, ToolRequest, ToolOutput>,
    ) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            if request.effect == Effect::ReadOnly {
                next.run(request).await
            } else {
                ToolOutput::error(format!("Denied by policy: {}", request.name))
            }
        })
    }
}

#[tokio::test]
async fn a_pipeline_layer_can_deny_a_tool_and_the_model_is_told() {
    let dir = tempfile::tempdir().unwrap();
    let llm = Arc::new(ScriptedLlm::new(Recording {
        responses: vec![
            tool_call("c1", "write", json!({"path": "a.txt", "content": "x"})),
            tool_call("c2", "nonexistent", json!({})),
            ScriptedResponse::text("I could not."),
        ],
    }));
    let agent = agent(llm, dir.path());
    agent.pipeline.push(DenyWrites);
    let mut session = new_session(&dir.path().join("sessions"));

    agent
        .run_turn(&mut session, user("Make a.txt"))
        .await
        .unwrap();

    assert!(!dir.path().join("a.txt").exists());
    let results: Vec<(String, bool)> = session
        .records()
        .iter()
        .filter_map(|record| match &record.event {
            Event::ToolResult {
                content, is_error, ..
            } => Some((format!("{:?}", content[0]), *is_error)),
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 2);
    assert!(results[0].0.contains("Denied by policy: write") && results[0].1);
    assert!(results[1].0.contains("no tool named") && results[1].1);
}

#[tokio::test]
async fn deferred_tools_are_found_by_search_and_run_under_their_own_names() {
    let dir = tempfile::tempdir().unwrap();
    let llm = Arc::new(ScriptedLlm::new(Recording {
        responses: vec![
            tool_call("c1", "tool_search", json!({"query": "create a file"})),
            tool_call(
                "c2",
                "tool_call",
                json!({"name": "write", "input": {"path": "a.txt", "content": "x"}}),
            ),
            tool_call(
                "c3",
                "tool_call",
                json!({"name": "read", "input": {"path": "a.txt"}}),
            ),
            tool_call("c4", "tool_call", json!({"name": "launch", "input": {}})),
            ScriptedResponse::text("Done."),
        ],
    }));
    let mut agent = agent(llm, dir.path());
    // Everything but bash is deferred; only bash and the two entry points
    // are declared to the model.
    agent.deferred = ToolSet::builtin();
    agent.deferred.retain(|tool| tool.spec().name != "bash");
    agent.tools.retain(|tool| tool.spec().name == "bash");
    zcode_agent::deferred::declare_deferred(&mut agent.tools, &agent.deferred);
    agent.pipeline.push(DenyWrites);
    let mut session = new_session(&dir.path().join("sessions"));

    agent
        .run_turn(&mut session, user("Make a.txt"))
        .await
        .unwrap();

    let declared: Vec<String> = session
        .history()
        .tools
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert_eq!(declared, ["bash", "tool_search", "tool_call"]);
    let results: Vec<(String, bool)> = session
        .records()
        .iter()
        .filter_map(|record| match &record.event {
            Event::ToolResult {
                content, is_error, ..
            } => match &content[0] {
                Block::Text { text } => Some((text.clone(), *is_error)),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert!(
        results[0].0.starts_with("write\nCreate a file"),
        "{}",
        results[0].0
    );
    assert!(results[0].0.contains("Input schema: {"));
    // Policy saw the real tool, not the wrapper: the write is refused by
    // name, and the read, which changes nothing, goes through.
    assert_eq!(results[1], ("Denied by policy: write".to_string(), true));
    assert!(
        results[2].0.contains("does not exist")
            || results[2].0.contains("No such file")
            || results[2].1,
        "{}",
        results[2].0
    );
    assert!(
        results[3].0.contains("no tool named \"launch\"") && results[3].1,
        "{}",
        results[3].0
    );
}

#[tokio::test]
async fn the_model_is_warned_a_few_requests_before_the_turn_runs_out() {
    let dir = tempfile::tempdir().unwrap();
    let mut responses: Vec<ScriptedResponse> = (1..=6)
        .map(|n| tool_call(&format!("c{n}"), "glob", json!({"pattern": "*"})))
        .collect();
    responses.push(ScriptedResponse::text("Listed; nothing else done."));
    let llm = Arc::new(ScriptedLlm::new(Recording { responses }));
    let agent = agent(llm, dir.path()); // eight steps at most
    let mut session = new_session(&dir.path().join("sessions"));

    agent
        .run_turn(&mut session, user("Look around"))
        .await
        .unwrap();

    let warned_after: Vec<usize> = session
        .history()
        .messages
        .iter()
        .enumerate()
        .filter(|(_, message)| {
            matches!(&message.content[0], Block::Text { text } if text.starts_with(zcode_agent::STEPS_LEFT_PREFIX))
        })
        .map(|(index, _)| index)
        .collect();
    // Once, after the fifth step's result: user, then five call/result
    // pairs, then the warning.
    assert_eq!(warned_after, [11]);
}

/// Has the user say something while the first tool call is running.
struct Interject(Steering);

impl Layer<ToolRequest, ToolOutput> for Interject {
    fn call<'a>(
        &'a self,
        request: ToolRequest,
        next: Next<'a, ToolRequest, ToolOutput>,
    ) -> BoxFuture<'a, ToolOutput> {
        if request.call_id == "c1" {
            self.0.push("Also add a README.");
        }
        next.run(request)
    }
}

#[tokio::test]
async fn a_message_sent_mid_turn_reaches_the_model_at_the_next_step() {
    let dir = tempfile::tempdir().unwrap();
    let llm = Arc::new(ScriptedLlm::new(Recording {
        responses: vec![
            tool_call("c1", "write", json!({"path": "a.txt", "content": "x"})),
            ScriptedResponse::text("Done, with a README."),
            ScriptedResponse::text("Stopped as asked."),
        ],
    }));
    let steering = Steering::default();
    let mut agent = agent(llm, dir.path());
    agent.hooks.push(Arc::new(steering.clone()));
    agent.pipeline.push(Interject(steering.clone()));
    let mut session = new_session(&dir.path().join("sessions"));

    agent
        .run_turn(&mut session, user("Make a.txt"))
        .await
        .unwrap();

    // The message sits after the tool result it arrived during, and before
    // the model's next answer.
    let order: Vec<String> = session
        .history()
        .messages
        .iter()
        .map(|message| match &message.content[0] {
            Block::Text { text } => text.clone(),
            Block::ToolUse { name, .. } => format!("call {name}"),
            Block::ToolResult { .. } => "result".into(),
            _ => String::new(),
        })
        .collect();
    assert_eq!(
        order,
        [
            "Make a.txt",
            "call write",
            "result",
            "Also add a README.",
            "Done, with a README."
        ]
    );

    // One that arrives as the model stops keeps the turn going.
    steering.push("Actually, stop there.");
    use zcode_agent::TurnHook;
    let follow_up = steering
        .before_stop(session.records(), false)
        .await
        .unwrap();
    assert!(matches!(&follow_up[0], Block::Text { text } if text == "Actually, stop there."));
    assert!(
        steering
            .before_stop(session.records(), false)
            .await
            .is_none()
    );
}

/// Streams part of an answer, then fails.
struct Flaky;

impl Llm for Flaky {
    fn stream<'a>(
        &'a self,
        _request: &'a Request,
        sink: &'a mut dyn DeltaSink,
    ) -> BoxFuture<'a, Result<Completion, LlmError>> {
        Box::pin(async move {
            sink.delta(Delta::Text {
                text: "Partial ans".into(),
            })
            .map_err(LlmError::Sink)?;
            Err(LlmError::Transport("connection reset".into()))
        })
    }
}

#[tokio::test]
async fn a_failed_request_closes_the_turn_and_the_session_stays_usable() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = new_session(&dir.path().join("sessions"));

    let error = agent(Arc::new(Flaky), dir.path())
        .run_turn(&mut session, user("Hi"))
        .await
        .unwrap_err();
    assert!(matches!(error, AgentError::Llm(LlmError::Transport(_))));

    let tail: Vec<Event> = session.records()[session.records().len() - 3..]
        .iter()
        .map(|record| record.event.clone())
        .collect();
    assert_eq!(
        tail,
        [
            Event::AssistantAttempt {
                content: vec![Block::text("Partial ans")],
                reason: AttemptReason::Failed,
            },
            Event::StepEnd { turn: 1, step: 1 },
            Event::TurnEnd {
                turn: 1,
                reason: TurnEndReason::Failed,
            },
        ]
    );

    let llm = Arc::new(ScriptedLlm::new(Recording {
        responses: vec![ScriptedResponse::text("Hello.")],
    }));
    agent(llm, dir.path())
        .run_turn(&mut session, user("Hi again"))
        .await
        .unwrap();
    // The failed attempt is in the log but not in what the model sees.
    let history = session.history();
    let roles: Vec<Role> = history.messages.iter().map(|m| m.role).collect();
    assert_eq!(roles, [Role::User, Role::User, Role::Assistant]);
}

#[tokio::test]
async fn truncated_tool_calls_are_not_run_and_step_limits_end_the_turn() {
    let dir = tempfile::tempdir().unwrap();
    let mut cut_short = tool_call("c1", "bash", json!({"command": "touch ran"}));
    cut_short.stop = StopReason::MaxTokens;
    let llm = Arc::new(ScriptedLlm::new(Recording {
        responses: vec![cut_short, ScriptedResponse::text("Retrying later.")],
    }));
    let mut session = new_session(&dir.path().join("sessions"));
    agent(llm, dir.path())
        .run_turn(&mut session, user("Run it"))
        .await
        .unwrap();
    assert!(!dir.path().join("ran").exists());
    let (text, is_error) = last_tool_result(&session);
    assert!(is_error && text.starts_with("Not run"));
    assert!(!kinds(&session).contains(&"tool/call"));

    let looping = Arc::new(ScriptedLlm::new(Recording {
        responses: (0..3)
            .map(|n| tool_call(&format!("l{n}"), "bash", json!({"command": "true"})))
            .collect(),
    }));
    let mut limited = agent(looping, dir.path());
    limited.max_steps = 3;
    let error = limited
        .run_turn(&mut session, user("Loop"))
        .await
        .unwrap_err();
    assert!(matches!(error, AgentError::StepLimit(3)));
    assert!(matches!(
        session.records().last().unwrap().event,
        Event::TurnEnd {
            reason: TurnEndReason::Failed,
            ..
        }
    ));
    assert!(session.history().dangling_tool_calls().is_empty());
}

#[tokio::test]
async fn replaying_the_recording_lifted_from_a_log_reproduces_the_log() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("work");
    fs::create_dir(&workspace).unwrap();
    let recording = Recording {
        responses: vec![
            tool_call("c1", "write", json!({"path": "n.txt", "content": "1\n"})),
            tool_call("c2", "bash", json!({"command": "cat n.txt; echo done"})),
            ScriptedResponse {
                deltas: vec![
                    Delta::Thinking { text: "ok".into() },
                    Delta::Text {
                        text: "All ".into(),
                    },
                    Delta::Text {
                        text: "set.".into(),
                    },
                ],
                stop: StopReason::EndTurn,
                usage: Usage {
                    input_tokens: 9,
                    output_tokens: 3,
                    ..Usage::default()
                },
            },
        ],
    };

    let run = |root: &str, recording: Recording| {
        let root = dir.path().join(root);
        let workspace = workspace.clone();
        async move {
            let mut session = new_session(&root);
            agent(Arc::new(ScriptedLlm::new(recording)), &workspace)
                .run_turn(&mut session, user("Count"))
                .await
                .unwrap();
            let lifted = Recording::from_log(session.records());
            (fs::read(root.join("s1/session.v1.jsonl")).unwrap(), lifted)
        }
    };

    let (first_log, lifted) = run("first", recording.clone()).await;
    assert_eq!(lifted, recording);
    let (second_log, _) = run("second", lifted).await;
    assert_eq!(first_log, second_log);
}

/// Sends the turn back once, the first time it tries to stop after a change.
struct Objector;

impl zcode_agent::TurnHook for Objector {
    fn before_stop<'a>(
        &'a self,
        records: &'a [zcode_session::Record],
        changed: bool,
    ) -> BoxFuture<'a, Option<Vec<Block>>> {
        Box::pin(async move {
            let already = records.iter().any(|record| {
                matches!(&record.event, Event::UserMessage { content }
                    if content == &vec![Block::text("Check failed: fix it.")])
            });
            (changed && !already).then(|| vec![Block::text("Check failed: fix it.")])
        })
    }
}

#[tokio::test]
async fn a_hook_can_keep_the_turn_going_when_the_model_tries_to_stop() {
    let dir = tempfile::tempdir().unwrap();
    let llm = Arc::new(ScriptedLlm::new(Recording {
        responses: vec![
            tool_call("c1", "write", json!({"path": "a.txt", "content": "bad"})),
            ScriptedResponse::text("Done."),
            tool_call("c2", "write", json!({"path": "a.txt", "content": "good"})),
            ScriptedResponse::text("Fixed."),
            ScriptedResponse::text("Just talking."),
        ],
    }));
    let mut agent = agent(llm, dir.path());
    agent.hooks.push(Arc::new(Objector));
    let mut session = new_session(&dir.path().join("sessions"));

    agent
        .run_turn(&mut session, user("Write a.txt"))
        .await
        .unwrap();

    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "good"
    );
    let users = kinds(&session).iter().filter(|k| **k == "user").count();
    assert_eq!(users, 2, "the hook's objection is logged as a user message");
    assert_eq!(
        kinds(&session).iter().filter(|k| **k == "turn/end").count(),
        1
    );

    // A turn that changes nothing is not held back.
    agent.run_turn(&mut session, user("Hi")).await.unwrap();
    assert_eq!(kinds(&session).iter().filter(|k| **k == "user").count(), 3);
}

#[tokio::test]
async fn a_change_can_carry_the_command_that_checks_it() {
    let dir = tempfile::tempdir().unwrap();
    let llm = Arc::new(ScriptedLlm::new(Recording {
        responses: vec![
            tool_call(
                "c1",
                "write",
                json!({"path": "a.txt", "content": "hello\n", "then_run": "cat a.txt; exit 3"}),
            ),
            tool_call(
                "c2",
                "edit",
                json!({"path": "a.txt", "old_string": "nope", "new_string": "x", "then_run": "echo ran"}),
            ),
            ScriptedResponse::text("Done."),
        ],
    }));
    let agent = agent(llm, dir.path());
    let mut session = new_session(&dir.path().join("sessions"));
    agent.run_turn(&mut session, user("Write")).await.unwrap();

    let results: Vec<(String, bool)> = session
        .records()
        .iter()
        .filter_map(|record| match &record.event {
            Event::ToolResult {
                content, is_error, ..
            } => match content.first() {
                Some(Block::Text { text }) => Some((text.clone(), *is_error)),
                _ => None,
            },
            _ => None,
        })
        .collect();
    // One result for the change and its command together.
    assert_eq!(results.len(), 2);
    let (text, failed) = &results[0];
    assert!(text.contains("[then_run] $ cat a.txt; exit 3") && text.contains("hello"));
    assert!(text.contains("exit code 3") && *failed);
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "hello\n"
    );
    // A change that was not made does not run its command.
    let (text, failed) = &results[1];
    assert!(*failed && text.contains("Not run") && !text.contains("\nran"));
}

/// Speaks once, whether or not anything changed.
struct Nudge;

impl zcode_agent::TurnHook for Nudge {
    fn before_stop<'a>(
        &'a self,
        records: &'a [zcode_session::Record],
        _changed: bool,
    ) -> BoxFuture<'a, Option<Vec<Block>>> {
        Box::pin(async move {
            let already = records.iter().any(|record| {
                matches!(&record.event, Event::UserMessage { content }
                    if content == &vec![Block::text("Did you run anything?")])
            });
            (!already).then(|| vec![Block::text("Did you run anything?")])
        })
    }
}

#[tokio::test]
async fn a_hook_that_speaks_first_does_not_hide_the_change_from_later_hooks() {
    let dir = tempfile::tempdir().unwrap();
    let llm = Arc::new(ScriptedLlm::new(Recording {
        responses: vec![
            tool_call("c1", "write", json!({"path": "a.txt", "content": "bad"})),
            ScriptedResponse::text("Done."),
            // After the first hook's nudge the model changes nothing.
            ScriptedResponse::text("Yes, done."),
            ScriptedResponse::text("Fixed."),
        ],
    }));
    let mut agent = agent(llm, dir.path());
    agent.hooks.push(Arc::new(Nudge));
    agent.hooks.push(Arc::new(Objector));
    let mut session = new_session(&dir.path().join("sessions"));

    agent
        .run_turn(&mut session, user("Write a.txt"))
        .await
        .unwrap();

    let said = |text: &str| {
        session.records().iter().any(|record| {
            matches!(&record.event, Event::UserMessage { content }
                if content == &vec![Block::text(text)])
        })
    };
    assert!(said("Did you run anything?"));
    assert!(
        said("Check failed: fix it."),
        "the second hook still hears about the change"
    );
}

#[tokio::test]
async fn an_answer_cut_off_by_the_output_limit_is_asked_to_carry_on() {
    let dir = tempfile::tempdir().unwrap();
    let cut = || ScriptedResponse {
        stop: StopReason::MaxTokens,
        ..ScriptedResponse::text("Thinking about")
    };
    let llm = Arc::new(ScriptedLlm::new(Recording {
        responses: vec![cut(), ScriptedResponse::text("The answer is 4.")],
    }));
    let agent = agent(llm, dir.path());
    let mut session = new_session(&dir.path().join("sessions"));

    agent.run_turn(&mut session, user("2+2?")).await.unwrap();

    let kinds = kinds(&session);
    assert_eq!(kinds.iter().filter(|k| **k == "user").count(), 2);
    assert_eq!(kinds.iter().filter(|k| **k == "turn/end").count(), 1);
}

/// A read-only tool that takes a while and notes when it ran.
struct Slow(Arc<Mutex<Vec<(std::time::Instant, std::time::Instant)>>>);

impl zcode_tools::Tool for Slow {
    fn spec(&self) -> zcode_session::ToolSpec {
        zcode_session::ToolSpec {
            name: "slow".into(),
            description: String::new(),
            input_schema: json!({"type": "object"}),
        }
    }

    fn effect(&self) -> Effect {
        Effect::ReadOnly
    }

    fn call<'a>(&'a self, _input: &'a Value, _ctx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            let started = std::time::Instant::now();
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            self.0
                .lock()
                .unwrap()
                .push((started, std::time::Instant::now()));
            ToolOutput::ok("done")
        })
    }
}

#[tokio::test]
async fn read_only_calls_issued_together_overlap_and_writes_stay_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let call = |id: &str, name: &str, input: Value| {
        vec![
            Delta::ToolUseStart {
                id: id.into(),
                name: name.into(),
            },
            Delta::ToolInput {
                json: input.to_string(),
            },
        ]
    };
    let mut deltas = Vec::new();
    deltas.extend(call("s1", "slow", json!({})));
    deltas.extend(call("s2", "slow", json!({})));
    deltas.extend(call("s3", "slow", json!({})));
    deltas.extend(call(
        "w1",
        "write",
        json!({"path": "a.txt", "content": "1"}),
    ));
    deltas.extend(call(
        "w2",
        "edit",
        json!({"path": "a.txt", "old_string": "1", "new_string": "2"}),
    ));
    let llm = Arc::new(ScriptedLlm::new(Recording {
        responses: vec![
            ScriptedResponse {
                deltas,
                stop: StopReason::ToolUse,
                usage: Usage::default(),
            },
            ScriptedResponse::text("Done."),
        ],
    }));
    let runs = Arc::new(Mutex::new(Vec::new()));
    let mut agent = agent(llm, dir.path());
    agent.tools.insert(Arc::new(Slow(Arc::clone(&runs))));
    let mut session = new_session(&dir.path().join("sessions"));

    agent.run_turn(&mut session, user("Go")).await.unwrap();

    // The three slow calls ran at the same time.
    let runs = runs.lock().unwrap();
    let latest_start = runs.iter().map(|(start, _)| *start).max().unwrap();
    let earliest_end = runs.iter().map(|(_, end)| *end).min().unwrap();
    assert!(latest_start < earliest_end, "the calls did not overlap");
    // The write and the edit that depends on it ran one after the other.
    assert_eq!(fs::read_to_string(dir.path().join("a.txt")).unwrap(), "2");
    let step: Vec<&str> = kinds(&session)
        .into_iter()
        .filter(|kind| kind.starts_with("tool/"))
        .collect();
    assert_eq!(
        step,
        [
            "tool/call",
            "tool/call",
            "tool/call",
            "tool/result",
            "tool/result",
            "tool/result",
            "tool/call",
            "tool/result",
            "tool/call",
            "tool/result",
        ]
    );
    assert!(session.history().dangling_tool_calls().is_empty());
}
