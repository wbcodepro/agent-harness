//! The host connector: takes envelopes from the relay, accepts only those
//! from devices on the host's allowlist (or a pairing request answering the
//! host's current QR offer), and runs the remote API frames they carry
//! against the hame web router **in process**, exactly as a local page
//! would. Answers, stream events and artifact chunks go back sealed to the
//! device.
//!
//! Nothing else is exposed: no shell, file or generic RPC. Every remote
//! action goes through the hame web API and so through the agent's
//! permission engine and sandbox.
//!
//! The allowlist on disk is the authority. Every change the connector makes
//! is a read-modify-write under the store's lock, and a revocation made by
//! another process (`hame connect revoke`) is taken in before the next
//! envelope is handled and within half a second otherwise: the device's
//! envelopes are refused, its streams and requests stop, and the turns it
//! started are cancelled.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{Method, Request, header};
use base64::Engine;
use futures_util::StreamExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio::task::AbortHandle;
use tower::ServiceExt;

use crate::SyncError;
use crate::approval::{PendingApprovals, approval_session, events_session, looks_like_approval};
use crate::envelope::{Counter, Header, MAX_PLAINTEXT, Sealed, seal};
use crate::frames::{BLOB_CHUNK, Frame};
use crate::keys::{Identity, PublicIdentity, b64, hex, random32, unb64, unb64_32};
use crate::pairing::{
    Grant, GrantEntry, GrantList, PairingOffer, SignedGrantList, check_pairing_proof, now_unix, sas,
};
use crate::relay::{RelayEvent, RelayHandle};
use crate::store::{AllowedDevice, Allowlist, HostStore, Stamp};
use crate::text::{is_bidi_format, valid_id};

/// The largest artifact sent to a device.
pub const MAX_BLOB: usize = 64 * 1024 * 1024;
/// Requests and subscriptions one device may have running at once.
pub const MAX_TASKS_PER_DEVICE: usize = 32;
/// Subscriptions one device may have open at once.
pub const MAX_SUBSCRIPTIONS_PER_DEVICE: usize = 8;
/// Sessions one device may create per minute.
pub const SESSIONS_PER_MINUTE: usize = 10;
/// Host counters reserved on disk ahead of use (see [`Inner::send_to`]).
const COUNTER_RESERVE: u64 = 256;
const MAX_LABEL: usize = 64;
const MAX_PATH: usize = 2048;

/// What the host user is asked before a device is enrolled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairingPrompt {
    pub device: String,
    pub label: String,
    /// The device's key fingerprint, grouped for reading.
    pub fingerprint: String,
    /// The eight digits the device shows too.
    pub sas: String,
}

/// Something the host user or a log should know about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Notice {
    /// An envelope was dropped.
    Rejected {
        from: String,
        reason: String,
    },
    Paired {
        device: String,
        label: String,
    },
    PairingRefused {
        device: String,
        reason: String,
    },
    /// A known device presented a different key; refused.
    KeyChanged {
        device: String,
    },
    Revoked {
        device: String,
        by: String,
    },
    Relay(String),
}

pub type ConfirmFn = Arc<
    dyn Fn(PairingPrompt) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send>>
        + Send
        + Sync,
>;
pub type NoticeFn = Arc<dyn Fn(Notice) + Send + Sync>;
pub type GrantsFn = Arc<dyn Fn(SignedGrantList) + Send + Sync>;

pub struct ConnectorOptions {
    /// This host's device id at the relay.
    pub device_id: String,
    /// The hame web router and its token (`hame_web::router`).
    pub router: axum::Router,
    pub token: String,
    pub store: HostStore,
    /// Asks the user on the host to confirm a pairing (compare the SAS).
    pub confirm: ConfirmFn,
    pub notice: NoticeFn,
    /// Called with the signed list of accepted devices after it changes,
    /// to publish it to the relay.
    pub grants_changed: Option<GrantsFn>,
    /// Include device labels in the grant list the relay stores. Off by
    /// default: labels are the user's words and the relay needs none.
    pub publish_labels: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TaskKind {
    Request,
    Subscription,
}

struct Sender {
    counter: Counter,
    /// Counters up to here are reserved on disk.
    reserved: u64,
}

struct Inner {
    identity: Identity,
    device_id: String,
    store: HostStore,
    /// The allowlist as last read or written; the file is the authority.
    allowlist: Mutex<Allowlist>,
    stamp: Mutex<Stamp>,
    router: axum::Router,
    token: String,
    confirm: ConfirmFn,
    notice: NoticeFn,
    grants_changed: Option<GrantsFn>,
    publish_labels: bool,
    offer: Mutex<Option<PairingOffer>>,
    approvals: PendingApprovals,
    tasks: Mutex<HashMap<(String, u64), (AbortHandle, TaskKind)>>,
    /// Sessions each device created or prompted, to cancel on revocation.
    sessions: Mutex<HashMap<String, HashSet<String>>>,
    /// When each device created its recent sessions.
    created: Mutex<HashMap<String, VecDeque<Instant>>>,
    /// Held while a frame is sealed and queued, so envelopes reach each
    /// device in counter order.
    sender: tokio::sync::Mutex<Sender>,
    relay: RelayHandle,
}

/// A running host connector. Cheap to clone.
#[derive(Clone)]
pub struct Connector {
    inner: Arc<Inner>,
}

impl Connector {
    pub fn new(options: ConnectorOptions, relay: RelayHandle) -> Result<Self, SyncError> {
        let identity = options.store.identity()?;
        let allowlist = options.store.allowlist()?;
        let host_counter = allowlist.host_counter;
        let stamp = options.store.allowlist_stamp();
        Ok(Self {
            inner: Arc::new(Inner {
                identity,
                device_id: options.device_id,
                store: options.store,
                allowlist: Mutex::new(allowlist),
                stamp: Mutex::new(stamp),
                router: options.router,
                token: options.token,
                confirm: options.confirm,
                notice: options.notice,
                grants_changed: options.grants_changed,
                publish_labels: options.publish_labels,
                offer: Mutex::new(None),
                approvals: PendingApprovals::default(),
                tasks: Mutex::new(HashMap::new()),
                sessions: Mutex::new(HashMap::new()),
                created: Mutex::new(HashMap::new()),
                // Restart above anything that may have been used.
                sender: tokio::sync::Mutex::new(Sender {
                    counter: Counter::starting_after(host_counter),
                    reserved: host_counter,
                }),
                relay,
            }),
        })
    }

