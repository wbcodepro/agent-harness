//! The API's route handlers.

use std::collections::{HashMap, VecDeque};
use std::convert::Infallible;
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use hame_app::Options;
use hame_session::{Block, Event, SessionId};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use crate::caller::Caller;
use crate::meta::{Mode, SessionMeta};
use crate::session::{Entry, Outgoing, TurnCancel, run_turns};
use crate::{App, Failure, MAX_UPLOAD_BYTES, bad, conflict, failed, forbidden, not_found};

/// Where uploaded files that are not images go, inside the session's
/// working folder.
const ATTACHMENTS_DIR: &str = ".hame/attachments";

fn parse_body<T: for<'de> Deserialize<'de> + Default>(body: &Bytes) -> Result<T, Failure> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(T::default());
    }
    serde_json::from_slice(body).map_err(|e| bad(format!("invalid request body: {e}")))
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

fn session_id(id: &str) -> Result<SessionId, Failure> {
    SessionId::parse(id).map_err(|e| bad(e.to_string()))
}

/// The `mode/changed` record for a change from `previous` to `mode`.
fn mode_changed(mode: Mode, previous: Mode, caller: &Caller) -> Event {
    let (by, label) = caller.by();
    Event::ModeChanged {
        mode: mode.name().into(),
        previous: previous.name().into(),
        by,
        label,
    }
}

pub(crate) async fn info(State(app): State<Arc<App>>) -> Result<Json<Value>, Failure> {
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
    let warnings: Vec<String> = app.meta.warning().into_iter().collect();
    Ok(Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "cwd": app.cwd,
        "folders": folders,
        "modes": modes,
        "models": models(&app),
        "limits": {"upload_bytes": MAX_UPLOAD_BYTES},
        "warnings": warnings,
    })))
}

