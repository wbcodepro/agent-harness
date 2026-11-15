//! A local web interface.
//!
//! `hame web` serves the built interface of `apps/ui` and a JSON API on
//! 127.0.0.1. Session records reach the page as server-sent events, so it
//! shows a turn as it streams; approval questions are answered from the
//! page.
//!
//! Who may call: the page on this computer, which must carry the token
//! printed at startup (and address the server as `127.0.0.1` or
//! `localhost`, against DNS rebinding); and remote devices, whose requests
//! the relay connector passes to [`router`] in process marked with a
//! [`RemoteCaller`]. Remote callers get a stricter policy (`[remote]` in
//! config.toml) on every route that acts on a session: its folder must be
//! in `roots` (by default only folders that already have a session, and the
//! folder `hame web` started in), and unless `allow_full_access = true`
//! they cannot choose "Full access" or "Accept edits", give session-wide
//! grants, or drive a session that runs without the sandbox and without
//! asking; grants given on this computer do not answer for turns they
//! drive, and browser and desktop actions that act are asked about in
//! them. With `roots` set, a remote device does not see (list, read,
//! follow, rename) sessions outside them: they answer 404. A server built
//! with `Options::ask_before_changes` (`hame connect`) reports it: `info`
//! and every session carry `enforced`, and a remote device's new session is
//! recorded as Ask.
//!
//! A session that is only being looked at is read from its log without its
//! lock, so a terminal can resume it; its agent is built, and the lock
//! taken, when a turn starts, and released after `[web] idle_seconds`
//! without one.
//!
//! The API, route by route (all under `/api`):
//!
//! | Route | What |
//! |---|---|
//! | `GET /info` | version, default folder, recent folders, permission modes, models, warnings |
//! | `GET /sessions` | sessions, latest activity first, with title and status |
//! | `POST /sessions` | a new session, optionally in `cwd`, with `mode`, `provider`, `model` |
//! | `GET /sessions/{id}` | one session: title, folder, mode, model, usage, cost, context, grants |
//! | `PATCH /sessions/{id}` | rename (`title`) or pin (`pinned`) |
//! | `DELETE /sessions/{id}` | delete the session's log |
//! | `PUT /sessions/{id}/settings` | change `mode`, `provider`, `model` between turns |
//! | `DELETE /sessions/{id}/grants/{tool}` | revoke a session-wide "always allow" |
//! | `GET /sessions/{id}/events` | the event stream; `after` or `Last-Event-ID` resumes |
//! | `POST /sessions/{id}/prompt` | start a turn with `text`, uploaded `images` and `files` |
//! | `POST /sessions/{id}/steer` | queue a message for the running turn |
//! | `POST /sessions/{id}/steer/withdraw` | take a queued message back |
//! | `POST /sessions/{id}/cancel` | stop the running turn |
//! | `POST /sessions/{id}/approval` | answer an approval question (naming its tool call) |
//! | `POST /sessions/{id}/attachments` | upload an image or a file (`?name=`) |
//! | `GET /artifacts/{sha256}?session=` | an image or file the session refers to |
//!
//! Choosing "Full access" takes two requests: the first is answered `428`
//! with a one-time `confirm` nonce, and the second must echo it.

mod assets;
mod caller;
pub mod jcs;
mod meta;
mod overview;
mod routes;
mod session;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path as FsPath, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{DefaultBodyLimit, Query, Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use hame_agent::Session;
use hame_app::{Options, RuleAction, SandboxMode};
use hame_session::{Event, Record, SessionId};
use serde_json::{Value, json};

pub use caller::{Caller, RemoteCaller, mark_remote};
pub use meta::Mode;
use meta::{MetaStore, SessionMeta};
use overview::OverviewCache;
use session::{Entry, Live, PagePrompter};

/// The largest upload accepted, in bytes.
pub const MAX_UPLOAD_BYTES: usize = 20 * 1024 * 1024;

/// How long a "Full access" confirmation nonce stays valid.
const CONFIRM_FOR: Duration = Duration::from_secs(120);

/// A failed request: a status and a text message, or a JSON body.
#[derive(Debug)]
pub(crate) struct Failure {
    status: StatusCode,
    message: String,
    body: Option<Value>,
}

impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        match self.body {
            Some(body) => (self.status, Json(body)).into_response(),
            None => (self.status, self.message).into_response(),
        }
    }
}

