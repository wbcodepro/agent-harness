//! A local web interface.
//!
//! `zcode web` serves one page and a small JSON API on 127.0.0.1. Session
//! records reach the page as server-sent events, so it shows a turn as it
//! streams; approval questions are answered from the page.
//!
//! The server is reachable only from this machine, and every API call must
//! carry the token printed at startup, so another site open in the browser
//! cannot drive the agent.

use std::collections::{HashMap, VecDeque};
use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{Notify, broadcast, oneshot};
use zcode_agent::{Session, ToolRequest};
use zcode_app::{Options, Prompter, Runtime};
use zcode_kernel::BoxFuture;
use zcode_session::{Block, SessionId, TurnEndReason};

const PAGE: &str = include_str!("index.html");

/// What a page and its session share: the event stream and the approval
/// questions waiting for an answer.
struct Channel {
    events: broadcast::Sender<String>,
    approvals: Mutex<HashMap<u64, oneshot::Sender<Option<bool>>>>,
    next_approval: AtomicU64,
}

impl Channel {
    fn send(&self, event: Value) {
        // No page listening is fine: the log has everything.
        let _ = self.events.send(event.to_string());
    }
}

struct Entry {
    state: tokio::sync::Mutex<(Session, Runtime)>,
    channel: Arc<Channel>,
    cancel: Notify,
    busy: AtomicBool,
}

struct App {
    home: PathBuf,
    cwd: PathBuf,
    options: Options,
    token: String,
    sessions: Mutex<HashMap<String, Arc<Entry>>>,
}

/// A running server.
pub struct WebServer {
    pub addr: SocketAddr,
    pub token: String,
    task: tokio::task::JoinHandle<()>,
}

impl WebServer {
    /// The address to open in a browser.
    pub fn url(&self) -> String {
        format!("http://{}/?token={}", self.addr, self.token)
    }

    /// Runs until the server stops.
    pub async fn wait(self) {
        let _ = self.task.await;
    }
}

/// Starts serving on 127.0.0.1. `port` 0 picks a free one. New sessions
/// work in `cwd`.
pub async fn start(
    home: PathBuf,
    cwd: PathBuf,
    options: Options,
    port: u16,
) -> Result<WebServer, String> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .map_err(|e| format!("cannot listen on port {port}: {e}"))?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    let token = uuid::Uuid::new_v4().simple().to_string();
    let app = Arc::new(App {
        home,
        cwd,
        options,
        token: token.clone(),
        sessions: Mutex::new(HashMap::new()),
    });
    let api = Router::new()
        .route("/sessions", get(list).post(create))
        .route("/sessions/{id}/events", get(events))
        .route("/sessions/{id}/prompt", post(prompt))
        .route("/sessions/{id}/cancel", post(cancel))
        .route("/sessions/{id}/approval", post(approval))
        .layer(axum::middleware::from_fn_with_state(
            Arc::clone(&app),
            guard,
        ));
    let router = Router::new()
        .route("/", get(|| async { Html(PAGE) }))
        .nest("/api", api)
        .with_state(app);
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    Ok(WebServer { addr, token, task })
}

/// Rejects API calls without the token, and calls addressed to any host
/// name but this machine's (which is how a hostile page would reach a
/// local server through DNS rebinding).
async fn guard(
    State(app): State<Arc<App>>,
    Query(query): Query<HashMap<String, String>>,
    request: Request,
    next: Next,
) -> Response {
    let headers = request.headers();
    let host = headers
        .get("host")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let local = host.starts_with("127.0.0.1:") || host.starts_with("localhost:");
    let presented = headers
        .get("x-zcode-token")
        .and_then(|value| value.to_str().ok())
        .or(query.get("token").map(String::as_str));
    if !local || presented != Some(app.token.as_str()) {
        return (StatusCode::FORBIDDEN, "missing or wrong token").into_response();
    }
    next.run(request).await
}

type Failure = (StatusCode, String);

fn bad(message: impl Into<String>) -> Failure {
    (StatusCode::BAD_REQUEST, message.into())
}

fn failed(message: impl Into<String>) -> Failure {
    (StatusCode::INTERNAL_SERVER_ERROR, message.into())
}

struct PagePrompter(Arc<Channel>);

impl Prompter for PagePrompter {
    fn ask<'a>(
        &'a self,
        _request: &'a ToolRequest,
        question: String,
    ) -> BoxFuture<'a, Option<bool>> {
        Box::pin(async move {
            let id = self.0.next_approval.fetch_add(1, Ordering::Relaxed);
            let (answer, answered) = oneshot::channel();
            self.0
                .approvals
                .lock()
                .expect("approvals")
                .insert(id, answer);
            self.0
                .send(json!({"kind": "approval", "id": id, "question": question}));
            // A page that goes away without answering has not approved.
            answered.await.ok().flatten()
        })
    }
}

