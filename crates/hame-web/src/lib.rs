//! A local web interface.
//!
//! `hame web` serves the built interface of `apps/ui` and a JSON API on
//! 127.0.0.1. Session records reach the page as server-sent events, so it
//! shows a turn as it streams; approval questions are answered from the
//! page.
//!
//! The server is reachable only from this machine, and every API call must
//! carry the token printed at startup, so another site open in the browser
//! cannot drive the agent.
//!
//! Images a session refers to are served from `/api/artifacts/<sha256>`,
//! only to a page that names a session referring to them.
//!
//! The API, route by route (all under `/api`, all needing the token):
//!
//! | Route | What |
//! |---|---|
//! | `GET /info` | version, default folder, recent folders, permission modes, models |
//! | `GET /sessions` | sessions, latest activity first, with title and status |
//! | `POST /sessions` | a new session, optionally in `cwd`, with `mode`, `provider`, `model` |
//! | `GET /sessions/{id}` | one session: title, folder, mode, model, usage, cost, context |
//! | `PATCH /sessions/{id}` | rename (`title`) or pin (`pinned`) |
//! | `DELETE /sessions/{id}` | delete the session's log |
//! | `PUT /sessions/{id}/settings` | change `mode`, `provider`, `model` between turns |
//! | `GET /sessions/{id}/events` | the event stream; `after` or `Last-Event-ID` resumes |
//! | `POST /sessions/{id}/prompt` | start a turn with `text`, uploaded `images` and `files` |
//! | `POST /sessions/{id}/steer` | queue a message for the running turn |
//! | `POST /sessions/{id}/steer/withdraw` | take a queued message back |
//! | `POST /sessions/{id}/cancel` | stop the running turn |
//! | `POST /sessions/{id}/approval` | answer an approval question |
//! | `POST /sessions/{id}/attachments` | upload an image or a file (`?name=`) |
//! | `GET /artifacts/{sha256}?session=` | an image or file the session refers to |

mod assets;
mod meta;
mod overview;

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::{Path as FsPath, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use hame_agent::{Session, Steering, ToolRequest};
use hame_app::{Options, Prompter, RuleAction, Runtime, SandboxMode};
use hame_kernel::BoxFuture;
use hame_session::{ArtifactRef, Block, Event, Record, SessionId, TurnEndReason};
use hame_tools::Effect;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{Notify, broadcast, oneshot};

pub use meta::Mode;
use meta::{MetaStore, SessionMeta};

/// The largest upload accepted, in bytes.
pub const MAX_UPLOAD_BYTES: usize = 20 * 1024 * 1024;

/// Where uploaded files that are not images go, inside the session's
/// working folder.
const ATTACHMENTS_DIR: &str = ".hame/attachments";

/// One event for the pages watching a session. Records carry their `seq`,
/// which becomes the SSE event id, so a page that reconnects resumes after
/// the last record it saw.
#[derive(Clone)]
struct Outgoing {
    seq: Option<u64>,
    data: Arc<str>,
}

impl Outgoing {
    fn event(event: &Value) -> Self {
        Self {
            seq: None,
            data: event.to_string().into(),
        }
    }

    fn record(record: &Record) -> Self {
        Self {
            seq: Some(record.seq),
            data: json!({"kind": "record", "record": record})
                .to_string()
                .into(),
        }
    }

    fn sse(&self) -> SseEvent {
        let event = SseEvent::default().data(&*self.data);
        match self.seq {
            Some(seq) => event.id(seq.to_string()),
            None => event,
        }
    }
}

/// An approval question waiting for an answer.
struct Waiting {
    answer: oneshot::Sender<Option<bool>>,
    /// The question as it was sent, for pages that connect later.
    event: Value,
    tool: String,
}

/// What a page and its session share: the event stream, the approval
/// questions waiting for an answer, and the answers that hold for the rest
/// of the session.
struct Channel {
    events: broadcast::Sender<Outgoing>,
    approvals: Mutex<BTreeMap<u64, Waiting>>,
    next_approval: AtomicU64,
    /// Tools the user allowed for the rest of this session.
    always: Mutex<HashSet<String>>,
    mode: Mutex<Mode>,
}

impl Channel {
    fn send(&self, event: Value) {
        // No page listening is fine: the log has everything.
        let _ = self.events.send(Outgoing::event(&event));
    }

    fn send_steering(&self, steering: &Steering) {
        self.send(json!({"kind": "steering", "queued": steering.pending()}));
    }
}

#[derive(Default)]
struct Uploads {
    images: HashMap<String, ArtifactRef>,
    files: HashSet<String>,
}

struct Entry {
    cwd: PathBuf,
    state: tokio::sync::Mutex<(Session, Runtime)>,
    channel: Arc<Channel>,
    cancel: Notify,
    busy: AtomicBool,
    steering: Steering,
    /// The model the runtime talks to, readable while a turn holds `state`.
    model: Mutex<Option<String>>,
    uploads: Mutex<Uploads>,
    /// Why the last turn failed, for a page that connects after it did.
    last_error: Mutex<Option<String>>,
}

struct App {
    home: PathBuf,
    cwd: PathBuf,
    options: Options,
    token: String,
    meta: MetaStore,
    sessions: Mutex<HashMap<String, Arc<Entry>>>,
    /// Held while a session is loaded, so two requests do not both open it.
    loading: tokio::sync::Mutex<()>,
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
/// work in `cwd` unless the page names another folder.
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
        meta: MetaStore::new(&home),
        home,
        cwd,
        options,
        token: token.clone(),
        sessions: Mutex::new(HashMap::new()),
        loading: tokio::sync::Mutex::new(()),
    });
    let api = Router::new()
        .route("/info", get(info))
        .route("/sessions", get(list).post(create))
        .route(
            "/sessions/{id}",
            get(detail).patch(rename).delete(delete_session),
        )
        .route("/sessions/{id}/settings", put(settings))
        .route("/sessions/{id}/events", get(events))
        .route("/sessions/{id}/prompt", post(prompt))
        .route("/sessions/{id}/steer", post(steer))
        .route("/sessions/{id}/steer/withdraw", post(withdraw))
        .route("/sessions/{id}/cancel", post(cancel))
        .route("/sessions/{id}/approval", post(approval))
        .route(
            "/sessions/{id}/attachments",
            post(attach).layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES)),
        )
        .route("/artifacts/{sha256}", get(artifact))
        .layer(axum::middleware::from_fn_with_state(
            Arc::clone(&app),
            guard,
        ));
    let router = Router::new()
        .nest("/api", api)
        .fallback(assets::serve)
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
        .get("x-hame-token")
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

