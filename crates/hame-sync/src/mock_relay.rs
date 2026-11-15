//! An in-process relay for tests and local development. It follows the real
//! relay's contract (crates/hame-relay, docs/relay/API.md) for everything
//! the host and devices use:
//!
//! - WebSocket `/v1/ws?device=<id>` authenticated with
//!   `Authorization: Bearer` or the subprotocols `hame-relay.v1,
//!   bearer.<token>`; the token's session must be bound to that device;
//!   per-recipient mailboxes with a monotonic `seq` from 1; `resume` as the
//!   first frame (anything else resumes from the last acknowledged seq);
//!   `ack`; `sent`; errors answering a send carry its `to`; `presence` for
//!   the account's online devices after `resume` and on changes; `ping` /
//!   `pong`; envelopes up to 1 MiB decoded; routing only within an account
//!   (`not_found` otherwise), never to oneself (`forbidden`); a newer
//!   connection replaces the older one;
//! - device-code sign-in (`start`, `lookup`, `approve`, `token` with
//!   `authorization_pending` / `slow_down` / `access_denied` /
//!   `expired_token` / `invalid_grant`), `refresh` with rotation and reuse
//!   detection, `POST /v1/devices {name, kind, ed25519, x25519}` binding the
//!   session, `GET /v1/devices`, `GET /v1/hosts`, and host grants
//!   (`POST {version, document, signature}` verifies the host's Ed25519
//!   signature over `hame-grants:v1\n{host}\n{version}\n` ‖ document and
//!   refuses a version not above the stored one with `409 stale_version`;
//!   `GET` returns them);
//! - device-code sessions have the `host` scope: they may register only a
//!   `host` device and cannot approve device codes (`403
//!   insufficient_scope`);
//! - `{"op":"auth","token"}` renews a connection's token (answered with
//!   `{"op":"auth","expires_at"}`; a bad token gets `unauthorized` and the
//!   connection closes); at most 20 devices per account.
//!
//! Not modelled: accounts, passwords, OAuth, passkeys, push, rate limits,
//! the 24-hour expiry. Test controls let a test play a malicious relay:
//! inject or replay envelopes, drop connections, read everything routed.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::frames::RelayFrame;
use crate::keys::{PublicIdentity, b64, hex, random32, unb64, unb64_32, verify_signature};
use crate::pairing::SignedGrantList;

/// The WebSocket subprotocol the relay selects.
pub const PROTOCOL: &str = "hame-relay.v1";
/// Largest envelope, decoded.
const MAX_ENVELOPE: usize = 1024 * 1024;

#[derive(Clone, Debug)]
pub struct MockDevice {
    pub account: String,
    pub name: String,
    pub kind: String,
    /// Base64url public keys as registered (empty when registered by a test
    /// control without keys).
    pub ed25519: String,
    pub x25519: String,
}

/// One envelope as the relay routed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Routed {
    pub from: String,
    pub to: String,
    pub envelope: String,
}

enum Command {
    Frame(RelayFrame),
    Close,
}

#[derive(Clone)]
struct Session {
    account: String,
    device: Option<String>,
    /// `full` or `host` (device-code sign-ins).
    scope: &'static str,
}

#[derive(Default)]
struct Mailboxes {
    next_seq: HashMap<String, u64>,
    acked: HashMap<String, u64>,
    queues: HashMap<String, VecDeque<(u64, String, String)>>,
    connections: HashMap<String, (u64, mpsc::UnboundedSender<Command>)>,
}

struct DeviceCode {
    user_code: String,
    client_name: String,
    /// None: pending; Some(Some(account)): approved; Some(None): denied.
    decision: Option<Option<String>>,
}

#[derive(Default)]
struct Inner {
    /// session id → session
    sessions: Mutex<HashMap<String, Session>>,
    /// access token → session id
    access: Mutex<HashMap<String, String>>,
    /// refresh token → (session id, still valid)
    refresh: Mutex<HashMap<String, (String, bool)>>,
    devices: Mutex<HashMap<String, MockDevice>>,
    codes: Mutex<HashMap<String, DeviceCode>>,
    auto_approve: Mutex<Option<String>>,
    mail: Mutex<Mailboxes>,
    grants: Mutex<HashMap<String, SignedGrantList>>,
    log: Mutex<Vec<Routed>>,
    renewals: Mutex<HashMap<String, usize>>,
    counter: AtomicU64,
}