impl App {
    /// The loaded session with this id, loading or creating it as needed.
    async fn entry(&self, id: Option<&str>) -> Result<Arc<Entry>, Failure> {
        if let Some(id) = id
            && let Some(entry) = self.sessions.lock().expect("sessions").get(id)
        {
            return Ok(Arc::clone(entry));
        }
        let store = zcode_app::session_store(&self.home);
        let existing = id
            .map(SessionId::parse)
            .transpose()
            .map_err(|e| bad(e.to_string()))?;
        let cwd = match &existing {
            Some(id) => zcode_app::session_cwd(&store, id).map_err(bad)?,
            None => self.cwd.clone(),
        };
        let channel = Arc::new(Channel {
            events: broadcast::channel(4096).0,
            approvals: Mutex::new(HashMap::new()),
            next_approval: AtomicU64::new(1),
        });
        let mut options = self.options.clone();
        options.prompter = Some(Arc::new(PagePrompter(Arc::clone(&channel))));
        let runtime = zcode_app::build(&self.home, &cwd, &options)
            .await
            .map_err(failed)?;
        let mut session =
            zcode_app::open_session(&store, existing.as_ref(), &cwd).map_err(failed)?;
        let observer = Arc::clone(&channel);
        session.observe(move |record| observer.send(json!({"kind": "record", "record": record})));

        let id = session.id().to_string();
        let entry = Arc::new(Entry {
            state: tokio::sync::Mutex::new((session, runtime)),
            channel,
            cancel: Notify::new(),
            busy: AtomicBool::new(false),
        });
        self.sessions
            .lock()
            .expect("sessions")
            .insert(id, Arc::clone(&entry));
        Ok(entry)
    }
}

async fn list(State(app): State<Arc<App>>) -> Result<Json<Value>, Failure> {
    let headers = zcode_app::session_store(&app.home)
        .list()
        .map_err(|e| failed(e.to_string()))?;
    Ok(Json(
        headers
            .into_iter()
            .map(|header| json!({"id": header.id, "cwd": header.cwd}))
            .collect(),
    ))
}

async fn create(State(app): State<Arc<App>>) -> Result<Json<Value>, Failure> {
    let entry = app.entry(None).await?;
    let id = entry.state.lock().await.0.id().to_string();
    Ok(Json(json!({ "id": id })))
}

async fn events(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    _headers: HeaderMap,
) -> Result<impl IntoResponse, Failure> {
    let entry = app.entry(Some(&id)).await?;
    // Subscribe before reading the log, so nothing falls in between; the
    // page drops records it has already seen.
    let live = entry.channel.events.subscribe();
    let session = SessionId::parse(&id).map_err(|e| bad(e.to_string()))?;
    let stored = zcode_app::session_store(&app.home)
        .read(&session)
        .map_err(|e| failed(e.to_string()))?;
    let mut replay: VecDeque<String> = stored
        .iter()
        .map(|record| json!({"kind": "record", "record": record}).to_string())
        .collect();
    if entry.busy.load(Ordering::Relaxed) {
        replay.push_back(json!({"kind": "turn", "state": "started"}).to_string());
    }
    let stream =
        futures_util::stream::unfold((replay, live), |(mut replay, mut live)| async move {
            if let Some(next) = replay.pop_front() {
                return Some((
                    Ok::<_, Infallible>(SseEvent::default().data(next)),
                    (replay, live),
                ));
            }
            loop {
                match live.recv().await {
                    Ok(next) => return Some((Ok(SseEvent::default().data(next)), (replay, live))),
                    // A page that fell far behind skips ahead; reloading
                    // replays the log.
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => return None,
                }
            }
        });
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

#[derive(Deserialize)]
struct PromptBody {
    text: String,
}

async fn prompt(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    Json(body): Json<PromptBody>,
) -> Result<Json<Value>, Failure> {
    if body.text.trim().is_empty() {
        return Err(bad("the message is empty"));
    }
    let entry = app.entry(Some(&id)).await?;
    if entry.busy.swap(true, Ordering::SeqCst) {
        return Err((StatusCode::CONFLICT, "a turn is already running".into()));
    }
    tokio::spawn(async move {
        entry
            .channel
            .send(json!({"kind": "turn", "state": "started"}));
        let mut state = entry.state.lock().await;
        let (session, runtime) = &mut *state;
        let outcome = tokio::select! {
            result = runtime.agent.run_turn(session, vec![Block::text(body.text)]) => Some(result),
            _ = entry.cancel.notified() => None,
        };
        let error = match outcome {
            Some(Ok(())) => None,
            Some(Err(e)) => Some(e.to_string()),
            None => session
                .close_open_turn(TurnEndReason::Cancelled)
                .err()
                .map(|e| e.to_string()),
        };
        // Nobody is left to answer questions from a turn that is over.
        entry.channel.approvals.lock().expect("approvals").clear();
        entry.busy.store(false, Ordering::SeqCst);
        entry
            .channel
            .send(json!({"kind": "turn", "state": "ended", "error": error}));
    });
    Ok(Json(json!({"started": true})))
}

async fn cancel(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Failure> {
    let entry = app.entry(Some(&id)).await?;
    entry.cancel.notify_waiters();
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
struct ApprovalBody {
    id: u64,
    allow: bool,
}

async fn approval(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    Json(body): Json<ApprovalBody>,
) -> Result<Json<Value>, Failure> {
    let entry = app.entry(Some(&id)).await?;
    let waiting = entry
        .channel
        .approvals
        .lock()
        .expect("approvals")
        .remove(&body.id);
    match waiting {
        Some(answer) => {
            let _ = answer.send(Some(body.allow));
            Ok(Json(json!({})))
        }
        None => Err(bad("that question is no longer waiting")),
    }
}