fn conflict(message: impl Into<String>) -> Failure {
    (StatusCode::CONFLICT, message.into())
}

fn not_found(message: impl Into<String>) -> Failure {
    (StatusCode::NOT_FOUND, message.into())
}

fn parse_body<T: for<'de> Deserialize<'de> + Default>(body: &Bytes) -> Result<T, Failure> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(T::default());
    }
    serde_json::from_slice(body).map_err(|e| bad(format!("invalid request body: {e}")))
}

fn effect_name(effect: Effect) -> &'static str {
    match effect {
        Effect::ReadOnly => "read-only",
        Effect::Write => "write",
        Effect::Execute => "execute",
        Effect::Control => "control",
    }
}

struct PagePrompter(Arc<Channel>);

impl Prompter for PagePrompter {
    fn ask<'a>(
        &'a self,
        request: &'a ToolRequest,
        question: String,
    ) -> BoxFuture<'a, Option<bool>> {
        Box::pin(async move {
            let mode = *self.0.mode.lock().expect("mode");
            if mode == Mode::AcceptEdits && request.effect == Effect::Write {
                return Some(true);
            }
            if self
                .0
                .always
                .lock()
                .expect("always")
                .contains(&request.name)
            {
                return Some(true);
            }
            let id = self.0.next_approval.fetch_add(1, Ordering::Relaxed);
            let (answer, answered) = oneshot::channel();
            let event = json!({
                "kind": "approval",
                "id": id,
                "question": question,
                "tool": request.name,
                "call_id": request.call_id,
                "input": request.input,
                "effect": effect_name(request.effect),
                "scope": format!("{} for the rest of this session", request.name),
            });
            self.0.approvals.lock().expect("approvals").insert(
                id,
                Waiting {
                    answer,
                    event: event.clone(),
                    tool: request.name.clone(),
                },
            );
            self.0.send(event);
            // A page that goes away without answering has not approved.
            answered.await.ok().flatten()
        })
    }
}

impl App {
    fn store(&self) -> hame_session::SessionStore {
        hame_app::session_store(&self.home)
    }

    /// The options a session's runtime is built with: the server's, with
    /// this page as the one asked, the session's steering queue, and its
    /// permission mode and model.
    fn options_for(
        &self,
        meta: &SessionMeta,
        channel: &Arc<Channel>,
        steering: &Steering,
    ) -> Options {
        let mut options = self.options.clone();
        options.prompter = Some(Arc::new(PagePrompter(Arc::clone(channel))));
        options.steering = Some(steering.clone());
        if meta.provider.is_some() {
            options.provider = meta.provider.clone();
        }
        if meta.model.is_some() {
            options.model = meta.model.clone();
        }
        match meta.mode {
            Mode::Default => {}
            Mode::ReadOnly => {
                options.sandbox = Some(SandboxMode::ReadOnly);
                options.approval_default = Some(RuleAction::Deny);
            }
            Mode::Ask | Mode::AcceptEdits => options.approval_default = Some(RuleAction::Ask),
            Mode::FullAccess => {
                options.sandbox = Some(SandboxMode::DangerFullAccess);
                options.yes = true;
            }
        }
        options
    }