fn failure(status: StatusCode, message: impl Into<String>) -> Failure {
    Failure {
        status,
        message: message.into(),
        body: None,
    }
}

pub(crate) fn bad(message: impl Into<String>) -> Failure {
    failure(StatusCode::BAD_REQUEST, message)
}

pub(crate) fn failed(message: impl Into<String>) -> Failure {
    failure(StatusCode::INTERNAL_SERVER_ERROR, message)
}

pub(crate) fn conflict(message: impl Into<String>) -> Failure {
    failure(StatusCode::CONFLICT, message)
}

pub(crate) fn not_found(message: impl Into<String>) -> Failure {
    failure(StatusCode::NOT_FOUND, message)
}

pub(crate) fn forbidden(message: impl Into<String>) -> Failure {
    failure(StatusCode::FORBIDDEN, message)
}

/// 423: another hame process (a terminal, an editor) has the session open.
/// Distinct from 409 so a page does not mistake it for a turn that just
/// started or ended.
pub(crate) fn locked() -> Failure {
    failure(
        StatusCode::LOCKED,
        "the session is open in another hame process (a terminal or an editor); close it \
         there first",
    )
}

pub(crate) struct App {
    home: PathBuf,
    cwd: PathBuf,
    options: Options,
    token: String,
    meta: MetaStore,
    sessions: Mutex<HashMap<String, Arc<Entry>>>,
    overviews: OverviewCache,
    /// One-time nonces for choosing "Full access": nonce → (scope, issued).
    confirmations: Mutex<HashMap<String, (String, Instant)>>,
}

/// A running server.
pub struct WebServer {
    pub addr: SocketAddr,
    pub token: String,
    router: Router,
    task: tokio::task::JoinHandle<()>,
}

impl WebServer {
    /// The address to open in a browser.
    pub fn url(&self) -> String {
        format!("http://{}/?token={}", self.addr, self.token)
    }

    /// The same routes, to call in process (the desktop app, the relay
    /// connector).
    pub fn router(&self) -> Router {
        self.router.clone()
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
    let (router, token) = router(home, cwd, options);
    let serving = router.clone();
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, serving).await;
    });
    Ok(WebServer {
        addr,
        token,
        router,
        task,
    })
}

/// The server's routes without a listener, with the token every local API
/// call must carry. The relay connector (`hame connect`) calls this router
/// in process and marks each request with a [`RemoteCaller`], so a remote
/// session is exactly a local one under the remote policy. Must be called
/// inside a Tokio runtime (it starts the task that releases idle agents).
pub fn router(home: PathBuf, cwd: PathBuf, options: Options) -> (Router, String) {
    let token = uuid::Uuid::new_v4().simple().to_string();
    let idle = hame_app::load_config(&home)
        .map(|config| config.web.idle_seconds)
        .unwrap_or(600);
    let app = Arc::new(App {
        meta: MetaStore::new(&home),
        home,
        cwd,
        options,
        token: token.clone(),
        sessions: Mutex::new(HashMap::new()),
        overviews: OverviewCache::default(),
        confirmations: Mutex::new(HashMap::new()),
    });
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        let sweeper = Arc::downgrade(&app);
        let idle = Duration::from_secs(idle.max(1));
        let every = idle
            .min(Duration::from_secs(30))
            .max(Duration::from_millis(250));
        runtime.spawn(async move {
            loop {
                tokio::time::sleep(every).await;
                let Some(app) = sweeper.upgrade() else { return };
                app.sweep(idle).await;
            }
        });
    }
    use routes::*;
    let api = Router::new()
        .route("/info", get(info))
        .route("/sessions", get(list).post(create))
        .route(
            "/sessions/{id}",
            get(detail).patch(rename).delete(delete_session),
        )
        .route("/sessions/{id}/settings", put(settings))
        .route("/sessions/{id}/grants/{tool}", delete(revoke_grant))
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
    (router, token)
}