    pub fn public(&self) -> PublicIdentity {
        self.inner.identity.public()
    }

    /// Starts a pairing offer (one use, ten minutes), replacing any other.
    pub fn offer_pairing(&self, relay_url: &str) -> PairingOffer {
        let offer = PairingOffer::new(relay_url, &self.inner.device_id, self.public());
        *self.inner.offer.lock().expect("offer") = Some(offer.clone());
        offer
    }

    /// Whether a pairing offer is still open.
    pub fn pairing_open(&self) -> bool {
        let mut offer = self.inner.offer.lock().expect("offer");
        if offer.as_ref().is_some_and(|o| o.expired(now_unix())) {
            *offer = None;
        }
        offer.is_some()
    }

    pub fn devices(&self) -> Vec<AllowedDevice> {
        self.inner
            .allowlist
            .lock()
            .expect("allowlist")
            .devices
            .clone()
    }

    /// Revokes a device: its key is refused from now on, its streams stop
    /// and the turns it started are cancelled.
    pub fn revoke(&self, device: &str) -> Result<bool, SyncError> {
        self.inner.revoke(device, "host")
    }

    /// Publishes the grant list again with a version above `stored` (the
    /// relay holds a list at least that new, e.g. after the host's files
    /// were restored from a backup).
    pub fn publish_grants_above(&self, stored: u64) {
        if let Err(error) = self.inner.update(|list| {
            list.grants_version = list.grants_version.max(stored);
            Ok(())
        }) {
            (self.inner.notice)(Notice::Relay(format!("grant list: {error}")));
            return;
        }
        self.inner.publish_grants();
    }

    /// Handles relay events until the relay connection ends.
    pub async fn run(&self, mut events: mpsc::Receiver<RelayEvent>) {
        // A revocation made by another process takes effect within half a
        // second even if the device sends nothing.
        let mut check = tokio::time::interval(Duration::from_millis(500));
        loop {
            let event = tokio::select! {
                event = events.recv() => event,
                _ = check.tick() => {
                    if let Err(error) = self.inner.reload_if_changed() {
                        (self.inner.notice)(Notice::Relay(format!("allowlist: {error}")));
                    }
                    continue;
                }
            };
            let Some(event) = event else { break };
            match event {
                RelayEvent::Deliver {
                    seq,
                    from,
                    envelope,
                } => {
                    if let Err(error) = Arc::clone(&self.inner).deliver(&from, &envelope).await {
                        (self.inner.notice)(Notice::Rejected {
                            from,
                            reason: error.to_string(),
                        });
                    }
                    // Handled or refused, the relay may drop it.
                    let _ = self.inner.relay.ack(seq).await;
                }
                RelayEvent::Error { code, message } => {
                    (self.inner.notice)(Notice::Relay(format!("{code}: {message}")))
                }
                RelayEvent::Disconnected(reason) => {
                    (self.inner.notice)(Notice::Relay(format!("disconnected: {reason}")))
                }
                RelayEvent::Connected => (self.inner.notice)(Notice::Relay("connected".into())),
                RelayEvent::Sent { .. } | RelayEvent::Presence { .. } => {}
            }
        }
        for (_, (task, _)) in self.inner.tasks.lock().expect("tasks").drain() {
            task.abort();
        }
    }
}

/// A label as the host keeps and shows it: no control or bidirectional
/// formatting characters, at most 64 characters.
pub fn clean_label(label: &str) -> String {
    let label: String = label
        .chars()
        .filter(|c| !c.is_control() && !is_bidi_format(*c))
        .take(MAX_LABEL)
        .collect();
    match label.trim() {
        "" => "device".into(),
        trimmed => trimmed.into(),
    }
}

/// Who a remote request comes from, attached to every request the
/// connector passes to the hame web router as a request extension. It never
/// comes from HTTP headers, so only the connector can set it; the web layer
/// applies its remote policy to requests that carry it.
///
/// Integration point: when hame-web exposes its own `RemoteCaller` type,
/// replace this struct with a re-export of it (or convert in
/// [`mark_remote`]); nothing else changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteCaller {
    pub device_id: String,
    pub label: String,
}

