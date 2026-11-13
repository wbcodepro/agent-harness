//! Agent Client Protocol (ACP) server.
//!
//! Speaks JSON-RPC 2.0, one message per line, over any byte stream pair;
//! `hx acp` connects it to stdin and stdout so an editor can run hx as its
//! agent. Each ACP session is an hx session: prompts run turns, the session
//! log is streamed back as `session/update` notifications, and approval
//! questions become `session/request_permission` requests to the editor.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use hx_agent::{AgentError, Session, ToolRequest};
use hx_app::{Options, Prompter, Runtime};
use hx_kernel::BoxFuture;
use hx_session::{Block, Delta, Event, Record, SessionId, TurnEndReason};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{Notify, mpsc, oneshot};

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
}

struct Server {
    home: PathBuf,
    options: Options,
    outbox: Outbox,
    pending: Arc<Pending>,
    sessions: Mutex<HashMap<String, Arc<Entry>>>,
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
            "initialize" => Ok(json!({
                "protocolVersion": PROTOCOL_VERSION,
                "agentInfo": {"name": "hx", "version": env!("CARGO_PKG_VERSION")},
                "agentCapabilities": {
                    "loadSession": true,
                    "promptCapabilities": {"image": false, "audio": false, "embeddedContext": true},
                },
                "authMethods": [],
            })),
            "authenticate" => Ok(json!({})),
            "session/new" => {
                let cwd = params["cwd"]
                    .as_str()
                    .ok_or_else(|| RpcError::invalid("session/new needs cwd"))?;
                let id = self.start(None, Path::new(cwd)).await?;
                Ok(json!({"sessionId": id}))
            }
            "session/load" => {
                let id = params["sessionId"]
                    .as_str()
                    .ok_or_else(|| RpcError::invalid("session/load needs sessionId"))?;
                let id = SessionId::parse(id).map_err(|e| RpcError::invalid(e.to_string()))?;
                let store = hx_app::session_store(&self.home);
                let cwd = hx_app::session_cwd(&store, &id).map_err(RpcError::invalid)?;
                self.start(Some(&id), &cwd).await?;
                Ok(Value::Null)
            }
            "session/prompt" => self.prompt(params).await,
            other => Err(RpcError {
                code: -32601,
                message: format!("method not found: {other}"),
            }),
        }
    }

    /// Creates or reopens a session and registers it. Reopening replays the
    /// conversation to the client, as the protocol requires.
    async fn start(&self, existing: Option<&SessionId>, cwd: &Path) -> Result<String, RpcError> {
        let cwd = cwd
            .canonicalize()
            .map_err(|e| RpcError::invalid(format!("cannot use {}: {e}", cwd.display())))?;
        let store = hx_app::session_store(&self.home);
        let mut session =
            hx_app::open_session(&store, existing, &cwd).map_err(RpcError::internal)?;
        let id = session.id().to_string();

        let mut options = self.options.clone();
        options.prompter = Some(Arc::new(PermissionPrompter {
            session_id: id.clone(),
            outbox: self.outbox.clone(),
            pending: Arc::clone(&self.pending),
        }));
        let runtime = hx_app::build(&self.home, &cwd, &options)
            .await
            .map_err(RpcError::internal)?;

        let mut updates = Updates {
            session_id: id.clone(),
            outbox: self.outbox.clone(),
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
            }),
        );
        Ok(id)
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

    /// A record appended while a turn runs: text arrives as deltas.
    fn live(&mut self, record: &Record) {
        if self.tool(record) {
            return;
        }
        if let Event::AssistantDelta { delta } = &record.event {
            match delta {
                Delta::Text { text } => self.chunk("agent_message_chunk", text),
                Delta::Thinking { text } => self.chunk("agent_thought_chunk", text),
                Delta::ToolUseStart { .. } | Delta::ToolInput { .. } => {}
            }
        }
    }

    /// A record from a session being loaded: whole messages, including the
    /// user's.
    fn replay(&mut self, record: &Record) {
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
        _question: String,
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
                    "toolCall": {"toolCallId": request.call_id},
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