/// Lets through requests the relay connector marked as remote (they never
/// come from HTTP), and local requests with the token addressed to this
/// machine's name (a hostile page could otherwise reach a local server
/// through DNS rebinding).
async fn guard(
    State(app): State<Arc<App>>,
    Query(query): Query<HashMap<String, String>>,
    request: Request,
    next: Next,
) -> Response {
    if request.extensions().get::<RemoteCaller>().is_some() {
        return next.run(request).await;
    }
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

/// Runs `attempt` again for a moment while it fails with 423.
///
/// Why: a session's lock is an `flock` on its lock file, and an `flock`
/// belongs to the open file description, not to the descriptor. When
/// another thread of this process starts a child process, the child holds
/// a copy of every open description between `fork` and `exec` (close-on-
/// exec only closes them at `exec`), so a lock this process has just let
/// go can still be held by that child for an instant: under a millisecond
/// as measured on macOS. A lock another hame process holds is still
/// reported after the retries (about 200 ms).
pub(crate) async fn retry_locked<T>(
    mut attempt: impl FnMut() -> Result<T, Failure>,
) -> Result<T, Failure> {
    let mut tries = 0;
    loop {
        match attempt() {
            Err(failure) if failure.status == StatusCode::LOCKED && tries < 10 => {
                tries += 1;
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            done => return done,
        }
    }
}

/// `~/x` with the home folder.
fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(|home| PathBuf::from(home).join(rest))
            .unwrap_or_else(|| PathBuf::from(path)),
        None => PathBuf::from(path),
    }
}

impl App {
    pub(crate) fn store(&self) -> hame_session::SessionStore {
        hame_app::session_store(&self.home)
    }

    fn config(&self) -> Option<hame_policy::Config> {
        hame_app::load_config(&self.home).ok()
    }