impl Inner {
    fn fresh(&self, prefix: &str) -> String {
        format!(
            "{prefix}{}-{}",
            self.counter.fetch_add(1, Ordering::SeqCst),
            hex(&random32()[..8])
        )
    }

    fn session_of(&self, token: &str) -> Option<(String, Session)> {
        let id = self.access.lock().expect("access").get(token).cloned()?;
        let session = self.sessions.lock().expect("sessions").get(&id).cloned()?;
        Some((id, session))
    }

    fn new_session(&self, account: &str, device: Option<String>, scope: &'static str) -> String {
        let id = self.fresh("sess");
        self.sessions.lock().expect("sessions").insert(
            id.clone(),
            Session {
                account: account.into(),
                device,
                scope,
            },
        );
        id
    }

    /// A fresh token pair for a session, as every sign-in answers.
    fn tokens(&self, session: &str) -> Value {
        let access = format!("hra_{}", hex(&random32()));
        let refresh = format!("hrr_{}", hex(&random32()));
        self.access
            .lock()
            .expect("access")
            .insert(access.clone(), session.into());
        self.refresh
            .lock()
            .expect("refresh")
            .insert(refresh.clone(), (session.into(), true));
        let s = self
            .sessions
            .lock()
            .expect("sessions")
            .get(session)
            .cloned();
        json!({
            "access_token": access,
            "refresh_token": refresh,
            "token_type": "Bearer",
            "expires_in": 900,
            "account_id": s.as_ref().map(|s| s.account.clone()),
            "session_id": session,
            "device_id": s.and_then(|s| s.device),
        })
    }

    fn revoke_session(&self, session: &str) {
        self.sessions.lock().expect("sessions").remove(session);
        self.access
            .lock()
            .expect("access")
            .retain(|_, s| s != session);
        self.refresh
            .lock()
            .expect("refresh")
            .retain(|_, (s, _)| s != session);
    }

