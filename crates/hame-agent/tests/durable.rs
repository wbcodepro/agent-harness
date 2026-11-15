//! Images, action receipts and old logs through the agent loop.

use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

use hame_agent::{Agent, Session, ToolPipeline};
use hame_eval::{Recording, ScriptedLlm, ScriptedResponse};
use hame_kernel::BoxFuture;
use hame_llm::{Completion, DeltaSink, Llm, LlmError, Request};
use hame_session::contract::{
    ActionJournal, ActionReceipt, ActionStatus, CONTRACT_VERSION, Capability, Channel, Target,
};
use hame_session::{
    Block, CreateOptions, Delta, Event, History, Message, Record, Role, SessionId, SessionStore,
    StopReason, TurnEndReason, Usage, derive_history,
};
use hame_tools::{Effect, InProcess, Tool, ToolContext, ToolOutput, ToolSet};
use serde_json::{Value, json};

/// Wraps a model and remembers every request it was sent.
struct Spy {
    inner: ScriptedLlm,
    requests: Mutex<Vec<Request>>,
}

impl Spy {
    fn new(responses: Vec<ScriptedResponse>) -> Arc<Self> {
        Arc::new(Self {
            inner: ScriptedLlm::new(Recording { responses }),
            requests: Mutex::new(Vec::new()),
        })
    }
}

impl Llm for Spy {
    fn stream<'a>(
        &'a self,
        request: &'a Request,
        sink: &'a mut dyn DeltaSink,
    ) -> BoxFuture<'a, Result<Completion, LlmError>> {
        self.requests.lock().unwrap().push(request.clone());
        self.inner.stream(request, sink)
    }
}

const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0x0d, 0x49, 0x48, 0x44, 0x52, 0, 0, 0,
    7, 0, 0, 0, 5, 8, 6, 0, 0, 0,
];

/// Takes a "screenshot": stores an image in the session's artifact store
/// and returns it, remembering which session asked.
struct Shoot(Arc<Mutex<Vec<String>>>);

impl Tool for Shoot {
    fn spec(&self) -> hame_session::ToolSpec {
        hame_session::ToolSpec {
            name: "shoot".into(),
            description: "Take a screenshot.".into(),
            input_schema: json!({"type": "object", "properties": {}}),
        }
    }

    fn effect(&self) -> Effect {
        Effect::ReadOnly
    }

    fn call<'a>(&'a self, _input: &'a Value, ctx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            self.0
                .lock()
                .unwrap()
                .push(ctx.session_id.clone().unwrap_or_default());
            let Some(store) = &ctx.artifacts else {
                return ToolOutput::error("no artifact store");
            };
            let artifact = store.put(PNG, "image/png").unwrap();
            ToolOutput::ok("Took a screenshot.").with_part(Block::Image { artifact })
        })
    }
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

/// Clicks: journals the action through its life in the session's journal.
/// With `hang`, it stops after dispatching and never returns.
struct Click {
    hang: bool,
}

impl Tool for Click {
    fn spec(&self) -> hame_session::ToolSpec {
        hame_session::ToolSpec {
            name: "click".into(),
            description: "Click.".into(),
            input_schema: json!({"type": "object", "properties": {"op": {"type": "string"}}}),
        }
    }

    fn effect(&self) -> Effect {
        Effect::Control
    }

    fn host_side(&self) -> bool {
        true
    }

    fn call<'a>(&'a self, input: &'a Value, ctx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            let journal = ctx.journal.as_ref().expect("a journal for the session");
            let op = input["op"].as_str().unwrap_or("op");
            journal
                .record(&receipt(op, ActionStatus::NotStarted))
                .unwrap();
            journal
                .record(&receipt(op, ActionStatus::Dispatched))
                .unwrap();
            if self.hang {
                std::future::pending::<()>().await;
            }
            journal
                .record(&receipt(op, ActionStatus::Completed))
                .unwrap();
            ToolOutput::ok(format!("clicked {op}"))
        })
    }
}