    fn loaded(&self, id: &str) -> Option<Arc<Entry>> {
        self.sessions.lock().expect("sessions").get(id).cloned()
    }

    /// The loaded session with this id, loading it as needed.
    async fn entry(&self, id: &str) -> Result<Arc<Entry>, Failure> {
        if let Some(entry) = self.loaded(id) {
            return Ok(entry);
        }
        let session = SessionId::parse(id).map_err(|e| bad(e.to_string()))?;
        let _loading = self.loading.lock().await;
        if let Some(entry) = self.loaded(id) {
            return Ok(entry);
        }
        let store = self.store();
        let cwd = hame_app::session_cwd(&store, &session).map_err(|e| {
            if e.contains("not found") {
                not_found(e)
            } else {
                bad(e)
            }
        })?;
        let meta = self.meta.get(id);
        self.load(Some(&session), cwd, meta).await
    }

    /// Builds the runtime and opens (or, with no id, creates) a session.
    async fn load(
        &self,
        existing: Option<&SessionId>,
        cwd: PathBuf,
        meta: SessionMeta,
    ) -> Result<Arc<Entry>, Failure> {
        let channel = Arc::new(Channel {
            events: broadcast::channel(4096).0,
            approvals: Mutex::new(BTreeMap::new()),
            next_approval: AtomicU64::new(1),
            always: Mutex::new(HashSet::new()),
            mode: Mutex::new(meta.mode),
        });
        let steering = Steering::default();
        let options = self.options_for(&meta, &channel, &steering);
        let runtime = hame_app::build(&self.home, &cwd, &options)
            .await
            .map_err(failed)?;
        let store = self.store();
        let mut session = hame_app::open_session(&store, existing, &cwd).map_err(|e| {
            if e.contains("open in another process") {
                conflict(e)
            } else {
                failed(e)
            }
        })?;
        let observer = Arc::clone(&channel);
        let queue = steering.clone();
        session.observe(move |record| {
            let _ = observer.events.send(Outgoing::record(record));
            // A queued message the turn just took is no longer waiting.
            if matches!(record.event, Event::UserMessage { .. }) {
                observer.send_steering(&queue);
            }
        });

        let id = session.id().to_string();
        let entry = Arc::new(Entry {
            cwd,
            model: Mutex::new(runtime.model.clone()),
            state: tokio::sync::Mutex::new((session, runtime)),
            channel,
            cancel: Notify::new(),
            busy: AtomicBool::new(false),
            steering,
            uploads: Mutex::new(Uploads::default()),
            last_error: Mutex::new(None),
        });
        self.sessions
            .lock()
            .expect("sessions")
            .insert(id, Arc::clone(&entry));
        Ok(entry)
    }

    fn status(&self, id: &str, outcome: overview::Outcome) -> &'static str {
        if let Some(entry) = self.loaded(id) {
            if !entry
                .channel
                .approvals
                .lock()
                .expect("approvals")
                .is_empty()
            {
                return "needs_you";
            }
            if entry.busy.load(Ordering::Relaxed) {
                return "running";
            }
        }
        match outcome {
            overview::Outcome::Idle => "idle",
            overview::Outcome::Failed => "failed",
            overview::Outcome::Verified => "verified",
            overview::Outcome::NotVerified => "not_verified",
        }
    }

    /// What the list and the header show about a session.
    fn describe(&self, id: &str, cwd: &str, records: &[Record], meta: &SessionMeta) -> Value {
        let seen = overview::overview(records);
        json!({
            "id": id,
            "cwd": cwd,
            "title": meta.title.clone().or(seen.title),
            "named": meta.title.is_some(),
            "pinned": meta.pinned,
            "status": self.status(id, seen.outcome),
            "outcome": seen.outcome,
            "created": seen.created,
            "last_activity": seen.last_activity,
            "turns": seen.turns,
            "usage": seen.usage,
            "mode": meta.mode,
        })
    }
}

/// Settles an absolute folder the page asked for.
fn folder(raw: &str) -> Result<PathBuf, Failure> {
    let path = PathBuf::from(raw);
    if !path.is_absolute() {
        return Err(bad("the folder must be an absolute path"));
    }
    let real = hame_kernel::real_path(&path)
        .map_err(|e| bad(format!("cannot use {raw} as the working folder: {e}")))?;
    if !real.is_dir() {
        return Err(bad(format!("{raw} is not a folder")));
    }
    Ok(real)
}

