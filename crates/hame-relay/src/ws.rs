//! The WebSocket at `/v1/ws`: per-recipient queues of opaque envelopes,
//! delivery, acknowledgements, resume and presence.
//!
//! Frames (JSON text):
//!
//! ```text
//! client → relay  {"op":"send","to":"<device id>","envelope":"<b64url>","push":false}
//! relay → client  {"op":"sent","to":"<device id>","seq":41}
//! relay → client  {"op":"deliver","seq":42,"from":"<device id>","envelope":"<b64url>"}
//! client → relay  {"op":"ack","seq":42}
//! client → relay  {"op":"resume","after":40}
//! relay → client  {"op":"presence","device":"<id>","online":true}
//! relay → client  {"op":"error","code":"...","message":"..."}
//! both            {"op":"ping"} / {"op":"pong"}
//! ```
//!
//! The relay never looks inside an envelope; it checks only that it is
//! base64url and at most 1 MiB once decoded.

use crate::auth::{authenticate, bearer_token};
use crate::error::ApiError;
use crate::push::{PushError, PushMessage, PushProvider};
use crate::ratelimit::{Limit, LocalBucket};
use crate::repo::{AuthContext, EnqueueOutcome};
use crate::state::AppState;
use crate::util;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use tokio::sync::mpsc;

/// The largest envelope, decoded.
pub const MAX_ENVELOPE: usize = 1024 * 1024;
/// The largest WebSocket message accepted (an envelope in base64 plus the
/// frame around it, with room to answer `too_large` rather than drop the
/// connection).
pub const MAX_MESSAGE: usize = 2 * 1024 * 1024;
/// The WebSocket subprotocol; browsers, which cannot set headers on a
/// WebSocket, send the token as a second protocol `bearer.<token>`.
pub const PROTOCOL: &str = "hame-relay.v1";

/// What the hub tells a connection.
#[derive(Debug)]
pub enum HubMsg {
    /// New envelopes may be queued.
    Wake,
    Presence {
        device: String,
        online: bool,
    },
    /// Close the connection (revoked, signed out or replaced).
    Kick,
}

struct ConnHandle {
    id: u64,
    tx: mpsc::UnboundedSender<HubMsg>,
}

#[derive(Default)]
struct HubInner {
    by_account: HashMap<String, HashMap<String, ConnHandle>>,
    account_of: HashMap<String, String>,
}

/// The live connections of this relay process.
#[derive(Default)]
pub struct Hub {
    inner: Mutex<HubInner>,
    next_id: AtomicU64,
}

impl Hub {
    fn register(&self, account: &str, device: &str, tx: mpsc::UnboundedSender<HubMsg>) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let Ok(mut inner) = self.inner.lock() else {
            return id;
        };
        inner
            .account_of
            .insert(device.to_string(), account.to_string());
        let conns = inner.by_account.entry(account.to_string()).or_default();
        if let Some(old) = conns.insert(device.to_string(), ConnHandle { id, tx }) {
            let _ = old.tx.send(HubMsg::Kick);
        }
        for (other, c) in conns.iter() {
            if other != device {
                let _ = c.tx.send(HubMsg::Presence {
                    device: device.to_string(),
                    online: true,
                });
            }
        }
        id
    }

    fn unregister(&self, account: &str, device: &str, id: u64) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        let Some(conns) = inner.by_account.get_mut(account) else {
            return;
        };
        if conns.get(device).map(|c| c.id) != Some(id) {
            return; // Replaced by a newer connection.
        }
        conns.remove(device);
        for c in conns.values() {
            let _ = c.tx.send(HubMsg::Presence {
                device: device.to_string(),
                online: false,
            });
        }
        if conns.is_empty() {
            inner.by_account.remove(account);
        }
        inner.account_of.remove(device);
    }

    fn send(&self, device: &str, msg: HubMsg) {
        let Ok(inner) = self.inner.lock() else {
            return;
        };
        if let Some(c) = inner
            .account_of
            .get(device)
            .and_then(|a| inner.by_account.get(a))
            .and_then(|m| m.get(device))
        {
            let _ = c.tx.send(msg);
        }
    }

    /// Tells the device's connection that envelopes are waiting.
    pub fn wake(&self, device: &str) {
        self.send(device, HubMsg::Wake);
    }

    /// Closes the device's connection, if any.
    pub fn kick(&self, device: &str) {
        self.send(device, HubMsg::Kick);
    }

    pub fn is_online(&self, device: &str) -> bool {
        self.inner
            .lock()
            .map(|i| i.account_of.contains_key(device))
            .unwrap_or(false)
    }

    fn online_devices(&self, account: &str) -> Vec<String> {
        self.inner
            .lock()
            .ok()
            .and_then(|i| {
                i.by_account
                    .get(account)
                    .map(|m| m.keys().cloned().collect())
            })
            .unwrap_or_default()
    }

    /// Number of open connections.
    pub fn connections(&self) -> usize {
        self.inner.lock().map(|i| i.account_of.len()).unwrap_or(0)
    }
}

