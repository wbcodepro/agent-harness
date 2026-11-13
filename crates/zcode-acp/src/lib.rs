//! Agent Client Protocol (ACP) server.
//!
//! Speaks JSON-RPC 2.0, one message per line, over any byte stream pair;
//! `zcode acp` connects it to stdin and stdout so an editor can run zcode as its
//! agent. Each ACP session is an zcode session: prompts run turns, the session
//! log is streamed back as `session/update` notifications, and approval
//! questions become `session/request_permission` requests to the editor.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{Notify, mpsc, oneshot};
use zcode_agent::{AgentError, Session, ToolRequest};
use zcode_app::{Options, Prompter, Runtime};
use zcode_kernel::BoxFuture;
use zcode_session::{Block, Delta, Event, Record, SessionId, TurnEndReason};

const PROTOCOL_VERSION: u64 = 1;

/// Everything written to the client goes through here, so messages from
/// concurrent turns never interleave within a line.
type Outbox = mpsc::UnboundedSender<Value>;

/// Requests this server sent that still await the client's response.
#[derive(Default)]
struct Pending {
    next_id: AtomicU64,
    waiting: Mutex<HashMap<u64, oneshot::Sender<Value>>>,
}

struct Entry {
    state: tokio::sync::Mutex<(Session, Runtime)>,
    cancel: Notify,
    cwd: PathBuf,
}

struct Server {
    home: PathBuf,
    options: Options,
    outbox: Outbox,
    pending: Arc<Pending>,
    sessions: Mutex<HashMap<String, Arc<Entry>>>,
    /// The client asked for whole session records (`_zcode/record`) instead of
    /// the protocol's summarized `session/update` notifications.
    raw_records: AtomicBool,
}

struct RpcError {
    code: i64,
    message: String,
}

impl RpcError {
    fn invalid(message: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            code: -32603,
            message: message.into(),
        }
    }
}

/// Serves one client until `reader` reaches end of input.
pub async fn serve<R, W>(
    home: PathBuf,
    options: Options,
    reader: R,
    mut writer: W,
) -> std::io::Result<()>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (outbox, mut outgoing) = mpsc::unbounded_channel::<Value>();
    let writing = tokio::spawn(async move {
        while let Some(message) = outgoing.recv().await {
            let mut line = message.to_string();
            line.push('\n');
            if writer.write_all(line.as_bytes()).await.is_err() || writer.flush().await.is_err() {
                break;
            }
        }
    });

    let server = Arc::new(Server {
        home,
        options,
        outbox,
        pending: Arc::new(Pending::default()),
        sessions: Mutex::new(HashMap::new()),
        raw_records: AtomicBool::new(false),
    });

    let mut lines = BufReader::new(reader).lines();
    while let Some(line) = lines.next_line().await? {
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let id = message.get("id").cloned();
        match (message["method"].as_str(), id) {
            (Some(method), Some(id)) => {
                // Each request runs on its own task: a prompt takes as long
                // as its turn, and cancellations must still get through.
                let server = Arc::clone(&server);
                let method = method.to_string();
                let params = message["params"].clone();
                tokio::spawn(async move {
                    let reply = match server.handle(&method, &params).await {
                        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
                        Err(e) => json!({
                            "jsonrpc": "2.0", "id": id,
                            "error": {"code": e.code, "message": e.message}
                        }),
                    };
                    let _ = server.outbox.send(reply);
                });
            }
            (Some("session/cancel"), None) => {
                if let Some(entry) = server.entry(message["params"]["sessionId"].as_str()) {
                    entry.cancel.notify_waiters();
                }
            }
            (Some(_), None) => {}
            (None, Some(id)) => {
                let waiter = id.as_u64().and_then(|id| {
                    server
                        .pending
                        .waiting
                        .lock()
                        .expect("pending lock")
                        .remove(&id)
                });
                if let Some(waiter) = waiter {
                    let _ = waiter.send(message);
                }
            }
            (None, None) => {}
        }
    }

    // The client is gone: stop every turn and let the writer drain.
    let entries: Vec<Arc<Entry>> = server
        .sessions
        .lock()
        .expect("sessions lock")
        .values()
        .cloned()
        .collect();
    for entry in entries {
        entry.cancel.notify_waiters();
    }
    drop(server);
    let _ = writing.await;
    Ok(())
}