async fn info(State(app): State<Arc<App>>) -> Result<Json<Value>, Failure> {
    let mut folders = vec![app.cwd.to_string_lossy().into_owned()];
    let mut headers = app.store().list().map_err(|e| failed(e.to_string()))?;
    // Latest sessions first: their folders are the likeliest next ones.
    headers.reverse();
    for header in headers {
        if !folders.contains(&header.cwd) && FsPath::new(&header.cwd).is_dir() {
            folders.push(header.cwd);
        }
        if folders.len() >= 12 {
            break;
        }
    }
    let modes: Vec<Value> = Mode::ALL
        .into_iter()
        .map(|mode| json!({"name": mode, "label": mode.label(), "description": mode.description()}))
        .collect();
    Ok(Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "cwd": app.cwd,
        "folders": folders,
        "modes": modes,
        "models": models(&app),
        "limits": {"upload_bytes": MAX_UPLOAD_BYTES},
    })))
}

/// The providers hame can use here, whether a key for each is set, and the
/// model each would use. Never the keys themselves.
fn models(app: &App) -> Value {
    if app.options.llm.is_some() {
        return json!({"fixed": true, "current": null, "providers": []});
    }
    let (Ok(config), Ok(secrets)) = (
        hame_app::load_config(&app.home),
        hame_app::load_secrets(&app.home, &app.cwd),
    ) else {
        return json!({"fixed": false, "current": null, "providers": []});
    };
    let current = hame_app::choose_model(&config, &secrets, &app.options)
        .ok()
        .map(|chosen| json!({"provider": chosen.provider.name(), "model": chosen.model}));
    let providers: Vec<Value> = ["deepseek", "anthropic", "openai"]
        .into_iter()
        .map(|name| {
            let options = Options {
                provider: Some(name.to_string()),
                ..Options::default()
            };
            match hame_app::choose_model(&config, &secrets, &options) {
                Ok(chosen) => json!({"name": name, "configured": true, "model": chosen.model}),
                Err(_) => json!({"name": name, "configured": false, "model": null}),
            }
        })
        .collect();
    json!({"fixed": false, "current": current, "providers": providers})
}