/// Marks a request as coming from an enrolled remote device.
pub fn mark_remote(request: &mut Request<Body>, device_id: &str, label: &str) {
    request.extensions_mut().insert(RemoteCaller {
        device_id: device_id.into(),
        label: label.into(),
    });
}

/// A path the connector passes to the hame web API: under `/api/`, no
/// traversal, no fragments, printable ASCII.
pub fn allowed_path(path: &str) -> bool {
    path.starts_with("/api/")
        && path.len() <= MAX_PATH
        && !path.contains("..")
        && !path.contains("//")
        && !path.contains('\\')
        && !path.contains('#')
        && path.bytes().all(|b| b.is_ascii_graphic())
}

/// Whether a request body asks for full access anywhere (a mode change).
/// Interim remote policy until hame-web enforces its own for
/// [`RemoteCaller`]s: such requests are refused.
pub fn asks_for_full_access(body: &Value) -> bool {
    match body {
        Value::String(s) => {
            let s = s.to_ascii_lowercase().replace('_', "-");
            s == "danger-full-access" || s == "full-access" || s == "yolo"
        }
        Value::Array(items) => items.iter().any(asks_for_full_access),
        Value::Object(map) => map.values().any(asks_for_full_access),
        _ => false,
    }
}

impl Inner {
    async fn deliver(self: Arc<Self>, relay_from: &str, envelope: &str) -> Result<(), SyncError> {
        let sealed = Sealed::parse(envelope)?;
        let from = sealed.header.from.clone();
        if from != relay_from {
            return Err(SyncError::Malformed(
                "the relay's sender differs from the signed one",
            ));
        }
        if sealed.header.to != self.device_id {
            return Err(SyncError::WrongRecipient);
        }
        // A revocation made elsewhere counts before anything else.
        self.reload_if_changed()?;
        let known = {
            let allowlist = self.allowlist.lock().expect("allowlist");
            allowlist
                .get(&from)
                .map(|device| (device.keys(), device.window))
        };
        let Some((keys, window)) = known else {
            return self.pair(sealed).await;
        };
        let keys = keys?;
        let plain = match sealed.open(&self.identity, &self.device_id, &keys, Some(&window)) {
            Ok(plain) => plain,
            Err(SyncError::BadSignature) => {
                self.check_changed_key(&sealed, &keys).await;
                return Err(SyncError::BadSignature);
            }
            // A device that lost its counter can only pair again.
            Err(error @ SyncError::Replay { .. }) => {
                if self.offer.lock().expect("offer").is_some() {
                    return self.pair(sealed).await;
                }
                return Err(error);
            }
            Err(error) => return Err(error),
        };
        // Record the counter under the lock; a device revoked meanwhile is
        // refused here.
        let counter = sealed.header.counter;
        let pinned = b64(&keys.ed);
        self.update(|list| match list.get_mut(&from) {
            Some(device) if device.ed == pinned => {
                device.window.check(counter)?;
                device.window.record(counter);
                Ok(())
            }
            _ => Err(SyncError::UnknownSender(from.clone())),
        })?;
        let frame: Frame =
            serde_json::from_slice(&plain).map_err(|_| SyncError::Malformed("bad frame"))?;
        if frame.stream() != sealed.header.stream {
            return Err(SyncError::Malformed(
                "frame and header name different streams",
            ));
        }
        self.dispatch(from, frame);
        Ok(())
    }

    /// A read-modify-write of the allowlist under the store's lock; the
    /// result becomes this connector's copy.
    fn update<T>(
        &self,
        change: impl FnOnce(&mut Allowlist) -> Result<T, SyncError>,
    ) -> Result<T, SyncError> {
        let (saved, result) = self.store.update_allowlist(change)?;
        self.adopt(saved);
        Ok(result)
    }

    /// Takes an allowlist read from disk as this connector's copy. Devices
    /// that are gone were revoked: they lose everything at once.
    fn adopt(&self, saved: Allowlist) {
        let gone: Vec<String> = {
            let mut allowlist = self.allowlist.lock().expect("allowlist");
            let gone = allowlist
                .devices
                .iter()
                .filter(|d| saved.get(&d.device).is_none_or(|now| now.ed != d.ed))
                .map(|d| d.device.clone())
                .collect();
            *allowlist = saved;
            gone
        };
        *self.stamp.lock().expect("stamp") = self.store.allowlist_stamp();
        for device in gone {
            self.revoked(&device, "host");
        }
    }