impl Server {
    fn entry(&self, id: Option<&str>) -> Option<Arc<Entry>> {
        self.sessions
            .lock()
            .expect("sessions lock")
            .get(id?)
            .cloned()
    }

    async fn handle(&self, method: &str, params: &Value) -> Result<Value, RpcError> {
        match method {
            "initialize" => {
                if params["clientCapabilities"]["_meta"]["zcode"]["records"] == true {
                    self.raw_records.store(true, Ordering::Relaxed);
                }
                self.initialize()
            }
            "session/set_mode" => self.set_mode(params).await,
            "_zcode/rewind" => self.rewind(params).await,
            "authenticate" => Ok(json!({})),
            "session/new" => {
                let cwd = params["cwd"]
                    .as_str()
                    .ok_or_else(|| RpcError::invalid("session/new needs cwd"))?;
                let (id, sandbox) = self.start(None, Path::new(cwd), false).await?;
                Ok(json!({
                    "sessionId": id,
                    "modes": modes(false),
                    "_meta": {"zcode": {"sandbox": sandbox}},
                }))
            }
            "session/load" => {
                let id = params["sessionId"]
                    .as_str()
                    .ok_or_else(|| RpcError::invalid("session/load needs sessionId"))?;
                let id = SessionId::parse(id).map_err(|e| RpcError::invalid(e.to_string()))?;
                let store = zcode_app::session_store(&self.home);
                let cwd = zcode_app::session_cwd(&store, &id).map_err(RpcError::invalid)?;
                let (_, sandbox) = self.start(Some(&id), &cwd, false).await?;
                Ok(json!({"modes": modes(false), "_meta": {"zcode": {"sandbox": sandbox}}}))
            }
            "session/prompt" => self.prompt(params).await,
            other => Err(RpcError {
                code: -32601,
                message: format!("method not found: {other}"),
            }),
        }
    }