#[derive(Deserialize)]
pub struct WsQuery {
    device: String,
}

/// `GET /v1/ws?device=<id>`.
pub async fn handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<WsQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    let token = bearer_token(&headers).map(str::to_string).or_else(|| {
        headers
            .get_all(axum::http::header::SEC_WEBSOCKET_PROTOCOL)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(','))
            .find_map(|p| p.trim().strip_prefix("bearer.").map(str::to_string))
    });
    let Some(token) = token else {
        return ApiError::unauthorized().into_response();
    };
    let ctx = match authenticate(&state, &token).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };
    if ctx.device_id.as_deref() != Some(q.device.as_str()) {
        return ApiError::forbidden("the token does not belong to this device").into_response();
    }
    ws.protocols([PROTOCOL])
        .max_message_size(MAX_MESSAGE)
        .max_frame_size(MAX_MESSAGE)
        .on_upgrade(move |socket| async move {
            Connection::new(state, ctx, q.device, socket).run().await;
        })
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
enum ClientFrame {
    Send {
        to: String,
        envelope: String,
        #[serde(default)]
        push: bool,
    },
    Ack {
        seq: i64,
    },
    Resume {
        after: i64,
    },
    Ping,
    Pong,
}

fn error_frame(code: &str, message: &str, to: Option<&str>) -> Value {
    let mut v = json!({ "op": "error", "code": code, "message": message });
    if let Some(to) = to {
        v["to"] = json!(to);
    }
    v
}

struct Connection {
    state: AppState,
    account: String,
    device: String,
    socket: WebSocket,
    resumed: bool,
    last_delivered: i64,
}

/// Why the loop stops.
struct Closed;

impl Connection {
    fn new(state: AppState, ctx: AuthContext, device: String, socket: WebSocket) -> Self {
        Self {
            state,
            account: ctx.account_id,
            device,
            socket,
            resumed: false,
            last_delivered: 0,
        }
    }

    async fn send(&mut self, v: Value) -> Result<(), Closed> {
        self.socket
            .send(Message::Text(v.to_string().into()))
            .await
            .map_err(|_| Closed)
    }