    /// Takes in a change another process made to the allowlist (a
    /// revocation from `hame connect revoke`).
    fn reload_if_changed(&self) -> Result<(), SyncError> {
        let now = self.store.allowlist_stamp();
        if now == *self.stamp.lock().expect("stamp") {
            return Ok(());
        }
        let saved = self.store.allowlist()?;
        self.adopt(saved);
        Ok(())
    }

    /// What follows a revocation: running requests and streams stop, the
    /// turns the device started are cancelled, the grant list changes.
    fn revoked(&self, device: &str, by: &str) {
        self.tasks
            .lock()
            .expect("tasks")
            .retain(|(owner, _), (task, _)| {
                let keep = owner != device;
                if !keep {
                    task.abort();
                }
                keep
            });
        let sessions = self
            .sessions
            .lock()
            .expect("sessions")
            .remove(device)
            .unwrap_or_default();
        for session in sessions {
            let router = self.router.clone();
            let token = self.token.clone();
            tokio::spawn(async move {
                let request = Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/sessions/{session}/cancel"))
                    .header(header::HOST, "127.0.0.1:0")
                    .header("x-hame-token", token)
                    .body(Body::empty());
                if let Ok(request) = request {
                    let _ = router.oneshot(request).await;
                }
            });
        }
        (self.notice)(Notice::Revoked {
            device: device.into(),
            by: by.into(),
        });
        self.publish_grants();
    }

    /// A known device whose envelope does not verify with its pinned key:
    /// if it is a pairing request with a new key, warn, and refuse.
    async fn check_changed_key(self: &Arc<Self>, sealed: &Sealed, pinned: &PublicIdentity) {
        let Ok(plain) = sealed.decrypt(&self.identity) else {
            return;
        };
        let Ok(Frame::Pair { ed, x, .. }) = serde_json::from_slice::<Frame>(&plain) else {
            return;
        };
        let (Ok(ed), Ok(x)) = (unb64_32(&ed), unb64_32(&x)) else {
            return;
        };
        if ed == pinned.ed || sealed.verify(&ed).is_err() {
            return;
        }
        let device = sealed.header.from.clone();
        (self.notice)(Notice::KeyChanged {
            device: device.clone(),
        });
        let _ = self
            .send_to(
                &device,
                &x,
                Frame::PairRefused {
                    reason: SyncError::KeyChanged(device.clone()).to_string(),
                },
            )
            .await;
    }

    /// A pairing request answering the open offer: from a device not on
    /// the allowlist, or from an enrolled one that lost its counter.
    async fn pair(self: Arc<Self>, sealed: Sealed) -> Result<(), SyncError> {
        let from = sealed.header.from.clone();
        let offer = {
            let mut offer = self.offer.lock().expect("offer");
            if offer.as_ref().is_some_and(|o| o.expired(now_unix())) {
                *offer = None;
            }
            offer.clone()
        };
        let Some(offer) = offer else {
            return Err(SyncError::UnknownSender(from));
        };
        let plain = sealed
            .decrypt(&self.identity)
            .map_err(|_| SyncError::UnknownSender(from.clone()))?;
        let Ok(Frame::Pair { ed, x, label, mac }) = serde_json::from_slice::<Frame>(&plain) else {
            return Err(SyncError::UnknownSender(from));
        };
        let keys = PublicIdentity {
            ed: unb64_32(&ed)?,
            x: unb64_32(&x)?,
        };
        // The device holds the key it presents, and read the QR code.
        sealed.verify(&keys.ed)?;
        check_pairing_proof(&offer.secret, &keys, &unb64(&mac)?)?;
        // One use: whatever happens next, this offer is spent.
        {
            let mut current = self.offer.lock().expect("offer");
            if current.as_ref() != Some(&offer) {
                return Err(SyncError::Pairing("the offer was already used".into()));
            }
            *current = None;
        }
        let label = clean_label(&label);
        let refusal = {
            let allowlist = self.allowlist.lock().expect("allowlist");
            if allowlist.is_revoked(&keys) {
                Some("this key was revoked".to_string())
            } else {
                None
            }
        };
        if let Some(reason) = refusal {
            (self.notice)(Notice::PairingRefused {
                device: from.clone(),
                reason: reason.clone(),
            });
            let _ = self
                .send_to(
                    &from,
                    &keys.x,
                    Frame::PairRefused {
                        reason: reason.clone(),
                    },
                )
                .await;
            return Err(SyncError::Pairing(reason));
        }
        // The host's nonce, chosen after the device committed to its keys,
        // goes into the SAS: nobody can search for keys whose digits match.
        let nonce = random32();
        self.send_to(&from, &keys.x, Frame::PairChallenge { nonce: b64(&nonce) })
            .await?;
        let prompt = PairingPrompt {
            device: from.clone(),
            label: label.clone(),
            fingerprint: keys.display_fingerprint(),
            sas: sas(&offer.secret, &self.identity.public().ed, &keys.ed, &nonce),
        };
        let counter = sealed.header.counter;
        let this = Arc::clone(&self);
        tokio::spawn(async move {
            let confirmed = (this.confirm)(prompt).await;
            let outcome = if confirmed {
                this.update(|list| list.add(&from, &label, &keys, counter))
            } else {
                Err(SyncError::Pairing("not confirmed on the host".into()))
            };
            match outcome {
                Ok(()) => {
                    let grant =
                        Grant::new(&this.device_id, &from, &keys, &label).sign(&this.identity);
                    let _ = this
                        .send_to(
                            &from,
                            &keys.x,
                            Frame::Paired {
                                grant: grant.grant,
                                sig: grant.sig,
                            },
                        )
                        .await;
                    (this.notice)(Notice::Paired {
                        device: from,
                        label,
                    });
                    this.publish_grants();
                }
                Err(error) => {
                    if let SyncError::KeyChanged(device) = &error {
                        (this.notice)(Notice::KeyChanged {
                            device: device.clone(),
                        });
                    }
                    let reason = error.to_string();
                    (this.notice)(Notice::PairingRefused {
                        device: from.clone(),
                        reason: reason.clone(),
                    });
                    let _ = this
                        .send_to(&from, &keys.x, Frame::PairRefused { reason })
                        .await;
                }
            }
        });
        Ok(())
    }