fn new_agent(llm: Arc<dyn Llm>, workspace: &Path) -> Agent {
    Agent {
        llm,
        tools: ToolSet::new(),
        pipeline: Arc::new(ToolPipeline::new()),
        tool_context: ToolContext::new(workspace),
        runner: Arc::new(InProcess),
        checkpointer: None,
        hooks: Vec::new(),
        system: "You are hame.".into(),
        max_steps: 8,
        max_tokens: None,
        max_tool_calls: 32,
        fuse_actions: false,
        dedupe_results: true,
        deferred: ToolSet::new(),
    }
}

fn call(id: &str, name: &str, input: Value) -> ScriptedResponse {
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

fn create(store: &SessionStore, raw: &str) -> Session {
    let writer = store
        .create(CreateOptions {
            id: Some(SessionId::parse(raw).unwrap()),
            cwd: "/work".into(),
        })
        .unwrap();
    let records = store.read(writer.id()).unwrap();
    Session::new(records, writer)
}

/// The images in a request's tool results, read back through the
/// request's own artifact source.
fn images_sent(request: &Request) -> Vec<Vec<u8>> {
    let source = request
        .artifacts
        .as_ref()
        .expect("the request can read artifacts");
    request
        .messages
        .iter()
        .flat_map(|message| message.content.iter())
        .flat_map(|block| block.artifacts())
        .map(|artifact| source.read(artifact).unwrap())
        .collect()
}

#[tokio::test]
async fn an_image_survives_tool_log_history_request_and_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("sessions");
    let asked = Arc::new(Mutex::new(Vec::new()));

    let spy = Spy::new(vec![
        call("c1", "shoot", json!({})),
        ScriptedResponse::text("A 7x5 image."),
    ]);
    let mut agent = new_agent(spy.clone(), dir.path());
    agent.tools.insert(Arc::new(Shoot(Arc::clone(&asked))));
    let mut session = create(&SessionStore::new(&root), "s1");
    agent
        .run_turn(&mut session, vec![Block::text("Look")])
        .await
        .unwrap();
    assert_eq!(*asked.lock().unwrap(), ["s1"], "the tool knew its session");

    // In the log, as a reference.
    let artifact = session
        .records()
        .iter()
        .find_map(|record| match &record.event {
            Event::ToolResult { content, .. } => match &content[..] {
                [Block::Text { text }, Block::Image { artifact }]
                    if text == "Took a screenshot." =>
                {
                    Some(artifact.clone())
                }
                _ => None,
            },
            _ => None,
        })
        .expect("the image is in the tool result");
    assert_eq!((artifact.width, artifact.height), (Some(7), Some(5)));
    let log = fs::read_to_string(root.join("s1").join("session.v1.jsonl")).unwrap();
    assert!(log.contains(&artifact.sha256));
    assert!(!log.contains("iVBOR"), "no image bytes in the log");
    // In the history and the next request, readable through the request.
    assert!(
        session
            .history()
            .messages
            .iter()
            .any(|m| m.content.iter().any(|b| b.artifacts().contains(&&artifact)))
    );
    {
        let requests = spy.requests.lock().unwrap();
        assert_eq!(images_sent(&requests[1]), [PNG.to_vec()]);
    }
    drop(session);
    drop(agent);

    // A new process: a new store and agent, the session resumed.
    let store = SessionStore::new(&root);
    let (records, writer) = store.open(&SessionId::parse("s1").unwrap()).unwrap();
    assert!(store.check_artifacts(writer.id()).unwrap().is_empty());
    let mut session = Session::new(records, writer);
    let spy = Spy::new(vec![ScriptedResponse::text("Still 7x5.")]);
    let agent = new_agent(spy.clone(), dir.path());
    agent
        .run_turn(&mut session, vec![Block::text("Again?")])
        .await
        .unwrap();
    let requests = spy.requests.lock().unwrap();
    assert_eq!(images_sent(&requests[0]), [PNG.to_vec()]);
}