    fn initialize(&self) -> Result<Value, RpcError> {
        Ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "agentInfo": {"name": "zcode", "version": env!("CARGO_PKG_VERSION")},
            "agentCapabilities": {
                "loadSession": true,
                "promptCapabilities": {"image": false, "audio": false, "embeddedContext": true},
            },
            "authMethods": [],
        }))
    }

    async fn runtime(&self, id: &str, cwd: &Path, plan: bool) -> Result<Runtime, RpcError> {
        let mut options = self.options.clone();
        options.plan = plan;
        options.prompter = Some(Arc::new(PermissionPrompter {
            session_id: id.to_string(),
            outbox: self.outbox.clone(),
            pending: Arc::clone(&self.pending),
        }));
        zcode_app::build(&self.home, cwd, &options)
            .await
            .map_err(RpcError::internal)
    }

    /// Switches a session between acting and planning by rebuilding its
    /// agent; the session itself carries on.
    async fn set_mode(&self, params: &Value) -> Result<Value, RpcError> {
        let id = params["sessionId"].as_str().unwrap_or("");
        let entry = self
            .entry(Some(id))
            .ok_or_else(|| RpcError::invalid("unknown sessionId"))?;
        let plan = match params["modeId"].as_str() {
            Some("plan") => true,
            Some("act") => false,
            other => return Err(RpcError::invalid(format!("unknown mode {other:?}"))),
        };
        let runtime = self.runtime(id, &entry.cwd, plan).await?;
        entry.state.lock().await.1 = runtime;
        Ok(json!({}))
    }

    /// Extension: undoes the session's last turn, files included, and
    /// continues in a fork that ends just before it.
    async fn rewind(&self, params: &Value) -> Result<Value, RpcError> {
        let id = params["sessionId"].as_str().unwrap_or("");
        let entry = self
            .entry(Some(id))
            .ok_or_else(|| RpcError::invalid("unknown sessionId"))?;
        let id = SessionId::parse(id).map_err(|e| RpcError::invalid(e.to_string()))?;
        let store = zcode_app::session_store(&self.home);
        let rewound = zcode_app::rewind(&self.home, &store, &id, None)
            .await
            .map_err(RpcError::invalid)?;
        let (fork, _) = self
            .start(Some(&rewound.session), &entry.cwd, false)
            .await?;
        Ok(json!({
            "sessionId": fork,
            "turn": rewound.turn,
            "removed": rewound.restored.removed,
        }))
    }

    /// Creates or reopens a session and registers it. Reopening replays the
    /// conversation to the client, as the protocol requires. Returns the
    /// session id and how its tools are confined.
    async fn start(
        &self,
        existing: Option<&SessionId>,
        cwd: &Path,
        plan: bool,
    ) -> Result<(String, String), RpcError> {
        let cwd = zcode_kernel::real_path(cwd)
            .map_err(|e| RpcError::invalid(format!("cannot use {}: {e}", cwd.display())))?;
        let store = zcode_app::session_store(&self.home);
        let mut session =
            zcode_app::open_session(&store, existing, &cwd).map_err(RpcError::internal)?;
        let id = session.id().to_string();

        let runtime = self.runtime(&id, &cwd, plan).await?;
        let sandbox = format!("{:?}", runtime.policy.mode);

        let mut updates = Updates {
            session_id: id.clone(),
            outbox: self.outbox.clone(),
            raw: self.raw_records.load(Ordering::Relaxed),
        };
        if existing.is_some() {
            for record in session.records() {
                updates.replay(record);
            }
        }
        session.observe(move |record| updates.live(record));

        self.sessions.lock().expect("sessions lock").insert(
            id.clone(),
            Arc::new(Entry {
                state: tokio::sync::Mutex::new((session, runtime)),
                cancel: Notify::new(),
                cwd,
            }),
        );
        Ok((id, sandbox))
    }

    async fn prompt(&self, params: &Value) -> Result<Value, RpcError> {
        let entry = self
            .entry(params["sessionId"].as_str())
            .ok_or_else(|| RpcError::invalid("unknown sessionId"))?;
        let mut text = String::new();
        for block in params["prompt"].as_array().into_iter().flatten() {
            match block["type"].as_str() {
                Some("text") => text.push_str(block["text"].as_str().unwrap_or("")),
                // An embedded file: give the model its content inline.
                Some("resource") => {
                    let resource = &block["resource"];
                    text.push_str(&format!(
                        "\n<file uri=\"{}\">\n{}\n</file>\n",
                        resource["uri"].as_str().unwrap_or(""),
                        resource["text"].as_str().unwrap_or("")
                    ));
                }
                Some("resource_link") => {
                    text.push_str(&format!(" {} ", block["uri"].as_str().unwrap_or("")));
                }
                _ => {}
            }
        }
        if text.trim().is_empty() {
            return Err(RpcError::invalid("the prompt has no text"));
        }

        let mut state = entry.state.lock().await;
        let (session, runtime) = &mut *state;
        let outcome = tokio::select! {
            result = runtime.agent.run_turn(session, vec![Block::text(text)]) => Some(result),
            _ = entry.cancel.notified() => None,
        };
        match outcome {
            Some(Ok(())) => Ok(json!({"stopReason": "end_turn"})),
            Some(Err(AgentError::StepLimit(_))) => Ok(json!({"stopReason": "max_turn_requests"})),
            Some(Err(e)) => Err(RpcError::internal(e.to_string())),
            None => {
                session
                    .close_open_turn(TurnEndReason::Cancelled)
                    .map_err(|e| RpcError::internal(e.to_string()))?;
                Ok(json!({"stopReason": "cancelled"}))
            }
        }
    }
}

/// Turns session records into `session/update` notifications.
struct Updates {
    session_id: String,
    outbox: Outbox,
    /// Send each record whole, as the `_zcode/record` extension notification.
    raw: bool,
}

/// The modes a session can be in, in the protocol's shape.
fn modes(plan: bool) -> Value {
    json!({
        "currentModeId": if plan { "plan" } else { "act" },
        "availableModes": [
            {"id": "act", "name": "Act", "description": "Do the work."},
            {"id": "plan", "name": "Plan", "description": "Investigate and propose a plan; change nothing."},
        ],
    })
}