    fn publish_grants(&self) {
        let Some(hook) = &self.grants_changed else {
            return;
        };
        let version = match self.update(|list| {
            list.grants_version += 1;
            Ok(list.grants_version)
        }) {
            Ok(version) => version,
            Err(error) => {
                (self.notice)(Notice::Relay(format!("grant list: {error}")));
                return;
            }
        };
        let devices = self
            .allowlist
            .lock()
            .expect("allowlist")
            .devices
            .iter()
            .map(|d| GrantEntry {
                device: d.device.clone(),
                ed: d.ed.clone(),
                x: d.x.clone(),
                label: if self.publish_labels {
                    d.label.clone()
                } else {
                    String::new()
                },
            })
            .collect();
        hook(GrantList::new(&self.device_id, version, devices).sign(&self.identity));
    }

    fn revoke(&self, device: &str, by: &str) -> Result<bool, SyncError> {
        // `adopt` sees the device gone and does the rest.
        let removed = {
            let (saved, removed) = self
                .store
                .update_allowlist(|list| Ok(list.revoke(device)))?;
            let mut allowlist = self.allowlist.lock().expect("allowlist");
            if removed {
                // Report who revoked it rather than "host".
                *allowlist = saved;
            } else {
                drop(allowlist);
                self.adopt(saved);
            }
            removed
        };
        if removed {
            *self.stamp.lock().expect("stamp") = self.store.allowlist_stamp();
            self.revoked(device, by);
        }
        Ok(removed)
    }

    fn running(&self, device: &str, kind: Option<TaskKind>) -> usize {
        self.tasks
            .lock()
            .expect("tasks")
            .iter()
            .filter(|((owner, _), (_, k))| owner == device && kind.is_none_or(|kind| kind == *k))
            .count()
    }

    fn dispatch(self: Arc<Self>, device: String, frame: Frame) {
        match frame {
            Frame::Req {
                id,
                method,
                path,
                body,
            } => {
                if self.running(&device, None) >= MAX_TASKS_PER_DEVICE {
                    self.reply(
                        &device,
                        id,
                        429,
                        json!({"error": "too many requests at once"}),
                    );
                    return;
                }
                let this = Arc::clone(&self);
                let owner = device.clone();
                self.track(&device, id, TaskKind::Request, async move {
                    this.request(&owner, id, &method, &path, body).await;
                });
            }
            Frame::Sub { id, path, after } => {
                if self.running(&device, None) >= MAX_TASKS_PER_DEVICE
                    || self.running(&device, Some(TaskKind::Subscription))
                        >= MAX_SUBSCRIPTIONS_PER_DEVICE
                {
                    let this = Arc::clone(&self);
                    tokio::spawn(async move {
                        let error = Some("too many subscriptions".to_string());
                        let _ = this.send(&device, Frame::End { id, error }).await;
                    });
                    return;
                }
                let this = Arc::clone(&self);
                let owner = device.clone();
                self.track(&device, id, TaskKind::Subscription, async move {
                    let error = this.subscribe(&owner, id, &path, after).await.err();
                    let _ = this.send(&owner, Frame::End { id, error }).await;
                });
            }
            Frame::Cancel { id } => {
                let task = self
                    .tasks
                    .lock()
                    .expect("tasks")
                    .remove(&(device.clone(), id));
                if let Some((task, _)) = task {
                    task.abort();
                    tokio::spawn(async move {
                        let _ = self
                            .send(
                                &device,
                                Frame::End {
                                    id,
                                    error: Some("cancelled".into()),
                                },
                            )
                            .await;
                    });
                }
            }
            Frame::Revoke { id, device: target } => {
                let (status, body) = match self.revoke(&target, &device) {
                    Ok(true) => (200, json!({"revoked": target})),
                    Ok(false) => (404, json!({"error": "no such device"})),
                    Err(e) => (500, json!({"error": e.to_string()})),
                };
                // A device that revoked itself gets no answer.
                self.reply(&device, id, status, body);
            }
            Frame::Devices { id } => {
                let devices: Vec<Value> = self
                    .allowlist
                    .lock()
                    .expect("allowlist")
                    .devices
                    .iter()
                    .map(|d| {
                        let fingerprint = d.keys().map(|k| k.fingerprint()).unwrap_or_default();
                        json!({"device": d.device, "label": d.label, "fingerprint": fingerprint, "added": d.added})
                    })
                    .collect();
                self.reply(&device, id, 200, json!({"devices": devices}));
            }
            // Frames only a host sends, or pairing from a device that is
            // already enrolled: nothing to do.
            other => (self.notice)(Notice::Rejected {
                from: device,
                reason: format!(
                    "unexpected {} frame",
                    serde_json::to_value(&other)
                        .map(|v| v["t"].to_string())
                        .unwrap_or_default()
                ),
            }),
        }
    }