    /// The options a session's agent is built with: the server's, with the
    /// page as the one asked, the session's queue, mode and model.
    fn options_for(&self, meta: &SessionMeta, entry: &Entry) -> Options {
        let mut options = self.options.clone();
        options.prompter = Some(Arc::new(PagePrompter(Arc::clone(&entry.channel))));
        // A turn driven from another device asks before browser and desktop
        // actions that act, as before any other change.
        let channel = Arc::clone(&entry.channel);
        options.ask_control = Some(Arc::new(move || channel.remote_run.load(Ordering::SeqCst)));
        options.steering = Some(entry.steering.clone());
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

    pub(crate) fn loaded(&self, id: &str) -> Option<Arc<Entry>> {
        self.sessions.lock().expect("sessions").get(id).cloned()
    }

    /// The session with this id, for viewing: no agent, no lock.
    pub(crate) fn view(&self, id: &str) -> Result<Arc<Entry>, Failure> {
        if let Some(entry) = self.loaded(id) {
            return Ok(entry);
        }
        let session = SessionId::parse(id).map_err(|e| bad(e.to_string()))?;
        let cwd = hame_app::session_cwd(&self.store(), &session).map_err(|e| {
            if e.contains("not found") {
                not_found(e)
            } else {
                bad(e)
            }
        })?;
        let meta = self.meta.get(id);
        let mut sessions = self.sessions.lock().expect("sessions");
        Ok(Arc::clone(sessions.entry(id.to_string()).or_insert_with(
            || Arc::new(Entry::new(id.to_string(), cwd, meta.mode)),
        )))
    }

    /// Builds the agent and takes the session's lock.
    pub(crate) async fn make_live(
        &self,
        entry: &Entry,
        slot: &mut Option<Live>,
    ) -> Result<(), Failure> {
        if slot.is_some() {
            return Ok(());
        }
        // The session's lock first: no agent is built for a session another
        // process has open.
        let session = self.open(entry).await?;
        let meta = self.meta.get(&entry.id);
        let options = self.options_for(&meta, entry);
        let runtime = hame_app::build(&self.home, &entry.cwd, &options)
            .await
            .map_err(failed)?;
        *slot = Some(Live {
            session,
            runtime,
            idle_since: Instant::now(),
        });
        Ok(())
    }

    /// Opens the session for writing, its records going to the pages.
    async fn open(&self, entry: &Entry) -> Result<Session, Failure> {
        let id = SessionId::parse(&entry.id).map_err(|e| bad(e.to_string()))?;
        let store = self.store();
        let mut session = retry_locked(|| {
            hame_app::open_session(&store, Some(&id), &entry.cwd).map_err(|e| {
                if e.contains("open in another process") {
                    locked()
                } else {
                    failed(e)
                }
            })
        })
        .await?;
        let channel = Arc::clone(&entry.channel);
        let steering = entry.steering.clone();
        session.observe(move |record: &Record| {
            channel.send_record(record);
            // A queued message the turn just took is no longer waiting.
            if matches!(record.event, Event::UserMessage { .. }) {
                channel.send_steering(&steering);
            }
        });
        Ok(session)
    }

    /// Appends `events` to the session's log: through the agent's session
    /// when it is live, otherwise under the lock taken just for this.
    pub(crate) async fn append(
        &self,
        entry: &Entry,
        slot: &mut Option<Live>,
        events: Vec<Event>,
    ) -> Result<(), Failure> {
        if events.is_empty() {
            return Ok(());
        }
        let mut brief;
        let session = match slot {
            Some(live) => &mut live.session,
            None => {
                // A `mode/changed` record needs a log marked with the
                // current format; an older build then calls the log newer
                // rather than corrupt.
                if events
                    .iter()
                    .any(|event| matches!(event, Event::ModeChanged { .. }))
                {
                    let id = SessionId::parse(&entry.id).map_err(|e| bad(e.to_string()))?;
                    let store = self.store();
                    retry_locked(|| {
                        store.upgrade_format(&id).map_err(|e| match e {
                            hame_session::SessionError::Locked(_) => locked(),
                            e => failed(e.to_string()),
                        })
                    })
                    .await?;
                }
                brief = self.open(entry).await?;
                &mut brief
            }
        };
        for event in events {
            session.append(event).map_err(|e| failed(e.to_string()))?;
        }
        Ok(())
    }

    /// The model the session uses or would use, without building anything.
    pub(crate) fn model_name(&self, meta: &SessionMeta, entry: &Entry) -> Option<String> {
        if let Ok(slot) = entry.live.try_lock()
            && let Some(live) = slot.as_ref()
        {
            return live.runtime.model.clone();
        }
        if self.options.llm.is_some() {
            return None;
        }
        let config = self.config()?;
        let secrets = hame_app::load_secrets(&self.home, &entry.cwd).ok()?;
        hame_app::choose_model(&config, &secrets, &self.options_for(meta, entry))
            .ok()
            .map(|chosen| chosen.model)
    }

    /// Remote callers may not choose "Full access" unless allowed.
    pub(crate) fn check_mode(&self, caller: &Caller, mode: Mode) -> Result<(), Failure> {
        // Accept edits lets file changes run unasked: for a remote device
        // it counts as Full access does.
        if caller.is_remote()
            && matches!(mode, Mode::FullAccess | Mode::AcceptEdits)
            && !self.remote_full_access()
        {
            return Err(forbidden(
                "a remote device cannot choose Full access or Accept edits; set [remote] \
                 allow_full_access = true in config.toml on this computer to allow it",
            ));
        }
        Ok(())
    }

    /// Whether a remote caller may see (list, read, follow, rename) a
    /// session in `folder`: with `[remote] roots` set, only sessions inside
    /// them; without roots, every session. Local callers see everything.
    /// Routes answer 404 for a session it may not see, so its existence
    /// does not leak.
    pub(crate) fn remote_may_see(&self, caller: &Caller, folder: &FsPath) -> bool {
        if !caller.is_remote() {
            return true;
        }
        let Some(config) = self.config() else {
            return false;
        };
        if config.remote.roots.is_empty() {
            return true;
        }
        let folder = hame_kernel::real_path(folder).unwrap_or_else(|_| folder.to_path_buf());
        config.remote.roots.iter().any(|root| {
            hame_kernel::real_path(&expand_home(root)).is_ok_and(|root| folder.starts_with(root))
        })
    }

    /// Whether turns in this session would run with no sandbox and no
    /// questions: the Full access mode, or the default mode where the
    /// sandbox is off (`[sandbox] mode = "danger-full-access"` or the
    /// server's own options) or every question is answered yes.
    pub(crate) fn effective_full_access(&self, meta: &SessionMeta) -> bool {
        match meta.mode {
            Mode::FullAccess => true,
            Mode::Default => {
                let configured = self.config().map(|config| config.sandbox.mode);
                self.options.yes
                    || self.options.sandbox.or(configured) == Some(SandboxMode::DangerFullAccess)
            }
            _ => false,
        }
    }

    /// What a remote caller must pass to act on a session (prompt, queue,
    /// answer, upload, change, delete): the session's folder within the
    /// remote roots and, when `with_mode`, no effective Full access unless
    /// remote Full access is allowed. Local callers always pass.
    pub(crate) fn remote_may_act(
        &self,
        caller: &Caller,
        entry: &Entry,
        with_mode: bool,
    ) -> Result<(), Failure> {
        if !caller.is_remote() {
            return Ok(());
        }
        self.check_folder(caller, &entry.cwd)?;
        if with_mode
            && self.effective_full_access(&self.meta.get(&entry.id))
            && !self.remote_full_access()
        {
            return Err(forbidden(
                "this session runs without the sandbox and without asking (Full access); a \
                 remote device cannot drive it unless [remote] allow_full_access = true in \
                 config.toml on this computer",
            ));
        }
        Ok(())
    }

    pub(crate) fn remote_full_access(&self) -> bool {
        self.config()
            .is_some_and(|config| config.remote.allow_full_access)
    }

    /// Remote callers start sessions only in `[remote] roots`, or, without
    /// roots, only in folders that already have a session and the folder
    /// `hame web` started in.
    pub(crate) fn check_folder(&self, caller: &Caller, folder: &FsPath) -> Result<(), Failure> {
        if !caller.is_remote() {
            return Ok(());
        }
        let config = self
            .config()
            .ok_or_else(|| forbidden("config.toml cannot be read; remote sessions are refused"))?;
        let allowed = if config.remote.roots.is_empty() {
            folder == self.cwd
                || self
                    .store()
                    .list()
                    .unwrap_or_default()
                    .iter()
                    .any(|header| FsPath::new(&header.cwd) == folder)
        } else {
            config.remote.roots.iter().any(|root| {
                hame_kernel::real_path(&expand_home(root))
                    .is_ok_and(|root| folder.starts_with(root))
            })
        };
        if allowed {
            Ok(())
        } else {
            Err(forbidden(format!(
                "a remote device cannot start a session in {}; add a folder above it to [remote] \
                 roots in config.toml on this computer",
                folder.display()
            )))
        }
    }

    /// Checks the one-time nonce for choosing "Full access" in `scope`, or
    /// answers 428 with a new one to echo. The nonce stays valid until
    /// [`App::consume`] is called once the change has been made.
    pub(crate) fn confirm(&self, scope: &str, given: Option<&str>) -> Result<String, Failure> {
        let mut nonces = self.confirmations.lock().expect("confirmations");
        nonces.retain(|_, (_, issued)| issued.elapsed() < CONFIRM_FOR);
        if let Some(given) = given
            && nonces
                .get(given)
                .is_some_and(|(issued_for, _)| issued_for == scope)
        {
            return Ok(given.to_string());
        }
        let nonce = uuid::Uuid::new_v4().simple().to_string();
        nonces.insert(nonce.clone(), (scope.to_string(), Instant::now()));
        let message = "Full access runs every command without the sandbox and without asking. \
                       Send the request again with this confirm value to choose it.";
        Err(Failure {
            status: StatusCode::PRECONDITION_REQUIRED,
            message: message.into(),
            body: Some(json!({"confirm": nonce, "message": message})),
        })
    }

    /// Uses up a confirmation nonce.
    pub(crate) fn consume(&self, nonce: &str) {
        self.confirmations
            .lock()
            .expect("confirmations")
            .remove(nonce);
    }

    pub(crate) fn status(&self, id: &str, outcome: overview::Outcome) -> &'static str {
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
    pub(crate) fn describe(
        &self,
        id: &str,
        cwd: &str,
        seen: &overview::Overview,
        meta: &SessionMeta,
    ) -> Value {
        json!({
            "id": id,
            "cwd": cwd,
            "title": meta.title.clone().or(seen.title.clone()),
            "named": meta.title.is_some(),
            "pinned": meta.pinned,
            "status": self.status(id, seen.outcome),
            "outcome": seen.outcome,
            "created": seen.created,
            "last_activity": seen.last_activity,
            "turns": seen.turns,
            "usage": seen.usage,
            "mode": meta.mode,
            "enforced": self.enforced(meta),
        })
    }

    /// What this server enforces on top of the session's mode: `"ask"`
    /// when it was started to ask before every change (`hame connect`:
    /// `Options::ask_before_changes`), `"ask-without-sandbox"` when such a
    /// session also runs without the sandbox (Full access there still
    /// asks); `null` when the mode is what applies.
    pub(crate) fn enforced(&self, meta: &SessionMeta) -> Option<&'static str> {
        if !self.options.ask_before_changes {
            return None;
        }
        let configured = self.config().map(|config| config.sandbox.mode);
        let unsandboxed = meta.mode == Mode::FullAccess
            || (meta.mode == Mode::Default
                && self.options.sandbox.or(configured) == Some(SandboxMode::DangerFullAccess));
        Some(if unsandboxed {
            "ask-without-sandbox"
        } else {
            "ask"
        })
    }