    async fn run(mut self) {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let conn_id = self.state.hub.register(&self.account, &self.device, tx);
        let _ = self
            .state
            .repo
            .touch_device(&self.device, util::now())
            .await;
        tracing::debug!(device = %self.device, "websocket connected");

        let cfg = &self.state.config;
        let rate = cfg.rate_limits.ws_frames_per_sec;
        let mut bucket = LocalBucket::new(rate, rate.saturating_mul(4));
        let idle = cfg.ws_idle_timeout;
        let mut ping = tokio::time::interval(cfg.ws_ping_interval);
        ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ping.tick().await;
        let mut last_rx = Instant::now();

        loop {
            let step: Result<(), Closed> = tokio::select! {
                msg = self.socket.recv() => match msg {
                    None | Some(Err(_)) | Some(Ok(Message::Close(_))) => Err(Closed),
                    Some(Ok(Message::Text(text))) => {
                        last_rx = Instant::now();
                        if bucket.take() {
                            self.on_text(text.as_str()).await
                        } else {
                            self.send(error_frame("rate_limited", "too many frames", None)).await
                        }
                    }
                    Some(Ok(Message::Binary(_))) => {
                        last_rx = Instant::now();
                        self.send(error_frame("bad_request", "binary frames are not supported", None)).await
                    }
                    Some(Ok(_)) => {
                        last_rx = Instant::now();
                        Ok(())
                    }
                },
                m = rx.recv() => match m {
                    Some(HubMsg::Wake) => {
                        if self.resumed { self.deliver_pending().await } else { Ok(()) }
                    }
                    Some(HubMsg::Presence { device, online }) => {
                        self.send(json!({ "op": "presence", "device": device, "online": online })).await
                    }
                    Some(HubMsg::Kick) | None => {
                        let _ = self.send(error_frame("forbidden", "connection closed by the relay", None)).await;
                        Err(Closed)
                    }
                },
                _ = ping.tick() => {
                    if last_rx.elapsed() > idle {
                        Err(Closed)
                    } else {
                        self.send(json!({ "op": "ping" })).await
                    }
                }
            };
            if step.is_err() {
                break;
            }
        }
        let _ = self.socket.send(Message::Close(None)).await;
        self.state
            .hub
            .unregister(&self.account, &self.device, conn_id);
        let _ = self
            .state
            .repo
            .touch_device(&self.device, util::now())
            .await;
        tracing::debug!(device = %self.device, "websocket closed");
    }

    async fn on_text(&mut self, text: &str) -> Result<(), Closed> {
        let frame: ClientFrame = match serde_json::from_str(text) {
            Ok(f) => f,
            Err(_) => {
                return self
                    .send(error_frame(
                        "bad_request",
                        "unknown or malformed frame",
                        None,
                    ))
                    .await;
            }
        };
        if !self.resumed && !matches!(frame, ClientFrame::Resume { .. }) {
            // No resume first: continue from the last acknowledged envelope.
            let after = match self.state.repo.device(&self.device).await {
                Ok(Some(d)) => d.acked_seq,
                _ => 0,
            };
            self.resume(after).await?;
        }
        match frame {
            ClientFrame::Send { to, envelope, push } => self.on_send(to, envelope, push).await,
            ClientFrame::Ack { seq } => {
                let upto = seq.min(self.last_delivered);
                if let Err(e) = self.state.repo.ack(&self.device, upto).await {
                    tracing::error!(error = %e, "ack failed");
                    return self.send(error_frame("internal", "ack failed", None)).await;
                }
                Ok(())
            }
            ClientFrame::Resume { after } => self.resume(after).await,
            ClientFrame::Ping => self.send(json!({ "op": "pong" })).await,
            ClientFrame::Pong => Ok(()),
        }
    }

    async fn resume(&mut self, after: i64) -> Result<(), Closed> {
        let last_seq = match self.state.repo.device(&self.device).await {
            Ok(Some(d)) => d.last_seq,
            _ => 0,
        };
        let after = after.clamp(0, last_seq);
        // Everything up to `after` has arrived.
        if let Err(e) = self.state.repo.ack(&self.device, after).await {
            tracing::error!(error = %e, "resume ack failed");
        }
        self.last_delivered = after;
        self.resumed = true;
        self.deliver_pending().await?;
        for other in self.state.hub.online_devices(&self.account) {
            if other != self.device {
                self.send(json!({ "op": "presence", "device": other, "online": true }))
                    .await?;
            }
        }
        Ok(())
    }

    async fn deliver_pending(&mut self) -> Result<(), Closed> {
        loop {
            let items = match self
                .state
                .repo
                .fetch_after(&self.device, self.last_delivered, 64)
                .await
            {
                Ok(items) => items,
                Err(e) => {
                    tracing::error!(error = %e, "fetching queued envelopes failed");
                    return Ok(());
                }
            };
            if items.is_empty() {
                return Ok(());
            }
            for item in items {
                self.send(json!({
                    "op": "deliver",
                    "seq": item.seq,
                    "from": item.sender_id,
                    "envelope": util::b64url(&item.envelope),
                }))
                .await?;
                self.last_delivered = item.seq;
            }
        }
    }