    fn reply(self: &Arc<Self>, device: &str, id: u64, status: u16, body: Value) {
        let this = Arc::clone(self);
        let device = device.to_string();
        tokio::spawn(async move {
            let _ = this.send(&device, Frame::Res { id, status, body }).await;
        });
    }

    /// Runs `work` as the task of stream `id` of `device`, so a `cancel`
    /// or a revocation can stop it.
    fn track(
        self: &Arc<Self>,
        device: &str,
        id: u64,
        kind: TaskKind,
        work: impl std::future::Future<Output = ()> + Send + 'static,
    ) {
        let key = (device.to_string(), id);
        let this = Arc::clone(self);
        let done = key.clone();
        let mut tasks = self.tasks.lock().expect("tasks");
        if let Some((previous, _)) = tasks.remove(&key) {
            previous.abort();
        }
        let task = tokio::spawn(async move {
            work.await;
            this.tasks.lock().expect("tasks").remove(&done);
        });
        tasks.insert(key, (task.abort_handle(), kind));
    }

    async fn call(
        &self,
        device: &str,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<axum::response::Response, (u16, String)> {
        let method = match method.to_ascii_uppercase().as_str() {
            "GET" => Method::GET,
            "POST" => Method::POST,
            "PUT" => Method::PUT,
            "PATCH" => Method::PATCH,
            "DELETE" => Method::DELETE,
            _ => return Err((405, "method not allowed".into())),
        };
        if !allowed_path(path) {
            return Err((400, "only /api/ paths are served".into()));
        }
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, "127.0.0.1:0")
            .header("x-hame-token", &self.token);
        let bytes = match body {
            Some(body) => {
                request = request.header(header::CONTENT_TYPE, "application/json");
                serde_json::to_vec(body).unwrap_or_default()
            }
            None => Vec::new(),
        };
        let mut request = request
            .body(Body::from(bytes))
            .map_err(|e| (400, e.to_string()))?;
        let label = self
            .allowlist
            .lock()
            .expect("allowlist")
            .get(device)
            .map(|d| d.label.clone())
            .unwrap_or_default();
        mark_remote(&mut request, device, &label);
        Ok(self
            .router
            .clone()
            .oneshot(request)
            .await
            .unwrap_or_else(|never| match never {}))
    }