    fn account_devices(&self, account: &str) -> Vec<String> {
        self.devices
            .lock()
            .expect("devices")
            .iter()
            .filter(|(_, d)| d.account == account)
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Queues an envelope for `to` and delivers it at once if `to` is
    /// connected. Returns its seq.
    fn enqueue(&self, from: &str, to: &str, envelope: &str) -> u64 {
        self.log.lock().expect("log").push(Routed {
            from: from.into(),
            to: to.into(),
            envelope: envelope.into(),
        });
        let mut mail = self.mail.lock().expect("mail");
        let seq = {
            let next = mail.next_seq.entry(to.into()).or_insert(0);
            *next += 1;
            *next
        };
        mail.queues
            .entry(to.into())
            .or_default()
            .push_back((seq, from.into(), envelope.into()));
        if let Some((_, connection)) = mail.connections.get(to) {
            let _ = connection.send(Command::Frame(RelayFrame::Deliver {
                seq,
                from: from.into(),
                envelope: envelope.into(),
            }));
        }
        seq
    }

    fn acknowledge(&self, device: &str, seq: u64) {
        let mut mail = self.mail.lock().expect("mail");
        let acked = mail.acked.entry(device.into()).or_insert(0);
        *acked = (*acked).max(seq);
        if let Some(queue) = mail.queues.get_mut(device) {
            queue.retain(|(queued, _, _)| *queued > seq);
        }
    }

    fn presence(&self, device: &str, online: bool) {
        let Some(account) = self
            .devices
            .lock()
            .expect("devices")
            .get(device)
            .map(|d| d.account.clone())
        else {
            return;
        };
        let peers = self.account_devices(&account);
        let mail = self.mail.lock().expect("mail");
        for peer in peers.iter().filter(|p| p.as_str() != device) {
            if let Some((_, connection)) = mail.connections.get(peer) {
                let _ = connection.send(Command::Frame(RelayFrame::Presence {
                    device: device.into(),
                    online,
                }));
            }
        }
    }
}

/// A running mock relay.
pub struct MockRelay {
    pub url: String,
    inner: Arc<Inner>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for MockRelay {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl MockRelay {
    pub async fn start() -> Self {
        let inner = Arc::new(Inner::default());
        let router = Router::new()
            .route("/v1/auth/device/start", post(device_start))
            .route("/v1/auth/device/lookup", post(device_lookup))
            .route("/v1/auth/device/approve", post(device_approve))
            .route("/v1/auth/device/token", post(device_token))
            .route("/v1/auth/refresh", post(refresh))
            .route("/v1/devices", post(register).get(list_devices))
            .route("/v1/hosts", get(hosts))
            .route("/v1/hosts/{id}/grants", post(put_grants).get(get_grants))
            .route("/v1/ws", get(ws))
            .route("/healthz", get(|| async { Json(json!({"status": "ok"})) }))
            .with_state(Arc::clone(&inner));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let url = format!("http://{}", listener.local_addr().expect("addr"));
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Self { url, inner, task }
    }

    /// A new account; returns its id.
    pub fn account(&self) -> String {
        self.inner.fresh("acct")
    }

    /// Registers a device of `account` directly (as if it had signed in
    /// and called `POST /v1/devices`); returns its id.
    pub fn register(
        &self,
        account: &str,
        name: &str,
        kind: &str,
        keys: Option<&PublicIdentity>,
    ) -> String {
        let id = self.inner.fresh("dev");
        self.inner.devices.lock().expect("devices").insert(
            id.clone(),
            MockDevice {
                account: account.into(),
                name: name.into(),
                kind: kind.into(),
                ed25519: keys.map(|k| b64(&k.ed)).unwrap_or_default(),
                x25519: keys.map(|k| b64(&k.x)).unwrap_or_default(),
            },
        );
        id
    }

    /// An access token of a new session bound to `device`.
    pub fn token_for(&self, device: &str) -> String {
        let account = self
            .device(device)
            .expect("a registered device")
            .account
            .clone();
        let session = self
            .inner
            .new_session(&account, Some(device.into()), "full");
        self.inner.tokens(&session)["access_token"]
            .as_str()
            .expect("token")
            .to_string()
    }

    pub fn device(&self, id: &str) -> Option<MockDevice> {
        self.inner.devices.lock().expect("devices").get(id).cloned()
    }

    /// Approves device-code sign-ins into `account` as soon as they are
    /// polled (the user approving in a signed-in app).
    pub fn auto_approve(&self, account: &str) {
        *self.inner.auto_approve.lock().expect("approve") = Some(account.into());
    }

    /// Drops a device's connection, as a network failure would.
    pub fn kick(&self, device: &str) {
        if let Some((_, connection)) = self
            .inner
            .mail
            .lock()
            .expect("mail")
            .connections
            .remove(device)
        {
            let _ = connection.send(Command::Close);
        }
    }

    pub fn connected(&self, device: &str) -> bool {
        self.inner
            .mail
            .lock()
            .expect("mail")
            .connections
            .contains_key(device)
    }

    /// Queues an envelope for `to` claiming to be from `from`, as a
    /// malicious relay could.
    pub fn inject(&self, from: &str, to: &str, envelope: &str) -> u64 {
        self.inner.enqueue(from, to, envelope)
    }

    /// Everything routed so far, in order.
    pub fn routed(&self) -> Vec<Routed> {
        self.inner.log.lock().expect("log").clone()
    }

    /// Envelopes queued for `device` and not acknowledged.
    pub fn queued(&self, device: &str) -> usize {
        self.inner
            .mail
            .lock()
            .expect("mail")
            .queues
            .get(device)
            .map_or(0, VecDeque::len)
    }

    /// How many times `device` renewed its token on a connection.
    pub fn renewals(&self, device: &str) -> usize {
        self.inner
            .renewals
            .lock()
            .expect("renewals")
            .get(device)
            .copied()
            .unwrap_or(0)
    }

    /// The grant list a host last published.
    pub fn grants(&self, host: &str) -> Option<SignedGrantList> {
        self.inner.grants.lock().expect("grants").get(host).cloned()
    }
}

fn bearer(headers: &HeaderMap) -> Option<String> {
    if let Some(token) = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    {
        return Some(token.to_string());
    }
    // Browsers cannot set headers on a WebSocket: the token comes as a
    // `bearer.<token>` subprotocol next to `hame-relay.v1`.
    headers
        .get_all("sec-websocket-protocol")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .find_map(|p| p.trim().strip_prefix("bearer.").map(str::to_string))
}

fn error(status: StatusCode, code: &str) -> Response {
    (status, Json(json!({"error": code, "message": code}))).into_response()
}

fn authed(inner: &Inner, headers: &HeaderMap) -> Result<(String, Session), Box<Response>> {
    bearer(headers)
        .and_then(|t| inner.session_of(&t))
        .ok_or_else(|| Box::new(error(StatusCode::UNAUTHORIZED, "invalid_token")))
}

fn device_json(inner: &Inner, id: &str, d: &MockDevice) -> Value {
    let fingerprint = match (unb64_32(&d.ed25519), unb64_32(&d.x25519)) {
        (Ok(ed), Ok(x)) => PublicIdentity { ed, x }.fingerprint(),
        _ => String::new(),
    };
    let online = inner
        .mail
        .lock()
        .expect("mail")
        .connections
        .contains_key(id);
    json!({
        "id": id, "name": d.name, "kind": d.kind,
        "ed25519": d.ed25519, "x25519": d.x25519, "fingerprint": fingerprint,
        "online": online, "created_at": 0, "last_seen_at": null, "push": null,
    })
}

fn normalise_code(code: &str) -> String {
    code.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

async fn device_start(State(inner): State<Arc<Inner>>, body: Option<Json<Value>>) -> Response {
    let client_name = body
        .as_ref()
        .and_then(|b| b["client_name"].as_str())
        .unwrap_or("device")
        .to_string();
    let device_code = inner.fresh("dc");
    const LETTERS: &[u8] = b"BCDFGHJKLMNPQRSTVWXZ";
    let random = random32();
    let letters: String = random[..8]
        .iter()
        .map(|b| LETTERS[*b as usize % LETTERS.len()] as char)
        .collect();
    let user_code = format!("{}-{}", &letters[..4], &letters[4..]);
    inner.codes.lock().expect("codes").insert(
        device_code.clone(),
        DeviceCode {
            user_code: user_code.clone(),
            client_name,
            decision: None,
        },
    );
    Json(json!({
        "device_code": device_code,
        "user_code": user_code,
        "verification_url": "https://app.relay.invalid/device",
        "verification_url_complete": format!("https://app.relay.invalid/device?code={user_code}"),
        "expires_in": 600,
        "interval": 1,
    }))
    .into_response()
}

async fn device_lookup(
    State(inner): State<Arc<Inner>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Err(e) = authed(&inner, &headers) {
        return *e;
    }
    let wanted = normalise_code(body["user_code"].as_str().unwrap_or(""));
    let codes = inner.codes.lock().expect("codes");
    match codes
        .values()
        .find(|c| normalise_code(&c.user_code) == wanted)
    {
        Some(code) => {
            Json(json!({"client_name": code.client_name, "created_at": 0, "expires_in": 600}))
                .into_response()
        }
        None => error(StatusCode::NOT_FOUND, "not_found"),
    }
}

async fn device_approve(
    State(inner): State<Arc<Inner>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let (_, session) = match authed(&inner, &headers) {
        Ok(s) => s,
        Err(e) => return *e,
    };
    if session.scope != "full" {
        return error(StatusCode::FORBIDDEN, "insufficient_scope");
    }
    let wanted = normalise_code(body["user_code"].as_str().unwrap_or(""));
    let approve = body["approve"].as_bool().unwrap_or(true);
    let mut codes = inner.codes.lock().expect("codes");
    match codes
        .values_mut()
        .find(|c| normalise_code(&c.user_code) == wanted)
    {
        Some(code) => {
            code.decision = Some(approve.then_some(session.account));
            StatusCode::NO_CONTENT.into_response()
        }
        None => error(StatusCode::NOT_FOUND, "not_found"),
    }
}

async fn device_token(State(inner): State<Arc<Inner>>, Json(body): Json<Value>) -> Response {
    let code = body["device_code"].as_str().unwrap_or("");
    let auto = inner.auto_approve.lock().expect("approve").clone();
    let account = {
        let mut codes = inner.codes.lock().expect("codes");
        let Some(state) = codes.get_mut(code) else {
            return error(StatusCode::BAD_REQUEST, "invalid_grant");
        };
        if state.decision.is_none()
            && let Some(account) = auto
        {
            state.decision = Some(Some(account));
        }
        match state.decision.clone() {
            None => return error(StatusCode::BAD_REQUEST, "authorization_pending"),
            Some(None) => {
                codes.remove(code);
                return error(StatusCode::BAD_REQUEST, "access_denied");
            }
            Some(Some(account)) => {
                codes.remove(code);
                account
            }
        }
    };
    let session = inner.new_session(&account, None, "host");
    Json(inner.tokens(&session)).into_response()
}

async fn refresh(State(inner): State<Arc<Inner>>, Json(body): Json<Value>) -> Response {
    let token = body["refresh_token"].as_str().unwrap_or("");
    let found = inner.refresh.lock().expect("refresh").get(token).cloned();
    match found {
        None => error(StatusCode::UNAUTHORIZED, "invalid_token"),
        // A used refresh token presented again revokes the whole session.
        Some((session, false)) => {
            inner.revoke_session(&session);
            error(StatusCode::UNAUTHORIZED, "token_reused")
        }
        Some((session, true)) => {
            inner
                .refresh
                .lock()
                .expect("refresh")
                .insert(token.into(), (session.clone(), false));
            Json(inner.tokens(&session)).into_response()
        }
    }
}

async fn register(
    State(inner): State<Arc<Inner>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let (session_id, session) = match authed(&inner, &headers) {
        Ok(s) => s,
        Err(e) => return *e,
    };
    if session.device.is_some() {
        return error(StatusCode::CONFLICT, "session_has_device");
    }
    let kind = body["kind"].as_str().unwrap_or("");
    if session.scope == "host" && kind != "host" {
        return error(StatusCode::FORBIDDEN, "insufficient_scope");
    }
    if inner.account_devices(&session.account).len() >= 20 {
        return error(StatusCode::CONFLICT, "too_many_devices");
    }
    if !["web", "desktop", "ios", "android", "host"].contains(&kind) {
        return error(StatusCode::BAD_REQUEST, "invalid_kind");
    }
    let (ed25519, x25519) = (
        body["ed25519"].as_str().unwrap_or(""),
        body["x25519"].as_str().unwrap_or(""),
    );
    if unb64_32(ed25519).is_err() || unb64_32(x25519).is_err() {
        return error(StatusCode::BAD_REQUEST, "invalid_key");
    }
    let id = inner.fresh("dev");
    let device = MockDevice {
        account: session.account.clone(),
        name: body["name"].as_str().unwrap_or("").into(),
        kind: kind.into(),
        ed25519: ed25519.into(),
        x25519: x25519.into(),
    };
    inner
        .devices
        .lock()
        .expect("devices")
        .insert(id.clone(), device.clone());
    if let Some(s) = inner
        .sessions
        .lock()
        .expect("sessions")
        .get_mut(&session_id)
    {
        s.device = Some(id.clone());
    }
    (StatusCode::CREATED, Json(device_json(&inner, &id, &device))).into_response()
}

async fn list_devices(State(inner): State<Arc<Inner>>, headers: HeaderMap) -> Response {
    let (_, session) = match authed(&inner, &headers) {
        Ok(s) => s,
        Err(e) => return *e,
    };
    let devices: Vec<Value> = inner
        .devices
        .lock()
        .expect("devices")
        .clone()
        .iter()
        .filter(|(_, d)| d.account == session.account)
        .map(|(id, d)| device_json(&inner, id, d))
        .collect();
    Json(json!({"devices": devices})).into_response()
}

async fn hosts(State(inner): State<Arc<Inner>>, headers: HeaderMap) -> Response {
    let (_, session) = match authed(&inner, &headers) {
        Ok(s) => s,
        Err(e) => return *e,
    };
    let hosts: Vec<Value> = inner
        .devices
        .lock()
        .expect("devices")
        .clone()
        .iter()
        .filter(|(_, d)| d.account == session.account && d.kind == "host")
        .map(|(id, d)| device_json(&inner, id, d))
        .collect();
    Json(json!({"hosts": hosts})).into_response()
}

async fn put_grants(
    State(inner): State<Arc<Inner>>,
    Path(host): Path<String>,
    headers: HeaderMap,
    Json(body): Json<SignedGrantList>,
) -> Response {
    let (_, session) = match authed(&inner, &headers) {
        Ok(s) => s,
        Err(e) => return *e,
    };
    if session.device.as_deref() != Some(host.as_str()) {
        return error(StatusCode::FORBIDDEN, "forbidden");
    }
    let Some(device) = inner.devices.lock().expect("devices").get(&host).cloned() else {
        return error(StatusCode::NOT_FOUND, "not_found");
    };
    // As the real relay: the host's registered key over
    // "hame-grants:v1\n{host}\n{version}\n" ‖ document, with a version
    // above the stored one.
    let stored = inner
        .grants
        .lock()
        .expect("grants")
        .get(&host)
        .map_or(0, |g| g.version);
    if body.version < 1 || body.version <= stored {
        return error(StatusCode::CONFLICT, "stale_version");
    }
    let (Ok(ed), Ok(document), Ok(signature)) = (
        unb64_32(&device.ed25519),
        unb64(&body.document),
        unb64(&body.signature),
    ) else {
        return error(StatusCode::BAD_REQUEST, "bad_request");
    };
    let Ok(signature) = <[u8; 64]>::try_from(signature) else {
        return error(StatusCode::BAD_REQUEST, "bad_signature");
    };
    let message = crate::pairing::grant_list_message(&host, body.version, &document);
    if verify_signature(&ed, &message, &signature).is_err() {
        return error(StatusCode::BAD_REQUEST, "bad_signature");
    }
    inner.grants.lock().expect("grants").insert(host, body);
    StatusCode::NO_CONTENT.into_response()
}

async fn get_grants(
    State(inner): State<Arc<Inner>>,
    Path(host): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(e) = authed(&inner, &headers) {
        return *e;
    }
    match inner.grants.lock().expect("grants").get(&host) {
        Some(g) => Json(json!({"host_id": host, "version": g.version, "document": g.document, "signature": g.signature, "updated_at": 0}))
            .into_response(),
        None => error(StatusCode::NOT_FOUND, "not_found"),
    }
}

async fn ws(
    State(inner): State<Arc<Inner>>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    let Some(device) = query.get("device").cloned() else {
        return error(StatusCode::BAD_REQUEST, "bad_request");
    };
    let (_, session) = match authed(&inner, &headers) {
        Ok(s) => s,
        Err(e) => return *e,
    };
    if session.device.as_deref() != Some(device.as_str()) {
        return error(StatusCode::FORBIDDEN, "forbidden");
    }
    upgrade
        .protocols([PROTOCOL])
        .on_upgrade(move |socket| connection(inner, device, session.account, socket))
}

fn text(frame: &RelayFrame) -> Message {
    Message::Text(serde_json::to_string(frame).expect("frame").into())
}

fn send_error(code: &str, message: &str, to: Option<&str>) -> RelayFrame {
    RelayFrame::Error {
        code: code.into(),
        message: message.into(),
        to: to.map(str::to_string),
    }
}

async fn connection(inner: Arc<Inner>, device: String, account: String, socket: WebSocket) {
    let (mut sink, mut stream) = socket.split();
    let id = inner.counter.fetch_add(1, Ordering::SeqCst);
    let (tx, mut rx) = mpsc::unbounded_channel();
    let writer = tokio::spawn(async move {
        while let Some(command) = rx.recv().await {
            match command {
                Command::Frame(frame) => {
                    if sink.send(text(&frame)).await.is_err() {
                        break;
                    }
                }
                Command::Close => {
                    let _ = sink.send(Message::Close(None)).await;
                    break;
                }
            }
        }
    });
    let reply = |frame: RelayFrame| {
        let _ = tx.send(Command::Frame(frame));
    };
    let mut resumed = false;
    while let Some(Ok(message)) = stream.next().await {
        let Message::Text(raw) = message else {
            if matches!(message, Message::Close(_)) {
                break;
            }
            continue;
        };
        let Ok(frame) = serde_json::from_str::<RelayFrame>(&raw) else {
            reply(send_error("bad_request", "not a relay frame", None));
            continue;
        };
        if !resumed {
            resumed = true;
            // `resume {after}` acknowledges up to `after`; any other first
            // frame resumes from the last acknowledged seq.
            if let RelayFrame::Resume { after } = frame {
                inner.acknowledge(&device, after);
            }
            let online: Vec<String> = {
                let mut mail = inner.mail.lock().expect("mail");
                if let Some((_, old)) = mail.connections.insert(device.clone(), (id, tx.clone())) {
                    let _ = old.send(Command::Frame(send_error(
                        "forbidden",
                        "replaced by a newer connection",
                        None,
                    )));
                    let _ = old.send(Command::Close);
                }
                for (seq, from, envelope) in mail.queues.entry(device.clone()).or_default().iter() {
                    let _ = tx.send(Command::Frame(RelayFrame::Deliver {
                        seq: *seq,
                        from: from.clone(),
                        envelope: envelope.clone(),
                    }));
                }
                mail.connections.keys().cloned().collect()
            };
            for peer in inner.account_devices(&account) {
                if peer != device && online.contains(&peer) {
                    reply(RelayFrame::Presence {
                        device: peer,
                        online: true,
                    });
                }
            }
            inner.presence(&device, true);
            if matches!(frame, RelayFrame::Resume { .. }) {
                continue;
            }
        }
        match frame {
            RelayFrame::Send { to, envelope, .. } => {
                let size = unb64(&envelope).map(|b| b.len());
                match size {
                    Err(_) => {
                        reply(send_error(
                            "bad_request",
                            "envelope must be base64url",
                            Some(&to),
                        ));
                        continue;
                    }
                    Ok(n) if n > MAX_ENVELOPE => {
                        reply(send_error(
                            "too_large",
                            "envelopes are at most 1 MiB",
                            Some(&to),
                        ));
                        continue;
                    }
                    Ok(_) => {}
                }
                if to == device {
                    reply(send_error("forbidden", "cannot send to oneself", Some(&to)));
                    continue;
                }
                let same_account = inner
                    .devices
                    .lock()
                    .expect("devices")
                    .get(&to)
                    .is_some_and(|d| d.account == account);
                if !same_account {
                    reply(send_error("not_found", "no such device", Some(&to)));
                    continue;
                }
                let seq = inner.enqueue(&device, &to, &envelope);
                reply(RelayFrame::Sent { to, seq });
            }
            RelayFrame::Ack { seq } => inner.acknowledge(&device, seq),
            RelayFrame::Ping => reply(RelayFrame::Pong),
            RelayFrame::Auth { token, .. } => {
                let same_device = token
                    .as_deref()
                    .and_then(|t| inner.session_of(t))
                    .is_some_and(|(_, s)| s.device.as_deref() == Some(device.as_str()));
                if same_device {
                    *inner
                        .renewals
                        .lock()
                        .expect("renewals")
                        .entry(device.clone())
                        .or_insert(0) += 1;
                    reply(RelayFrame::Auth {
                        token: None,
                        expires_at: Some(crate::pairing::now_unix() as i64 + 900),
                    });
                } else {
                    reply(send_error("unauthorized", "bad token", None));
                    let _ = tx.send(Command::Close);
                    break;
                }
            }
            RelayFrame::Resume { .. } => reply(send_error(
                "bad_request",
                "resume only as the first frame",
                None,
            )),
            _ => {}
        }
    }
    let current = {
        let mut mail = inner.mail.lock().expect("mail");
        if mail.connections.get(&device).is_some_and(|(c, _)| *c == id) {
            mail.connections.remove(&device);
            true
        } else {
            false
        }
    };
    let _ = tx.send(Command::Close);
    let _ = writer.await;
    if current {
        inner.presence(&device, false);
    }
}