    pub(crate) fn overview_of(&self, id: &SessionId) -> overview::Overview {
        let store = self.store();
        self.overviews.get(id.as_str(), &store.log_path(id), || {
            store.read(id).unwrap_or_default()
        })
    }

    /// Releases agents idle for `idle` (and with them the sessions' locks),
    /// and forgets sessions nobody is watching.
    async fn sweep(&self, idle: Duration) {
        let entries: Vec<Arc<Entry>> = self
            .sessions
            .lock()
            .expect("sessions")
            .values()
            .cloned()
            .collect();
        for entry in entries {
            if entry.busy.load(Ordering::SeqCst) {
                continue;
            }
            let Ok(mut slot) = entry.live.try_lock() else {
                continue;
            };
            if slot
                .as_ref()
                .is_some_and(|live| live.idle_since.elapsed() >= idle)
                && let Some(live) = slot.take()
            {
                live.runtime.computer.shutdown().await;
                drop(live);
            }
            drop(slot);
        }
        // Forget sessions nobody watches or uses. Under the sessions lock
        // and only when the map holds the last reference, so a request that
        // has just picked a session up keeps using the one that stays.
        let mut sessions = self.sessions.lock().expect("sessions");
        sessions.retain(|_, entry| {
            let unused = Arc::strong_count(entry) == 1
                && !entry.busy.load(Ordering::SeqCst)
                && entry.live.try_lock().is_ok_and(|slot| slot.is_none())
                && entry.channel.events.receiver_count() == 0
                && entry
                    .channel
                    .approvals
                    .lock()
                    .expect("approvals")
                    .is_empty()
                && entry.steering.pending().is_empty()
                && entry.uploads.lock().expect("uploads").images.is_empty()
                && entry.uploads.lock().expect("uploads").files.is_empty();
            !unused
        });
    }
}