    /// The interim remote policy: approval answers bound to their question,
    /// no full-access mode changes, a limit on new sessions.
    fn admit(
        &self,
        device: &str,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<(), (u16, String)> {
        match approval_session(path) {
            Some(session) => {
                let answer = body.cloned().unwrap_or(Value::Null);
                self.approvals
                    .take_matching(session, &answer)
                    .map_err(|reason| (409, reason))?;
            }
            // Never let an approval through in a form not checked above.
            None if looks_like_approval(path) => {
                return Err((409, "this path is not served remotely".into()));
            }
            None => {}
        }
        if body.is_some_and(asks_for_full_access) {
            return Err((403, "full access cannot be turned on remotely".into()));
        }
        if method.eq_ignore_ascii_case("POST") && path.split('?').next() == Some("/api/sessions") {
            let mut created = self.created.lock().expect("created");
            let recent = created.entry(device.to_string()).or_default();
            let now = Instant::now();
            while recent
                .front()
                .is_some_and(|t| now.duration_since(*t) > Duration::from_secs(60))
            {
                recent.pop_front();
            }
            if recent.len() >= SESSIONS_PER_MINUTE {
                return Err((429, "too many new sessions; wait a minute".into()));
            }
            recent.push_back(now);
        }
        Ok(())
    }

    /// Notes the sessions a device starts work in, to cancel on revocation.
    fn note_session(&self, device: &str, method: &str, path: &str, status: u16, body: &Value) {
        if status != 200 || !method.eq_ignore_ascii_case("POST") {
            return;
        }
        let path = path.split('?').next().unwrap_or(path);
        let session = if path == "/api/sessions" {
            body["id"].as_str().map(str::to_string)
        } else {
            path.strip_prefix("/api/sessions/")
                .and_then(|rest| rest.strip_suffix("/prompt"))
                .filter(|id| valid_id(id))
                .map(str::to_string)
        };
        if let Some(session) = session {
            self.sessions
                .lock()
                .expect("sessions")
                .entry(device.to_string())
                .or_default()
                .insert(session);
        }
    }

    async fn request(&self, device: &str, id: u64, method: &str, path: &str, body: Option<Value>) {
        if let Err((status, reason)) = self.admit(device, method, path, body.as_ref()) {
            let _ = self
                .send(
                    device,
                    Frame::Res {
                        id,
                        status,
                        body: json!({"error": reason}),
                    },
                )
                .await;
            return;
        }
        let response = match self.call(device, method, path, body.as_ref()).await {
            Ok(response) => response,
            Err((status, message)) => {
                let _ = self
                    .send(
                        device,
                        Frame::Res {
                            id,
                            status,
                            body: json!({"error": message}),
                        },
                    )
                    .await;
                return;
            }
        };
        let status = response.status().as_u16();
        let mime = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let is_json = mime.starts_with("application/json");
        let is_text = mime.starts_with("text/plain") || mime.is_empty();
        // Artifacts are always bytes, whatever their type.
        let artifact = path
            .split('?')
            .next()
            .and_then(|p| p.strip_prefix("/api/artifacts/"))
            .map(str::to_string);
        let as_blob = artifact.is_some() || !(is_json || is_text);
        if as_blob && status == 200 {
            self.stream_blob(device, id, artifact, &mime, response)
                .await;
            return;
        }
        let bytes = match axum::body::to_bytes(response.into_body(), MAX_PLAINTEXT).await {
            Ok(bytes) => bytes,
            Err(_) => {
                let _ = self
                    .send(
                        device,
                        Frame::Res {
                            id,
                            status: 502,
                            body: json!({"error": "the response is too large"}),
                        },
                    )
                    .await;
                return;
            }
        };
        let body = if is_json {
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into()))
        } else {
            Value::String(String::from_utf8_lossy(&bytes).into())
        };
        self.note_session(device, method, path, status, &body);
        let frame = Frame::Res { id, status, body };
        let frame =
            if serde_json::to_vec(&frame).map(|v| v.len()).unwrap_or(0) > MAX_PLAINTEXT - 1024 {
                Frame::Res {
                    id,
                    status: 502,
                    body: json!({"error": "the response is too large for one envelope"}),
                }
            } else {
                frame
            };
        let _ = self.send(device, frame).await;
    }

    /// Sends an artifact as it is read: a `res` describing it, then
    /// 256 KiB `blob` chunks. The size comes from the response's length;
    /// the hash is the artifact's id (content-addressed), which the device
    /// checks against what it assembles.
    async fn stream_blob(
        &self,
        device: &str,
        id: u64,
        artifact: Option<String>,
        mime: &str,
        response: axum::response::Response,
    ) {
        let fail = |message: &str| Frame::Res {
            id,
            status: 502,
            body: json!({"error": message}),
        };
        let size = response
            .headers()
            .get(header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<usize>().ok());
        let (Some(sha256), Some(size)) = (artifact, size) else {
            let _ = self
                .send(device, fail("only artifacts of known size are served"))
                .await;
            return;
        };
        if size > MAX_BLOB {
            let _ = self.send(device, fail("the artifact is too large")).await;
            return;
        }
        let of = size.div_ceil(BLOB_CHUNK).max(1) as u32;
        let described = Frame::Res {
            id,
            status: 200,
            body: json!({"blob": {"sha256": sha256, "mime": mime, "size": size, "chunks": of}}),
        };
        if self.send(device, described).await.is_err() {
            return;
        }
        let mut body = response.into_body().into_data_stream();
        let mut pending: Vec<u8> = Vec::with_capacity(BLOB_CHUNK);
        let mut hasher = Sha256::new();
        let mut chunk = 0u32;
        let mut total = 0usize;
        loop {
            let next = body.next().await;
            let finished = next.is_none();
            if let Some(data) = next {
                let Ok(data) = data else { return };
                total += data.len();
                if total > size {
                    return;
                }
                hasher.update(&data);
                pending.extend_from_slice(&data);
            }
            while pending.len() >= BLOB_CHUNK || (finished && (chunk < of)) {
                let take = pending.len().min(BLOB_CHUNK);
                let data: Vec<u8> = pending.drain(..take).collect();
                let frame = Frame::Blob {
                    id,
                    sha256: sha256.clone(),
                    chunk,
                    of,
                    data: base64::engine::general_purpose::STANDARD.encode(&data),
                };
                if self.send(device, frame).await.is_err() {
                    return;
                }
                chunk += 1;
                if finished && pending.is_empty() {
                    break;
                }
            }
            if finished {
                break;
            }
        }
        if hex(&hasher.finalize()) != sha256 {
            (self.notice)(Notice::Relay(format!(
                "artifact {sha256} did not match its id"
            )));
        }
    }

    /// Streams the server-sent events at `path` to the device as `evt`
    /// frames, skipping records up to `after`.
    async fn subscribe(&self, device: &str, id: u64, path: &str, after: i64) -> Result<(), String> {
        let response = self
            .call(device, "GET", path, None)
            .await
            .map_err(|(status, message)| format!("{status}: {message}"))?;
        let status = response.status();
        let is_stream = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("text/event-stream"));
        if !status.is_success() || !is_stream {
            let text = axum::body::to_bytes(response.into_body(), 4096)
                .await
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .unwrap_or_default();
            return Err(format!("{}: {text}", status.as_u16()));
        }
        let session = events_session(path).map(str::to_string);
        let mut body = response.into_body().into_data_stream();
        let mut buffer = String::new();
        let mut last_seq: i64 = -1;
        while let Some(chunk) = body.next().await {
            let chunk = chunk.map_err(|e| e.to_string())?;
            buffer.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(end) = buffer.find("\n\n") {
                let event: String = buffer.drain(..end + 2).collect();
                let data: Vec<&str> = event
                    .lines()
                    .filter_map(|line| line.strip_prefix("data:"))
                    .map(|d| d.strip_prefix(' ').unwrap_or(d))
                    .collect();
                if data.is_empty() {
                    continue;
                }
                let data = data.join("\n");
                let mut data: Value = serde_json::from_str(&data).unwrap_or(Value::String(data));
                if let Some(seq) = data["record"]["seq"].as_i64() {
                    last_seq = last_seq.max(seq);
                    if seq <= after {
                        continue;
                    }
                }
                if let Some(session) = &session {
                    self.approvals.observe(session, &mut data);
                }
                self.send(
                    device,
                    Frame::Evt {
                        id,
                        seq: last_seq,
                        data,
                    },
                )
                .await
                .map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }

    /// Seals a frame to an enrolled device and queues it.
    async fn send(&self, device: &str, frame: Frame) -> Result<(), SyncError> {
        let x = {
            let allowlist = self.allowlist.lock().expect("allowlist");
            let entry = allowlist
                .get(device)
                .ok_or_else(|| SyncError::UnknownSender(device.into()))?;
            unb64_32(&entry.x)?
        };
        self.send_to(device, &x, frame).await
    }

    /// Seals and queues a frame with the next host counter. Counters are
    /// reserved on disk in blocks before use, so a restarted host (even
    /// after a crash) never sends a counter twice.
    async fn send_to(&self, device: &str, x: &[u8; 32], frame: Frame) -> Result<(), SyncError> {
        let plain = serde_json::to_vec(&frame).expect("a frame serialises");
        let mut sender = self.sender.lock().await;
        let counter = sender.counter.advance();
        if counter > sender.reserved {
            let reserve = counter + COUNTER_RESERVE;
            self.update(|list| {
                list.host_counter = list.host_counter.max(reserve);
                Ok(())
            })?;
            sender.reserved = reserve;
        }
        let header = Header::new(&self.device_id, device, &frame.stream(), counter);
        let envelope = seal(&self.identity, x, &header, &plain)?;
        self.relay.send(device, envelope, false).await
    }
}