#[tokio::test]
async fn a_version_1_log_opens_derives_the_same_history_and_takes_new_turns() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("sessions");
    fs::create_dir_all(root.join("old")).unwrap();
    // Written by a build that knew only version 1: text blocks, no
    // artifacts, no receipts, no journal beside it.
    let lines = [
        r#"{"seq":0,"ts":1,"type":"session","data":{"version":1,"id":"old","cwd":"/work"}}"#,
        r#"{"seq":1,"ts":2,"type":"turn/start","data":{"turn":1}}"#,
        r#"{"seq":2,"ts":3,"type":"system/message","data":{"text":"You are hame."}}"#,
        r#"{"seq":3,"ts":4,"type":"user/message","data":{"content":[{"type":"text","text":"What is in a.rs?"}]}}"#,
        r#"{"seq":4,"ts":5,"type":"step/start","data":{"turn":1,"step":1}}"#,
        r#"{"seq":5,"ts":6,"type":"assistant/message","data":{"content":[{"type":"text","text":"Let me look."},{"type":"tool_use","id":"c1","name":"read","input":{"path":"a.rs"}}],"stop":"tool_use","usage":{"input_tokens":10,"output_tokens":2}}}"#,
        r#"{"seq":6,"ts":7,"type":"tool/call","data":{"call_id":"c1","name":"read","input":{"path":"a.rs"}}}"#,
        r#"{"seq":7,"ts":8,"type":"tool/result","data":{"call_id":"c1","content":[{"type":"text","text":"fn a() {}"}],"is_error":false}}"#,
        r#"{"seq":8,"ts":9,"type":"step/end","data":{"turn":1,"step":1}}"#,
        r#"{"seq":9,"ts":10,"type":"step/start","data":{"turn":1,"step":2}}"#,
        r#"{"seq":10,"ts":11,"type":"assistant/message","data":{"content":[{"type":"text","text":"One function."}],"stop":"end_turn"}}"#,
        r#"{"seq":11,"ts":12,"type":"step/end","data":{"turn":1,"step":2}}"#,
        r#"{"seq":12,"ts":13,"type":"turn/end","data":{"turn":1,"reason":"completed"}}"#,
    ];
    let text: String = lines.iter().map(|line| format!("{line}\n")).collect();
    let path = root.join("old").join("session.v1.jsonl");
    fs::write(&path, &text).unwrap();

    let expected = History {
        system: Some("You are hame.".into()),
        tools: Vec::new(),
        messages: vec![
            Message {
                role: Role::User,
                content: vec![Block::text("What is in a.rs?")],
            },
            Message {
                role: Role::Assistant,
                content: vec![
                    Block::text("Let me look."),
                    Block::ToolUse {
                        id: "c1".into(),
                        name: "read".into(),
                        input: json!({"path": "a.rs"}),
                    },
                ],
            },
            Message {
                role: Role::Tool,
                content: vec![Block::ToolResult {
                    call_id: "c1".into(),
                    content: vec![Block::text("fn a() {}")],
                    is_error: false,
                }],
            },
            Message {
                role: Role::Assistant,
                content: vec![Block::text("One function.")],
            },
        ],
    };
    let store = SessionStore::new(&root);
    let id = SessionId::parse("old").unwrap();
    assert_eq!(derive_history(&store.read(&id).unwrap()), expected);

    let (records, writer) = store.open(&id).unwrap();
    assert_eq!(records.len(), lines.len(), "a closed log gains nothing");
    assert_eq!(derive_history(&records), expected);
    assert!(store.check_artifacts(&id).unwrap().is_empty());
    // The file is untouched by opening it.
    assert_eq!(fs::read_to_string(&path).unwrap(), text);

    let spy = Spy::new(vec![ScriptedResponse::text("Still one.")]);
    let agent = new_agent(spy.clone(), dir.path());
    let mut session = Session::new(records, writer);
    agent
        .run_turn(&mut session, vec![Block::text("And now?")])
        .await
        .unwrap();
    let requests = spy.requests.lock().unwrap();
    assert_eq!(requests[0].messages[..4], expected.messages[..]);
    assert_eq!(
        requests[0].messages[4].content,
        vec![Block::text("And now?")]
    );
    assert_eq!(derive_history(session.records()).messages.len(), 6);
    // The new turn's records are appended to the old file, which still
    // reads as one log.
    assert!(fs::read_to_string(&path).unwrap().starts_with(&text));
    assert_eq!(store.read(&id).unwrap().len(), session.records().len());
}