async fn list(State(app): State<Arc<App>>) -> Result<Json<Value>, Failure> {
    let store = app.store();
    let headers = store.list().map_err(|e| failed(e.to_string()))?;
    let meta = app.meta.all();
    let mut rows: Vec<Value> = headers
        .into_iter()
        .map(|header| {
            let id = header.id.to_string();
            let records = store.read(&header.id).unwrap_or_default();
            let meta = meta.get(&id).cloned().unwrap_or_default();
            app.describe(&id, &header.cwd, &records, &meta)
        })
        .collect();
    rows.sort_by_key(|row| std::cmp::Reverse(row["last_activity"].as_u64().unwrap_or(0)));
    Ok(Json(Value::Array(rows)))
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct CreateBody {
    cwd: Option<String>,
    mode: Option<String>,
    provider: Option<String>,
    model: Option<String>,
}

fn parse_mode(name: &str) -> Result<Mode, Failure> {
    Mode::parse(name).ok_or_else(|| bad(format!("unknown permission mode {name:?}")))
}

fn check_model(provider: &Option<String>, model: &Option<String>) -> Result<(), Failure> {
    if let Some(provider) = provider {
        hame_app::Provider::parse(provider).map_err(bad)?;
    }
    if let Some(model) = model
        && (model.trim().is_empty() || model.len() > 200 || model.chars().any(char::is_control))
    {
        return Err(bad("invalid model name"));
    }
    Ok(())
}

async fn create(State(app): State<Arc<App>>, body: Bytes) -> Result<Json<Value>, Failure> {
    let body: CreateBody = parse_body(&body)?;
    let cwd = match &body.cwd {
        Some(raw) => folder(raw)?,
        None => app.cwd.clone(),
    };
    check_model(&body.provider, &body.model)?;
    let meta = SessionMeta {
        mode: body
            .mode
            .as_deref()
            .map(parse_mode)
            .transpose()?
            .unwrap_or_default(),
        provider: body.provider,
        model: body.model,
        ..SessionMeta::default()
    };
    let entry = {
        let _loading = app.loading.lock().await;
        app.load(None, cwd, meta.clone()).await?
    };
    let id = entry.state.lock().await.0.id().to_string();
    app.meta.update(&id, |_| Some(meta)).map_err(failed)?;
    Ok(Json(json!({ "id": id })))
}

async fn detail(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Failure> {
    let entry = app.entry(&id).await?;
    let session = SessionId::parse(&id).map_err(|e| bad(e.to_string()))?;
    let records = app
        .store()
        .read(&session)
        .map_err(|e| failed(e.to_string()))?;
    let meta = app.meta.get(&id);
    let mut described = app.describe(&id, &entry.cwd.to_string_lossy(), &records, &meta);
    let model = entry.model.lock().expect("model").clone();
    let compact_at = hame_app::load_config(&app.home)
        .map(|config| config.context.compact_at_tokens)
        .ok();
    let approvals: Vec<Value> = entry
        .channel
        .approvals
        .lock()
        .expect("approvals")
        .values()
        .map(|waiting| waiting.event.clone())
        .collect();
    let extra = json!({
        "busy": entry.busy.load(Ordering::Relaxed),
        "provider": meta.provider,
        "model": model,
        // Only a price from the tariff counts; an unknown price is not zero.
        "cost_usd": hame_verify::budget::cost_of(&records, model.as_deref()),
        "context": {
            "tokens": hame_context::context_tokens(&records),
            "compact_at": compact_at,
        },
        "queued": entry.steering.pending(),
        "approvals": approvals,
    });
    if let (Some(described), Value::Object(extra)) = (described.as_object_mut(), extra) {
        described.extend(extra);
    }
    Ok(Json(described))
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RenameBody {
    /// `null` or empty goes back to the title from the first message.
    #[serde(default, with = "double_option")]
    title: Option<Option<String>>,
    pinned: Option<bool>,
}

/// Tells a field that is absent from one that is `null`.
mod double_option {
    use serde::{Deserialize, Deserializer};

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Option<String>>, D::Error> {
        Ok(Some(Option::deserialize(deserializer)?))
    }
}

async fn rename(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<Value>, Failure> {
    let body: RenameBody = parse_body(&body)?;
    let session = SessionId::parse(&id).map_err(|e| bad(e.to_string()))?;
    let store = app.store();
    let records = store
        .read(&session)
        .map_err(|_| not_found("no such session"))?;
    let title = match body.title {
        Some(Some(title)) => {
            let title = title.trim().to_string();
            if title.chars().count() > 200 || title.chars().any(char::is_control) {
                return Err(bad(
                    "a title has at most 200 characters and no control characters",
                ));
            }
            Some((!title.is_empty()).then_some(title))
        }
        Some(None) => Some(None),
        None => None,
    };
    let meta = app
        .meta
        .update(&id, |mut meta| {
            if let Some(title) = title {
                meta.title = title;
            }
            if let Some(pinned) = body.pinned {
                meta.pinned = pinned;
            }
            Some(meta)
        })
        .map_err(failed)?;
    let cwd = match records.first().map(|record| &record.event) {
        Some(Event::Session(header)) => header.cwd.clone(),
        _ => String::new(),
    };
    let described = app.describe(&id, &cwd, &records, &meta);
    if let Some(entry) = app.loaded(&id) {
        entry
            .channel
            .send(json!({"kind": "session", "session": described}));
    }
    Ok(Json(described))
}

async fn delete_session(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Failure> {
    let session = SessionId::parse(&id).map_err(|e| bad(e.to_string()))?;
    let _loading = app.loading.lock().await;
    if let Some(entry) = app.loaded(&id) {
        if entry.busy.load(Ordering::SeqCst) {
            return Err(conflict("stop the running turn first"));
        }
        app.sessions.lock().expect("sessions").remove(&id);
        let state = entry.state.lock().await;
        state.1.computer.shutdown().await;
        // Releases the session's lock before anything is removed.
        drop(state);
        drop(entry);
    }
    let store = app.store();
    // Taking the lock proves no other hame (a terminal, an editor) has the
    // session open.
    match store.open(&session) {
        Ok((_, writer)) => drop(writer),
        Err(hame_session::SessionError::NotFound(_)) => return Err(not_found("no such session")),
        Err(hame_session::SessionError::Locked(_)) => {
            return Err(conflict("the session is open in another hame process"));
        }
        Err(e) => return Err(failed(e.to_string())),
    }
    std::fs::remove_dir_all(store.root().join(session.as_str()))
        .map_err(|e| failed(format!("cannot delete the session: {e}")))?;
    app.meta.update(&id, |_| None).map_err(failed)?;
    Ok(Json(json!({})))
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct SettingsBody {
    mode: Option<String>,
    provider: Option<String>,
    model: Option<String>,
}

/// Changes the permission mode or the model. The agent is rebuilt with the
/// new settings, so this waits for a turn to end.
async fn settings(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<Value>, Failure> {
    let body: SettingsBody = parse_body(&body)?;
    let mode = body.mode.as_deref().map(parse_mode).transpose()?;
    check_model(&body.provider, &body.model)?;
    let entry = app.entry(&id).await?;
    if entry.busy.load(Ordering::SeqCst) {
        return Err(conflict(
            "settings change between turns; wait or stop the turn",
        ));
    }
    let mut state = entry.state.lock().await;
    let mut meta = app.meta.get(&id);
    if let Some(mode) = mode {
        meta.mode = mode;
    }
    if body.provider.is_some() {
        meta.provider = body.provider;
    }
    if body.model.is_some() {
        meta.model = body.model;
    }
    let options = app.options_for(&meta, &entry.channel, &entry.steering);
    let runtime = hame_app::build(&app.home, &entry.cwd, &options)
        .await
        .map_err(bad)?;
    let (session, old) = &mut *state;
    let before = old.model.clone();
    let after = runtime.model.clone();
    let old = std::mem::replace(old, runtime);
    old.computer.shutdown().await;
    if let Some(model) = &after
        && before.as_ref() != Some(model)
    {
        // Prices and later escalation read the model from the log.
        session
            .append(Event::ModelSwitched {
                model: model.clone(),
            })
            .map_err(|e| failed(e.to_string()))?;
    }
    drop(state);
    *entry.model.lock().expect("model") = after.clone();
    *entry.channel.mode.lock().expect("mode") = meta.mode;
    app.meta
        .update(&id, |_| Some(meta.clone()))
        .map_err(failed)?;
    entry.channel.send(json!({
        "kind": "settings",
        "mode": meta.mode,
        "provider": meta.provider,
        "model": after,
    }));
    Ok(Json(
        json!({"mode": meta.mode, "provider": meta.provider, "model": after}),
    ))
}

#[derive(Deserialize)]
struct EventsQuery {
    after: Option<u64>,
}

async fn events(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    Query(query): Query<EventsQuery>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, Failure> {
    let entry = app.entry(&id).await?;
    // What the page already has: `after` when it asks, the id of the last
    // event it saw when the browser reconnects by itself.
    let resumed = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok());
    let after = query.after.max(resumed);
    // Subscribe before reading the log, so nothing falls in between; what
    // is in both is sent once.
    let live = entry.channel.events.subscribe();
    let session = SessionId::parse(&id).map_err(|e| bad(e.to_string()))?;
    let store = app.store();
    let stored = store.read(&session).map_err(|e| failed(e.to_string()))?;
    let mut replay: VecDeque<Outgoing> = stored
        .iter()
        .filter(|record| after.is_none_or(|after| record.seq > after))
        .map(Outgoing::record)
        .collect();
    let last_seq = stored.last().map(|record| record.seq).max(after);
    if after.is_none() {
        let missing: Vec<Value> = store
            .check_artifacts(&session)
            .unwrap_or_default()
            .into_iter()
            .map(|(artifact, error)| {
                json!({"sha256": artifact.sha256, "mime": artifact.mime, "error": error.to_string()})
            })
            .collect();
        if !missing.is_empty() {
            replay.push_back(Outgoing::event(&json!({
                "kind": "warning",
                "text": format!(
                    "{} image(s) or file(s) of this session are missing from the store; the \
                     model sees a note in their place.",
                    missing.len()
                ),
                "missing_artifacts": missing,
            })));
        }
    }
    if entry.busy.load(Ordering::SeqCst) {
        replay.push_back(Outgoing::event(
            &json!({"kind": "turn", "state": "started"}),
        ));
    } else if let Some(error) = entry.last_error.lock().expect("last error").clone() {
        replay.push_back(Outgoing::event(
            &json!({"kind": "turn", "state": "ended", "error": error}),
        ));
    }
    for waiting in entry.channel.approvals.lock().expect("approvals").values() {
        replay.push_back(Outgoing::event(&waiting.event));
    }
    let queued = entry.steering.pending();
    if !queued.is_empty() {
        replay.push_back(Outgoing::event(
            &json!({"kind": "steering", "queued": queued}),
        ));
    }
    drop(entry);
    let stream = futures_util::stream::unfold(
        (replay, live, last_seq),
        |(mut replay, mut live, mut last_seq)| async move {
            if let Some(next) = replay.pop_front() {
                return Some((Ok::<_, Infallible>(next.sse()), (replay, live, last_seq)));
            }
            loop {
                match live.recv().await {
                    Ok(next) => {
                        if let Some(seq) = next.seq {
                            if last_seq.is_some_and(|last| seq <= last) {
                                continue;
                            }
                            last_seq = Some(seq);
                        }
                        return Some((Ok(next.sse()), (replay, live, last_seq)));
                    }
                    // A page that fell behind is cut off; it reconnects and
                    // resumes after the last record it has.
                    Err(broadcast::error::RecvError::Lagged(_))
                    | Err(broadcast::error::RecvError::Closed) => return None,
                }
            }
        },
    );
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

#[derive(Deserialize)]
struct PromptBody {
    text: String,
    /// Images uploaded to this session, by sha256.
    #[serde(default)]
    images: Vec<String>,
    /// Files uploaded to this session, by the path the upload returned.
    #[serde(default)]
    files: Vec<String>,
}

/// The content of a message: its text, a note naming attached files, and
/// attached images.
fn message(entry: &Entry, body: PromptBody) -> Result<Vec<Block>, Failure> {
    let uploads = entry.uploads.lock().expect("uploads");
    let mut text = body.text;
    if !body.files.is_empty() {
        for file in &body.files {
            if !uploads.files.contains(file) {
                return Err(bad(format!("{file} was not uploaded to this session")));
            }
        }
        text.push_str("\n\nAttached files (in the working folder): ");
        text.push_str(&body.files.join(", "));
    }
    let mut blocks = vec![Block::text(text.trim().to_string())];
    for sha256 in &body.images {
        let artifact = uploads
            .images
            .get(sha256)
            .ok_or_else(|| bad("that image was not uploaded to this session"))?;
        blocks.push(Block::Image {
            artifact: artifact.clone(),
        });
    }
    Ok(blocks)
}

async fn prompt(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    Json(body): Json<PromptBody>,
) -> Result<Json<Value>, Failure> {
    if body.text.trim().is_empty() && body.images.is_empty() && body.files.is_empty() {
        return Err(bad("the message is empty"));
    }
    let entry = app.entry(&id).await?;
    let content = message(&entry, body)?;
    if entry.busy.swap(true, Ordering::SeqCst) {
        return Err(conflict("a turn is already running"));
    }
    *entry.last_error.lock().expect("last error") = None;
    tokio::spawn(run_turns(entry, content));
    Ok(Json(json!({"started": true})))
}

/// Runs a turn, then another for messages queued too late for it.
async fn run_turns(entry: Arc<Entry>, mut content: Vec<Block>) {
    loop {
        entry
            .channel
            .send(json!({"kind": "turn", "state": "started"}));
        let mut state = entry.state.lock().await;
        let (session, runtime) = &mut *state;
        let outcome = tokio::select! {
            result = runtime.agent.run_turn(session, content) => Some(result),
            _ = entry.cancel.notified() => None,
        };
        let error = match outcome {
            Some(Ok(())) => None,
            Some(Err(e)) => Some(e.to_string()),
            None => {
                // Stopping drops what was queued for the stopped turn.
                entry.steering.take();
                entry.channel.send_steering(&entry.steering);
                session
                    .close_open_turn(TurnEndReason::Cancelled)
                    .err()
                    .map(|e| e.to_string())
            }
        };
        drop(state);
        // Nobody is left to answer questions from a turn that is over.
        entry.channel.approvals.lock().expect("approvals").clear();
        let late = entry.steering.take();
        if error.is_none() && !late.is_empty() {
            entry.channel.send_steering(&entry.steering);
            entry
                .channel
                .send(json!({"kind": "turn", "state": "ended", "error": null}));
            content = vec![Block::text(late.join("\n\n"))];
            continue;
        }
        *entry.last_error.lock().expect("last error") = error.clone();
        entry.busy.store(false, Ordering::SeqCst);
        entry
            .channel
            .send(json!({"kind": "turn", "state": "ended", "error": error}));
        return;
    }
}

#[derive(Deserialize)]
struct SteerBody {
    text: String,
}

/// Queues a message for the running turn. The agent hands it to the model
/// at its next step; one queued as the turn ends starts the next turn.
async fn steer(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    Json(body): Json<SteerBody>,
) -> Result<Json<Value>, Failure> {
    let text = body.text.trim();
    if text.is_empty() {
        return Err(bad("the message is empty"));
    }
    let entry = app.entry(&id).await?;
    if !entry.busy.load(Ordering::SeqCst) {
        return Err(conflict("no turn is running; send the message as a prompt"));
    }
    entry.steering.push(text);
    entry.channel.send_steering(&entry.steering);
    Ok(Json(json!({"queued": entry.steering.pending()})))
}

#[derive(Deserialize)]
struct WithdrawBody {
    index: usize,
    text: String,
}

async fn withdraw(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    Json(body): Json<WithdrawBody>,
) -> Result<Json<Value>, Failure> {
    let entry = app.entry(&id).await?;
    if !entry.steering.withdraw(body.index, &body.text) {
        return Err(conflict("that message was already handed to the agent"));
    }
    entry.channel.send_steering(&entry.steering);
    Ok(Json(json!({"queued": entry.steering.pending()})))
}

async fn cancel(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Failure> {
    let entry = app.entry(&id).await?;
    entry.cancel.notify_waiters();
    Ok(Json(json!({})))
}

/// A file name safe to create: letters, digits, `.`, `_` and `-`, not
/// starting with a dot, at most 100 characters.
fn safe_name(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim_start_matches('.');
    let name: String = trimmed
        .chars()
        .rev()
        .take(100)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if name.is_empty() {
        "attachment".into()
    } else {
        name
    }
}

#[derive(Deserialize)]
struct AttachQuery {
    name: Option<String>,
}

/// Takes an uploaded image or file for the next message. Images go to the
/// artifact store, so the model sees them; other files are written to
/// `.hame/attachments` in the session's folder, where its tools can read
/// them.
async fn attach(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    Query(query): Query<AttachQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, Failure> {
    if body.is_empty() {
        return Err(bad("the upload is empty"));
    }
    let entry = app.entry(&id).await?;
    let mime = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/octet-stream")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if mime.starts_with("image/") {
        let mut artifact = app
            .store()
            .artifacts()
            .put(&body, &mime)
            .map_err(|e| bad(e.to_string()))?;
        if !artifact.is_image() {
            return Err(bad("that is not a PNG, JPEG, GIF or WebP image"));
        }
        artifact.name = query.name.as_deref().map(safe_name);
        entry
            .uploads
            .lock()
            .expect("uploads")
            .images
            .insert(artifact.sha256.clone(), artifact.clone());
        return Ok(Json(json!({"kind": "image", "artifact": artifact})));
    }
    let name = safe_name(query.name.as_deref().unwrap_or("attachment"));
    let dir = entry.cwd.join(ATTACHMENTS_DIR);
    std::fs::create_dir_all(&dir)
        .map_err(|e| failed(format!("cannot create {}: {e}", dir.display())))?;
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem.to_string(), format!(".{extension}")),
        _ => (name.clone(), String::new()),
    };
    let mut chosen = name.clone();
    let mut file = None;
    for n in 1..1000 {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dir.join(&chosen))
        {
            Ok(created) => {
                file = Some(created);
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                chosen = format!("{stem}-{n}{extension}");
            }
            Err(e) => return Err(failed(format!("cannot save the upload: {e}"))),
        }
    }
    let mut file = file.ok_or_else(|| failed("too many uploads with that name"))?;
    std::io::Write::write_all(&mut file, &body)
        .map_err(|e| failed(format!("cannot save the upload: {e}")))?;
    let path = format!("{ATTACHMENTS_DIR}/{chosen}");
    entry
        .uploads
        .lock()
        .expect("uploads")
        .files
        .insert(path.clone());
    Ok(Json(
        json!({"kind": "file", "path": path, "bytes": body.len()}),
    ))
}

/// Serves the bytes of an artifact, for an `<img>` on the page.
///
/// The id must be a well-formed hash, and the session named by the
/// `session` query parameter must refer to it: the page can only fetch what
/// it is showing. The type sent is the one the log recorded, which the
/// store checked against the bytes for images; anything that is not an
/// image is sent as a download, never rendered on this origin.
async fn artifact(
    State(app): State<Arc<App>>,
    Path(sha256): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Response, Failure> {
    let well_formed = sha256.len() == 64
        && sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if !well_formed {
        return Err(bad("not an artifact id"));
    }
    let session = query
        .get("session")
        .ok_or_else(|| bad("name the session the artifact belongs to"))?;
    let session = SessionId::parse(session).map_err(|e| bad(e.to_string()))?;
    let store = app.store();
    let missing = || not_found("no such artifact in this session");
    let records = store.read(&session).map_err(|_| missing())?;
    let artifact = hame_session::session_artifacts(&records)
        .into_iter()
        .find(|artifact| artifact.sha256 == sha256)
        .ok_or_else(missing)?;
    let bytes = match store.artifacts().get(&artifact) {
        Ok(bytes) => bytes,
        Err(hame_session::artifact::ArtifactError::Missing(_)) => {
            return Err(not_found("the artifact is missing from the store"));
        }
        Err(e) => return Err(failed(e.to_string())),
    };
    let mime = if hame_session::artifact::KNOWN_MIME.contains(&artifact.mime.as_str()) {
        artifact.mime.as_str()
    } else {
        "application/octet-stream"
    };
    let mut response = bytes.into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(mime).unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; sandbox"),
    );
    // Content-addressed: the bytes behind this id never change.
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=31536000, immutable"),
    );
    if !artifact.is_image() {
        headers.insert(
            header::CONTENT_DISPOSITION,
            HeaderValue::from_static("attachment"),
        );
    }
    Ok(response)
}

#[derive(Deserialize)]
struct ApprovalBody {
    id: u64,
    allow: bool,
    /// Also allow this tool for the rest of the session.
    #[serde(default)]
    always: bool,
}

async fn approval(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    Json(body): Json<ApprovalBody>,
) -> Result<Json<Value>, Failure> {
    let entry = app.entry(&id).await?;
    let waiting = entry
        .channel
        .approvals
        .lock()
        .expect("approvals")
        .remove(&body.id);
    match waiting {
        Some(waiting) => {
            if body.allow && body.always {
                entry
                    .channel
                    .always
                    .lock()
                    .expect("always")
                    .insert(waiting.tool);
            }
            let _ = waiting.answer.send(Some(body.allow));
            entry.channel.send(json!({
                "kind": "approval_resolved",
                "id": body.id,
                "allow": body.allow,
            }));
            Ok(Json(json!({})))
        }
        None => Err(bad("that question is no longer waiting")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upload_names_cannot_leave_the_attachments_folder() {
        assert_eq!(safe_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_name("..\\x\\.bashrc"), "bashrc");
        assert_eq!(safe_name("my report (1).pdf"), "my_report__1_.pdf");
        assert_eq!(safe_name("..."), "attachment");
        assert_eq!(safe_name(""), "attachment");
        assert_eq!(safe_name(&"a".repeat(300)).len(), 100);
    }
}