/// The fingerprint of a stored device, for listings.
pub fn device_fingerprint(device: &AllowedDevice) -> String {
    device
        .keys()
        .map(|k| k.display_fingerprint())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_api_paths_pass() {
        assert!(allowed_path("/api/sessions"));
        assert!(allowed_path("/api/artifacts/ab?session=x"));
        for bad in [
            "/",
            "/api",
            "/apix/sessions",
            "/api/../etc",
            "/api//x",
            "/api/a b",
            "/api/x#y",
            "/api/x\\y",
        ] {
            assert!(!allowed_path(bad), "{bad}");
        }
    }

    #[test]
    fn full_access_requests_are_recognised() {
        assert!(asks_for_full_access(
            &json!({"sandbox": "danger-full-access"})
        ));
        assert!(asks_for_full_access(&json!({"mode": ["x", "Full_Access"]})));
        assert!(!asks_for_full_access(
            &json!({"text": "please give full access to the docs"})
        ));
    }

    #[tokio::test]
    async fn requests_carry_the_remote_caller() {
        let router = axum::Router::new().route(
            "/api/who",
            axum::routing::get(|caller: Option<axum::Extension<RemoteCaller>>| async move {
                caller
                    .map(|c| format!("{}/{}", c.device_id, c.label))
                    .unwrap_or_default()
            }),
        );
        let mut request = Request::builder()
            .uri("/api/who")
            .body(Body::empty())
            .unwrap();
        mark_remote(&mut request, "phone", "My phone");
        let response = router.oneshot(request).await.unwrap();
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(&body[..], b"phone/My phone");
    }

    #[test]
    fn labels_are_cleaned() {
        assert_eq!(clean_label("  Phone\u{7}  "), "Phone");
        assert_eq!(clean_label("Pho\u{202E}ne\u{2066}"), "Phone");
        assert_eq!(clean_label(""), "device");
        assert_eq!(clean_label(&"x".repeat(100)).len(), MAX_LABEL);
    }
}