fn receipt_ops(records: &[Record]) -> Vec<(String, ActionStatus)> {
    records
        .iter()
        .filter_map(|record| match &record.event {
            Event::ActionReceipt { receipt } => Some((receipt.op_id.clone(), receipt.status)),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn each_step_logs_its_settled_receipts_once_in_the_session_that_made_them() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new(dir.path().join("sessions"));
    let spy = Spy::new(vec![
        call("c1", "click", json!({"op": "one"})),
        call("c2", "click", json!({"op": "two"})),
        ScriptedResponse::text("Clicked twice."),
        call("c3", "click", json!({"op": "three"})),
        ScriptedResponse::text("Clicked."),
    ]);
    let mut agent = new_agent(spy, dir.path());
    agent.tools.insert(Arc::new(Click { hang: false }));
    let mut first = create(&store, "s1");
    let mut second = create(&store, "s2");

    agent
        .run_turn(&mut first, vec![Block::text("Click twice")])
        .await
        .unwrap();
    // One agent, another session: its own journal.
    agent
        .run_turn(&mut second, vec![Block::text("Click")])
        .await
        .unwrap();

    assert_eq!(
        receipt_ops(first.records()),
        [
            ("one".to_string(), ActionStatus::Completed),
            ("two".to_string(), ActionStatus::Completed)
        ]
    );
    assert_eq!(
        receipt_ops(second.records()),
        [("three".to_string(), ActionStatus::Completed)]
    );
    // Each receipt follows its own step's tool result, before the step
    // ends.
    let kinds: Vec<&str> = first
        .records()
        .iter()
        .filter_map(|record| match record.event {
            Event::ToolResult { .. } => Some("result"),
            Event::ActionReceipt { .. } => Some("receipt"),
            Event::StepEnd { .. } => Some("end"),
            _ => None,
        })
        .collect();
    assert_eq!(
        kinds,
        [
            "result", "receipt", "end", "result", "receipt", "end", "end"
        ]
    );
    let journal = ActionJournal::read(&store.journal_path(first.id())).unwrap();
    assert!(journal.iter().all(|r| r.op_id != "three"));
}

#[tokio::test]
async fn a_turn_cancelled_mid_action_reports_it_as_outcome_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new(dir.path().join("sessions"));
    let spy = Spy::new(vec![call("c1", "click", json!({"op": "pay"}))]);
    let mut agent = new_agent(spy, dir.path());
    agent.tools.insert(Arc::new(Click { hang: true }));
    let mut session = create(&store, "s1");

    let journal_path = store.journal_path(session.id());
    tokio::select! {
        _ = agent.run_turn(&mut session, vec![Block::text("Pay")]) => panic!("the click hangs"),
        _ = async {
            while ActionJournal::read(&journal_path).unwrap().len() < 2 {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        } => {}
    }
    session.close_open_turn(TurnEndReason::Cancelled).unwrap();

    assert_eq!(
        receipt_ops(session.records()),
        [("pay".to_string(), ActionStatus::OutcomeUnknown)]
    );
    let result = session
        .records()
        .iter()
        .find_map(|record| match &record.event {
            Event::ToolResult { content, .. } => Some(hame_session::blocks_text(content)),
            _ => None,
        })
        .unwrap();
    assert!(result.contains("(op pay): outcome_unknown"), "{result}");
    assert!(session.history().dangling_tool_calls().is_empty());
}