impl Updates {
    fn send(&self, update: Value) {
        let _ = self.outbox.send(json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {"sessionId": self.session_id, "update": update},
        }));
    }

    fn chunk(&self, kind: &str, text: &str) {
        self.send(json!({"sessionUpdate": kind, "content": {"type": "text", "text": text}}));
    }

    fn tool(&self, record: &Record) -> bool {
        match &record.event {
            Event::ToolCall {
                call_id,
                name,
                input,
            } => {
                let kind = match name.as_str() {
                    "read" => "read",
                    "write" | "edit" => "edit",
                    "bash" => "execute",
                    "glob" | "grep" | "repo_map" => "search",
                    _ => "other",
                };
                let subject = input["command"]
                    .as_str()
                    .or_else(|| input["path"].as_str())
                    .or_else(|| input["pattern"].as_str())
                    .unwrap_or("");
                let title: String = format!("{name} {subject}")
                    .trim()
                    .chars()
                    .take(120)
                    .collect();
                self.send(json!({
                    "sessionUpdate": "tool_call",
                    "toolCallId": call_id,
                    "title": title,
                    "kind": kind,
                    "status": "in_progress",
                    "rawInput": input,
                }));
                true
            }
            Event::ToolResult {
                call_id,
                content,
                is_error,
            } => {
                let text = match content.first() {
                    Some(Block::Text { text }) => text.as_str(),
                    _ => "",
                };
                self.send(json!({
                    "sessionUpdate": "tool_call_update",
                    "toolCallId": call_id,
                    "status": if *is_error { "failed" } else { "completed" },
                    "content": [{"type": "content", "content": {"type": "text", "text": text}}],
                }));
                true
            }
            _ => false,
        }
    }

    fn record(&self, record: &Record, replay: bool) {
        let _ = self.outbox.send(json!({
            "jsonrpc": "2.0",
            "method": "_zcode/record",
            "params": {"sessionId": self.session_id, "record": record, "replay": replay},
        }));
    }

    /// A record appended while a turn runs: text arrives as deltas.
    fn live(&mut self, record: &Record) {
        if self.raw {
            return self.record(record, false);
        }
        if self.tool(record) {
            return;
        }
        if let Event::AssistantDelta { delta } = &record.event {
            match delta {
                Delta::Text { text } => self.chunk("agent_message_chunk", text),
                Delta::Thinking { text } => self.chunk("agent_thought_chunk", text),
                Delta::ThinkingSignature { .. }
                | Delta::Fallback { .. }
                | Delta::ToolUseStart { .. }
                | Delta::ToolInput { .. } => {}
            }
        }
    }

    /// A record from a session being loaded: whole messages, including the
    /// user's.
    fn replay(&mut self, record: &Record) {
        if self.raw {
            return self.record(record, true);
        }
        if self.tool(record) {
            return;
        }
        let (kind, content) = match &record.event {
            Event::UserMessage { content } => ("user_message_chunk", content),
            Event::AssistantMessage { content, .. } => ("agent_message_chunk", content),
            _ => return,
        };
        for block in content {
            if let Block::Text { text } = block {
                self.chunk(kind, text);
            }
        }
    }
}

/// Asks the editor whether a tool call may run.
struct PermissionPrompter {
    session_id: String,
    outbox: Outbox,
    pending: Arc<Pending>,
}

impl Prompter for PermissionPrompter {
    fn ask<'a>(
        &'a self,
        request: &'a ToolRequest,
        question: String,
    ) -> BoxFuture<'a, Option<bool>> {
        Box::pin(async move {
            let id = self.pending.next_id.fetch_add(1, Ordering::Relaxed) + 1;
            let (answer, answered) = oneshot::channel();
            self.pending
                .waiting
                .lock()
                .expect("pending lock")
                .insert(id, answer);
            let sent = self.outbox.send(json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "session/request_permission",
                "params": {
                    "sessionId": self.session_id,
                    "toolCall": {"toolCallId": request.call_id, "title": question},
                    "options": [
                        {"optionId": "allow-once", "name": "Allow once", "kind": "allow_once"},
                        {"optionId": "reject-once", "name": "Reject", "kind": "reject_once"},
                    ],
                },
            }));
            if sent.is_err() {
                return None;
            }
            // A dropped connection or a cancelled prompt leaves nobody to
            // answer; that is a refusal, never an approval.
            let response = answered.await.ok()?;
            let outcome = &response["result"]["outcome"];
            match outcome["outcome"].as_str() {
                Some("selected") => Some(outcome["optionId"].as_str() == Some("allow-once")),
                _ => Some(false),
            }
        })
    }
}