    async fn on_send(&mut self, to: String, envelope: String, push: bool) -> Result<(), Closed> {
        // A cheap bound before decoding.
        if envelope.len() > MAX_ENVELOPE.div_ceil(3) * 4 + 4 {
            return self
                .send(error_frame(
                    "too_large",
                    "envelope exceeds 1 MiB",
                    Some(&to),
                ))
                .await;
        }
        let Some(bytes) = util::b64url_decode(&envelope) else {
            return self
                .send(error_frame(
                    "bad_request",
                    "envelope must be base64url",
                    Some(&to),
                ))
                .await;
        };
        if bytes.len() > MAX_ENVELOPE {
            return self
                .send(error_frame(
                    "too_large",
                    "envelope exceeds 1 MiB",
                    Some(&to),
                ))
                .await;
        }
        if to == self.device {
            return self
                .send(error_frame(
                    "forbidden",
                    "cannot send to yourself",
                    Some(&to),
                ))
                .await;
        }
        // Devices of other accounts look exactly like unknown ones.
        let recipient = match self.state.repo.device(&to).await {
            Ok(Some(d)) if d.account_id == self.account && d.revoked_at.is_none() => d,
            Ok(_) => {
                return self
                    .send(error_frame("not_found", "no such device", Some(&to)))
                    .await;
            }
            Err(e) => {
                tracing::error!(error = %e, "device lookup failed");
                return self
                    .send(error_frame("internal", "try again", Some(&to)))
                    .await;
            }
        };
        let cfg = &self.state.config;
        let outcome = self
            .state
            .repo
            .enqueue(
                &to,
                &self.device,
                &bytes,
                util::now(),
                cfg.mailbox_max_per_device,
            )
            .await;
        let seq = match outcome {
            Ok(EnqueueOutcome::Queued(seq)) => seq,
            Ok(EnqueueOutcome::Full) => {
                return self
                    .send(error_frame(
                        "rate_limited",
                        "the recipient's queue is full",
                        Some(&to),
                    ))
                    .await;
            }
            Err(e) => {
                tracing::error!(error = %e, "enqueue failed");
                return self
                    .send(error_frame("internal", "try again", Some(&to)))
                    .await;
            }
        };
        self.send(json!({ "op": "sent", "to": to, "seq": seq }))
            .await?;
        let online = self.state.hub.is_online(&to);
        self.state.hub.wake(&to);
        if push && !online {
            self.push(recipient, seq);
        }
        Ok(())
    }

    /// Sends an opaque push to an offline mobile device (at most two a
    /// minute per device).
    fn push(&self, recipient: crate::repo::Device, seq: i64) {
        let (Some(provider), Some(token)) = (
            recipient
                .push_provider
                .as_deref()
                .and_then(PushProvider::parse),
            recipient.push_token.clone(),
        ) else {
            return;
        };
        if !matches!(recipient.kind.as_str(), "ios" | "android") {
            return;
        }
        let limit = Limit {
            burst: 1,
            per_minute: 2,
        };
        if self
            .state
            .limiter
            .check(&format!("push:{}", recipient.id), limit)
            .is_err()
        {
            return;
        }
        let state = self.state.clone();
        let msg = PushMessage {
            provider,
            token,
            recipient: recipient.id.clone(),
            sender: self.device.clone(),
            seq,
        };
        tokio::spawn(async move {
            match state.push.send(&msg).await {
                Ok(()) => {}
                Err(PushError::Unregistered) => {
                    let _ = state.repo.set_push_token(&msg.recipient, None, None).await;
                }
                Err(e) => tracing::warn!(error = %e, device = %msg.recipient, "push failed"),
            }
        });
    }
}