/// The providers hame can use here, whether a key for each is set, and the
/// model each would use. Never the keys themselves.
fn models(app: &App) -> Value {
    if app.options.llm.is_some() {
        return json!({"fixed": true, "current": null, "providers": []});
    }
    let (Some(config), Ok(secrets)) = (app.config(), hame_app::load_secrets(&app.home, &app.cwd))
    else {
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

pub(crate) async fn list(State(app): State<Arc<App>>) -> Result<Json<Value>, Failure> {
    let headers = app.store().list().map_err(|e| failed(e.to_string()))?;
    let meta = app.meta.all();
    let mut rows: Vec<Value> = headers
        .into_iter()
        .map(|header| {
            let id = header.id.to_string();
            let seen = app.overview_of(&header.id);
            let meta = meta.get(&id).cloned().unwrap_or_default();
            app.describe(&id, &header.cwd, &seen, &meta)
        })
        .collect();
    rows.sort_by_key(|row| std::cmp::Reverse(row["last_activity"].as_u64().unwrap_or(0)));
    Ok(Json(Value::Array(rows)))
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateBody {
    cwd: Option<String>,
    mode: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    /// The nonce a first request for "Full access" was answered with.
    confirm: Option<String>,
}

pub(crate) async fn create(
    State(app): State<Arc<App>>,
    caller: Caller,
    body: Bytes,
) -> Result<Json<Value>, Failure> {
    let body: CreateBody = parse_body(&body)?;
    let cwd = match &body.cwd {
        Some(raw) => folder(raw)?,
        None => app.cwd.clone(),
    };
    app.check_folder(&caller, &cwd)?;
    check_model(&body.provider, &body.model)?;
    let mode = body
        .mode
        .as_deref()
        .map(parse_mode)
        .transpose()?
        .unwrap_or_default();
    app.check_mode(&caller, mode)?;
    if mode == Mode::FullAccess {
        app.confirm("new", body.confirm.as_deref())?;
    }
    let mut session = hame_app::open_session(&app.store(), None, &cwd).map_err(failed)?;
    if mode != Mode::Default {
        session
            .append(mode_changed(mode, Mode::Default, &caller))
            .map_err(|e| failed(e.to_string()))?;
    }
    let id = session.id().to_string();
    // Only viewed until a turn starts: the lock is released at once.
    drop(session);
    let meta = SessionMeta {
        mode,
        provider: body.provider,
        model: body.model,
        ..SessionMeta::default()
    };
    app.meta.update(&id, |_| Some(meta)).map_err(failed)?;
    Ok(Json(json!({ "id": id })))
}

pub(crate) async fn detail(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Failure> {
    let entry = app.view(&id)?;
    let session = session_id(&id)?;
    let records = app
        .store()
        .read(&session)
        .map_err(|e| failed(e.to_string()))?;
    let meta = app.meta.get(&id);
    let seen = app.overview_of(&session);
    let mut described = app.describe(&id, &entry.cwd.to_string_lossy(), &seen, &meta);
    let model = app.model_name(&meta, &entry);
    let compact_at = app.config().map(|config| config.context.compact_at_tokens);
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
        "approvals": entry.channel.pending(),
        "grants": entry.channel.grants(),
    });
    if let (Some(described), Value::Object(extra)) = (described.as_object_mut(), extra) {
        described.extend(extra);
    }
    Ok(Json(described))
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct RenameBody {
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

pub(crate) async fn rename(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<Value>, Failure> {
    let body: RenameBody = parse_body(&body)?;
    let session = session_id(&id)?;
    let cwd =
        hame_app::session_cwd(&app.store(), &session).map_err(|_| not_found("no such session"))?;
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
    let seen = app.overview_of(&session);
    let described = app.describe(&id, &cwd.to_string_lossy(), &seen, &meta);
    if let Some(entry) = app.loaded(&id) {
        entry
            .channel
            .send(json!({"kind": "session", "session": described}));
    }
    Ok(Json(described))
}

pub(crate) async fn delete_session(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Failure> {
    let session = session_id(&id)?;
    if let Some(entry) = app.loaded(&id) {
        {
            // No turn may start while the session goes away.
            let _gate = entry.gate.lock().expect("gate");
            if entry.busy.load(Ordering::SeqCst) {
                return Err(conflict("stop the running turn first"));
            }
            entry.deleted.store(true, Ordering::SeqCst);
        }
        let mut slot = entry.live.lock().await;
        if let Some(live) = slot.take() {
            live.runtime.computer.shutdown().await;
            // Releases the session's lock before anything is removed.
            drop(live);
        }
        drop(slot);
        app.sessions.lock().expect("sessions").remove(&id);
    }
    let store = app.store();
    // Taking the lock proves no other hame (a terminal, an editor) has the
    // session open.
    match store.open(&session) {
        Ok((_, writer)) => drop(writer),
        Err(hame_session::SessionError::NotFound(_)) => return Err(not_found("no such session")),
        Err(hame_session::SessionError::Locked(_)) => {
            return Err(conflict(
                "the session is open in another hame process (a terminal or an editor); close \
                 it there first",
            ));
        }
        Err(e) => return Err(failed(e.to_string())),
    }
    std::fs::remove_dir_all(store.root().join(session.as_str()))
        .map_err(|e| failed(format!("cannot delete the session: {e}")))?;
    app.meta.update(&id, |_| None).map_err(failed)?;
    app.overviews.forget(&id);
    Ok(Json(json!({})))
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct SettingsBody {
    mode: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    /// The nonce a first request for "Full access" was answered with.
    confirm: Option<String>,
}

/// Changes the permission mode or the model. The agent is rebuilt with the
/// new settings at the next turn, so this waits for a turn to end. A new
/// mode is recorded in the log and ends the session's "always allow"
/// grants.
pub(crate) async fn settings(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    caller: Caller,
    body: Bytes,
) -> Result<Json<Value>, Failure> {
    let body: SettingsBody = parse_body(&body)?;
    let mode = body.mode.as_deref().map(parse_mode).transpose()?;
    check_model(&body.provider, &body.model)?;
    let entry = app.view(&id)?;
    let mut meta = app.meta.get(&id);
    let previous = meta.mode;
    if let Some(mode) = mode {
        app.check_mode(&caller, mode)?;
        if mode == Mode::FullAccess && previous != Mode::FullAccess {
            app.confirm(&id, body.confirm.as_deref())?;
        }
    }
    if entry.busy.load(Ordering::SeqCst) {
        return Err(conflict(
            "settings change between turns; wait or stop the turn",
        ));
    }
    let mut slot = entry.live.lock().await;
    if entry.busy.load(Ordering::SeqCst) {
        return Err(conflict(
            "settings change between turns; wait or stop the turn",
        ));
    }
    let before = app.model_name(&meta, &entry);
    if let Some(mode) = mode {
        meta.mode = mode;
    }
    if body.provider.is_some() {
        meta.provider = body.provider;
    }
    if body.model.is_some() {
        meta.model = body.model;
    }
    let mut events = Vec::new();
    if meta.mode != previous {
        events.push(mode_changed(meta.mode, previous, &caller));
    }
    // The agent is rebuilt with the new settings at the next turn.
    if let Some(live) = slot.take() {
        live.runtime.computer.shutdown().await;
    }
    let after = app.model_name(&meta, &entry);
    if let Some(model) = &after
        && before.as_ref() != Some(model)
    {
        // Prices and later escalation read the model from the log.
        events.push(Event::ModelSwitched {
            model: model.clone(),
        });
    }
    app.append(&entry, &mut slot, events)?;
    drop(slot);
    if meta.mode != previous {
        *entry.channel.mode.lock().expect("mode") = meta.mode;
        entry.channel.always.lock().expect("always").clear();
        entry.channel.send_grants();
    }
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

pub(crate) async fn revoke_grant(
    State(app): State<Arc<App>>,
    Path((id, tool)): Path<(String, String)>,
) -> Result<Json<Value>, Failure> {
    let entry = app.view(&id)?;
    if !entry.channel.always.lock().expect("always").remove(&tool) {
        return Err(not_found("no such grant"));
    }
    entry.channel.send_grants();
    Ok(Json(json!({"grants": entry.channel.grants()})))
}

#[derive(Deserialize)]
pub(crate) struct EventsQuery {
    after: Option<u64>,
}

pub(crate) async fn events(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    Query(query): Query<EventsQuery>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, Failure> {
    let entry = app.view(&id)?;
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
    let session = session_id(&id)?;
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
    // The live state, whole, on every subscription: a page that reconnects
    // replaces what it had.
    let turn = if entry.busy.load(Ordering::SeqCst) {
        json!({"kind": "turn", "state": "started"})
    } else {
        json!({"kind": "turn", "state": "ended",
               "error": entry.last_error.lock().expect("last error").clone()})
    };
    replay.push_back(Outgoing::event(&turn));
    replay.push_back(Outgoing::event(
        &json!({"kind": "approvals", "pending": entry.channel.pending()}),
    ));
    replay.push_back(Outgoing::event(
        &json!({"kind": "steering", "queued": entry.steering.pending()}),
    ));
    replay.push_back(Outgoing::event(
        &json!({"kind": "grants", "grants": entry.channel.grants()}),
    ));
    drop(entry);
    let stream = futures_util::stream::unfold(
        (replay, live, last_seq),
        |(mut replay, mut live, mut last_seq)| async move {
            if let Some(next) = replay.pop_front() {
                return Some((Ok::<_, Infallible>(sse(&next)), (replay, live, last_seq)));
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
                        return Some((Ok(sse(&next)), (replay, live, last_seq)));
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

fn sse(outgoing: &Outgoing) -> SseEvent {
    let event = SseEvent::default().data(&*outgoing.data);
    match outgoing.seq {
        Some(seq) => event.id(seq.to_string()),
        None => event,
    }
}

#[derive(Deserialize)]
pub(crate) struct PromptBody {
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

pub(crate) async fn prompt(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    Json(body): Json<PromptBody>,
) -> Result<Json<Value>, Failure> {
    if body.text.trim().is_empty() && body.images.is_empty() && body.files.is_empty() {
        return Err(bad("the message is empty"));
    }
    let entry = app.view(&id)?;
    let content = message(&entry, body)?;
    // The stop handle exists before the run does, so a stop sent at once
    // is honoured.
    let cancel = Arc::new(TurnCancel::default());
    {
        let _gate = entry.gate.lock().expect("gate");
        if entry.deleted.load(Ordering::SeqCst) {
            return Err(not_found("the session was deleted"));
        }
        if entry.busy.swap(true, Ordering::SeqCst) {
            return Err(conflict("a turn is already running"));
        }
        *entry.cancel.lock().expect("cancel") = Some(Arc::clone(&cancel));
        *entry.last_error.lock().expect("last error") = None;
    }
    let mut slot = Arc::clone(&entry.live).lock_owned().await;
    if let Err(failure) = app.make_live(&entry, &mut slot).await {
        let _gate = entry.gate.lock().expect("gate");
        *entry.cancel.lock().expect("cancel") = None;
        entry.busy.store(false, Ordering::SeqCst);
        return Err(failure);
    }
    tokio::spawn(run_turns(entry, slot, content, cancel));
    Ok(Json(json!({"started": true})))
}

#[derive(Deserialize)]
pub(crate) struct SteerBody {
    text: String,
}

/// Queues a message for the running turn. The agent hands it to the model
/// at its next step; one queued as the turn ends starts the next turn.
/// With no turn running the answer is 409: send a prompt instead.
pub(crate) async fn steer(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    Json(body): Json<SteerBody>,
) -> Result<Json<Value>, Failure> {
    let text = body.text.trim();
    if text.is_empty() {
        return Err(bad("the message is empty"));
    }
    let entry = app.view(&id)?;
    let queued = {
        let _gate = entry.gate.lock().expect("gate");
        if !entry.busy.load(Ordering::SeqCst) {
            None
        } else {
            entry.steering.push(text);
            Some(entry.steering.pending())
        }
    };
    entry.channel.send_steering(&entry.steering);
    match queued {
        Some(queued) => Ok(Json(json!({"queued": queued}))),
        None => Err(conflict("no turn is running; send the message as a prompt")),
    }
}

#[derive(Deserialize)]
pub(crate) struct WithdrawBody {
    index: usize,
    text: String,
}

pub(crate) async fn withdraw(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    Json(body): Json<WithdrawBody>,
) -> Result<Json<Value>, Failure> {
    let entry = app.view(&id)?;
    let withdrawn = entry.steering.withdraw(body.index, &body.text);
    entry.channel.send_steering(&entry.steering);
    if !withdrawn {
        return Err(conflict("that message was already handed to the agent"));
    }
    Ok(Json(json!({"queued": entry.steering.pending()})))
}

pub(crate) async fn cancel(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Failure> {
    let entry = app.view(&id)?;
    let running = entry.cancel.lock().expect("cancel").clone();
    let stopped = running.is_some();
    if let Some(cancel) = running {
        cancel.cancel();
    }
    Ok(Json(json!({"stopping": stopped})))
}

/// A file name safe to create: letters, digits, `.`, `_` and `-`, not
/// starting with a dot, at most 100 characters.
pub(crate) fn safe_name(raw: &str) -> String {
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
    let chars: Vec<char> = trimmed.chars().collect();
    let name: String = chars[chars.len().saturating_sub(100)..].iter().collect();
    if name.is_empty() {
        "attachment".into()
    } else {
        name
    }
}

/// Makes `dir` (one level) unless it exists, refusing a symbolic link.
fn plain_dir(dir: &FsPath) -> Result<(), Failure> {
    match std::fs::symlink_metadata(dir) {
        Ok(meta) if meta.file_type().is_symlink() => Err(bad(format!(
            "{} is a symbolic link; uploads are not written through it",
            dir.display()
        ))),
        Ok(meta) if meta.is_dir() => Ok(()),
        Ok(_) => Err(bad(format!("{} is not a folder", dir.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => std::fs::create_dir(dir)
            .map_err(|e| failed(format!("cannot create {}: {e}", dir.display()))),
        Err(e) => Err(failed(format!("cannot use {}: {e}", dir.display()))),
    }
}

/// The attachments folder of a session, made safe: neither `.hame` nor
/// `.hame/attachments` may be a link, the folder must resolve inside the
/// session's folder, and it carries a `.gitignore` so uploads stay out of
/// the project's history.
fn attachments_dir(cwd: &FsPath) -> Result<PathBuf, Failure> {
    let hame = cwd.join(".hame");
    plain_dir(&hame)?;
    let dir = hame.join("attachments");
    plain_dir(&dir)?;
    let root = hame_kernel::real_path(cwd).map_err(|e| failed(e.to_string()))?;
    let real = hame_kernel::real_path(&dir).map_err(|e| failed(e.to_string()))?;
    if !real.starts_with(&root) {
        return Err(bad(
            "the attachments folder leads outside the session's folder",
        ));
    }
    let ignore = real.join(".gitignore");
    if !ignore.exists() {
        let _ = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&ignore)
            .and_then(|mut file| std::io::Write::write_all(&mut file, b"*\n"));
    }
    Ok(real)
}

#[derive(Deserialize)]
pub(crate) struct AttachQuery {
    name: Option<String>,
}

/// Takes an uploaded image or file for the next message. Images (checked
/// by their bytes before anything is stored) go to the artifact store, so
/// the model sees them; other files are written to `.hame/attachments` in
/// the session's folder, where its tools can read them.
pub(crate) async fn attach(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    Query(query): Query<AttachQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, Failure> {
    if body.is_empty() {
        return Err(bad("the upload is empty"));
    }
    let entry = app.view(&id)?;
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
        let sniffed = hame_session::artifact::sniff_mime(&body)
            .filter(|sniffed| sniffed.starts_with("image/"))
            .ok_or_else(|| bad("that is not a PNG, JPEG, GIF or WebP image"))?;
        let mut artifact = app
            .store()
            .artifacts()
            .put(&body, sniffed)
            .map_err(|e| bad(e.to_string()))?;
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
    let dir = attachments_dir(&entry.cwd)?;
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem.to_string(), format!(".{extension}")),
        _ => (name.clone(), String::new()),
    };
    let mut chosen = name.clone();
    let mut file = None;
    for n in 1..1000 {
        // `create_new` never follows a link at the final name.
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
pub(crate) async fn artifact(
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
    let session = session_id(session)?;
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
pub(crate) struct ApprovalBody {
    id: u64,
    allow: bool,
    /// The tool call the question was about; must match it.
    tool_call_id: String,
    /// The sha256 of the arguments' canonical JSON, as the question gave
    /// it; required (only a local answer to a question whose arguments
    /// have no canonical form may leave it out).
    #[serde(default)]
    args_sha256: Option<String>,
    /// Also allow this tool for the rest of the session.
    #[serde(default)]
    always: bool,
}

pub(crate) async fn approval(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    caller: Caller,
    Json(body): Json<ApprovalBody>,
) -> Result<Json<Value>, Failure> {
    if body.allow && body.always && caller.is_remote() && !app.remote_full_access() {
        return Err(forbidden(
            "a remote device cannot allow a tool for the whole session; set [remote] \
             allow_full_access = true in config.toml on this computer to allow it",
        ));
    }
    let entry = app.view(&id)?;
    let waiting = {
        let mut approvals = entry.channel.approvals.lock().expect("approvals");
        let Some(question) = approvals.get(&body.id) else {
            return Err(bad("that question is no longer waiting"));
        };
        if question.tool_call_id != body.tool_call_id {
            return Err(bad("the answer names another tool call than the question"));
        }
        match (&question.args_sha256, &body.args_sha256) {
            (Some(expected), Some(given)) if expected == given => {}
            (Some(_), Some(_)) => {
                return Err(bad("the answer names other arguments than the question"));
            }
            (Some(_), None) => return Err(bad("the answer must name args_sha256")),
            (None, _) if caller.is_remote() => {
                return Err(forbidden(
                    "this call's arguments have no canonical form, so a remote answer cannot                      be bound to them; answer on this computer",
                ));
            }
            (None, _) => {}
        }
        approvals.remove(&body.id).expect("checked above")
    };
    if body.allow && body.always {
        entry
            .channel
            .always
            .lock()
            .expect("always")
            .insert(waiting.tool.clone());
        entry.channel.send_grants();
    }
    let _ = waiting.answer.send(Some(body.allow));
    entry.channel.send(json!({
        "kind": "approval_resolved",
        "id": body.id,
        "allow": body.allow,
    }));
    Ok(Json(json!({})))
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
