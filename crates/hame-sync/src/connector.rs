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

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

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
use crate::keys::{Identity, PublicIdentity, hex, unb64, unb64_32};
use crate::pairing::{
    Grant, GrantEntry, GrantList, PairingOffer, SignedGrantList, check_pairing_proof, now_unix, sas,
};
use crate::relay::{RelayEvent, RelayHandle};
use crate::store::{AllowedDevice, Allowlist, HostStore};

/// The largest artifact sent to a device.
pub const MAX_BLOB: usize = 64 * 1024 * 1024;
const MAX_LABEL: usize = 64;
const MAX_PATH: usize = 2048;

/// What the host user is asked before a device is enrolled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairingPrompt {
    pub device: String,
    pub label: String,
    pub fingerprint: String,
    /// The six digits the device shows too.
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
}

struct Inner {
    identity: Identity,
    device_id: String,
    store: HostStore,
    allowlist: Mutex<Allowlist>,
    router: axum::Router,
    token: String,
    confirm: ConfirmFn,
    notice: NoticeFn,
    grants_changed: Option<GrantsFn>,
    offer: Mutex<Option<PairingOffer>>,
    approvals: PendingApprovals,
    tasks: Mutex<HashMap<(String, u64), AbortHandle>>,
    /// Held while a frame is sealed and queued, so envelopes reach each
    /// device in counter order.
    sender: tokio::sync::Mutex<Counter>,
    host_counter: AtomicU64,
    relay: RelayHandle,
    /// The allowlist file as this connector last read or wrote it.
    stamp: Mutex<Option<(std::time::SystemTime, u64)>>,
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
                router: options.router,
                token: options.token,
                confirm: options.confirm,
                notice: options.notice,
                grants_changed: options.grants_changed,
                offer: Mutex::new(None),
                approvals: PendingApprovals::default(),
                tasks: Mutex::new(HashMap::new()),
                sender: tokio::sync::Mutex::new(Counter::starting_after(host_counter)),
                host_counter: AtomicU64::new(host_counter),
                relay,
                stamp: Mutex::new(stamp),
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

    /// Revokes a device: its key is refused from now on and its streams
    /// stop.
    pub fn revoke(&self, device: &str) -> Result<bool, SyncError> {
        self.inner.revoke(device, "host")
    }

    /// Handles relay events until the relay connection ends.
    pub async fn run(&self, mut events: mpsc::Receiver<RelayEvent>) {
        // A revocation made by another process stops streams within a
        // couple of seconds even if the device sends nothing.
        let mut check = tokio::time::interval(std::time::Duration::from_secs(2));
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
        for (_, task) in self.inner.tasks.lock().expect("tasks").drain() {
            task.abort();
        }
    }
}

fn clean_label(label: &str) -> String {
    let label: String = label
        .chars()
        .filter(|c| !c.is_control())
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
        self.reload_if_changed()?;
        let known = {
            let allowlist = self.allowlist.lock().expect("allowlist");
            allowlist
                .get(&from)
                .map(|device| (device.keys(), device.last_counter))
        };
        let Some((keys, last)) = known else {
            return self.pair(sealed).await;
        };
        let keys = keys?;
        let plain = match sealed.open(&self.identity, &self.device_id, &keys, Some(last)) {
            Ok(plain) => plain,
            Err(SyncError::BadSignature) => {
                self.check_changed_key(&sealed, &keys).await;
                return Err(SyncError::BadSignature);
            }
            Err(error) => return Err(error),
        };
        {
            let mut allowlist = self.allowlist.lock().expect("allowlist");
            if let Some(device) = allowlist.get_mut(&from) {
                device.last_counter = sealed.header.counter;
            }
            allowlist.host_counter = self.host_counter.load(Ordering::SeqCst);
            self.save_allowlist(&allowlist)?;
        }
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

    fn save_allowlist(&self, allowlist: &Allowlist) -> Result<(), SyncError> {
        self.store.save_allowlist(allowlist)?;
        *self.stamp.lock().expect("stamp") = self.store.allowlist_stamp();
        Ok(())
    }

    /// Takes in a change another process made to the allowlist file (a
    /// revocation from `hame connect revoke`): removed devices are refused
    /// from now on and their streams stop.
    fn reload_if_changed(&self) -> Result<(), SyncError> {
        let now = self.store.allowlist_stamp();
        if now == *self.stamp.lock().expect("stamp") {
            return Ok(());
        }
        let mut fresh = self.store.allowlist()?;
        let gone: Vec<String> = {
            let mut allowlist = self.allowlist.lock().expect("allowlist");
            for device in &mut fresh.devices {
                if let Some(known) = allowlist.get(&device.device) {
                    device.last_counter = device.last_counter.max(known.last_counter);
                }
            }
            fresh.host_counter = fresh.host_counter.max(allowlist.host_counter);
            let gone = allowlist
                .devices
                .iter()
                .filter(|d| fresh.get(&d.device).is_none())
                .map(|d| d.device.clone())
                .collect();
            *allowlist = fresh;
            gone
        };
        *self.stamp.lock().expect("stamp") = now;
        if !gone.is_empty() {
            self.tasks
                .lock()
                .expect("tasks")
                .retain(|(owner, _), task| {
                    let keep = !gone.contains(owner);
                    if !keep {
                        task.abort();
                    }
                    keep
                });
            for device in gone {
                (self.notice)(Notice::Revoked {
                    device,
                    by: "host".into(),
                });
            }
        }
        Ok(())
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

    /// An envelope from a device not on the allowlist: only a pairing
    /// request answering the open offer is considered.
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
        let prompt = PairingPrompt {
            device: from.clone(),
            label: label.clone(),
            fingerprint: keys.display_fingerprint(),
            sas: sas(&offer.secret, &self.identity.public().ed, &keys.ed),
        };
        let counter = sealed.header.counter;
        let this = Arc::clone(&self);
        tokio::spawn(async move {
            let confirmed = (this.confirm)(prompt).await;
            let outcome = if confirmed {
                let mut allowlist = this.allowlist.lock().expect("allowlist");
                allowlist
                    .add(&from, &label, &keys, counter)
                    .and_then(|()| this.save_allowlist(&allowlist))
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
                label: d.label.clone(),
            })
            .collect();
        hook(GrantList::new(&self.device_id, devices).sign(&self.identity));
    }

    fn revoke(&self, device: &str, by: &str) -> Result<bool, SyncError> {
        let removed = {
            let mut allowlist = self.allowlist.lock().expect("allowlist");
            let removed = allowlist.revoke(device);
            if removed {
                self.save_allowlist(&allowlist)?;
            }
            removed
        };
        if removed {
            self.tasks
                .lock()
                .expect("tasks")
                .retain(|(owner, _), task| {
                    let keep = owner != device;
                    if !keep {
                        task.abort();
                    }
                    keep
                });
            (self.notice)(Notice::Revoked {
                device: device.into(),
                by: by.into(),
            });
            self.publish_grants();
        }
        Ok(removed)
    }

    fn dispatch(self: Arc<Self>, device: String, frame: Frame) {
        match frame {
            Frame::Req {
                id,
                method,
                path,
                body,
            } => {
                let this = Arc::clone(&self);
                let owner = device.clone();
                self.track(&device, id, async move {
                    this.request(&owner, id, &method, &path, body).await;
                });
            }
            Frame::Sub { id, path, after } => {
                let this = Arc::clone(&self);
                let owner = device.clone();
                self.track(&device, id, async move {
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
                if let Some(task) = task {
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
                tokio::spawn(async move {
                    let _ = self.send(&device, Frame::Res { id, status, body }).await;
                });
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
                tokio::spawn(async move {
                    let _ = self
                        .send(
                            &device,
                            Frame::Res {
                                id,
                                status: 200,
                                body: json!({"devices": devices}),
                            },
                        )
                        .await;
                });
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

    /// Runs `work` as the task of stream `id` of `device`, so a `cancel`
    /// or a revocation can stop it.
    fn track(
        self: &Arc<Self>,
        device: &str,
        id: u64,
        work: impl std::future::Future<Output = ()> + Send + 'static,
    ) {
        let key = (device.to_string(), id);
        let this = Arc::clone(self);
        let done = key.clone();
        let mut tasks = self.tasks.lock().expect("tasks");
        if let Some(previous) = tasks.remove(&key) {
            previous.abort();
        }
        let task = tokio::spawn(async move {
            work.await;
            this.tasks.lock().expect("tasks").remove(&done);
        });
        tasks.insert(key, task.abort_handle());
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

    async fn request(&self, device: &str, id: u64, method: &str, path: &str, body: Option<Value>) {
        let binding = match approval_session(path) {
            Some(session) => {
                let answer = body.clone().unwrap_or(Value::Null);
                self.approvals.take_matching(session, &answer)
            }
            // Never let an approval through in a form not checked above.
            None if looks_like_approval(path) => {
                Err("this path is not served remotely".to_string())
            }
            None => Ok(()),
        };
        {
            if let Err(reason) = binding {
                let _ = self
                    .send(
                        device,
                        Frame::Res {
                            id,
                            status: 409,
                            body: json!({"error": reason}),
                        },
                    )
                    .await;
                return;
            }
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
        let as_blob = path.starts_with("/api/artifacts/") || !(is_json || is_text);
        let limit = if as_blob { MAX_BLOB } else { MAX_PLAINTEXT };
        let bytes = match axum::body::to_bytes(response.into_body(), limit).await {
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
        if !as_blob || status != 200 {
            let body = if is_json {
                serde_json::from_slice(&bytes)
                    .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into()))
            } else {
                Value::String(String::from_utf8_lossy(&bytes).into())
            };
            let frame = Frame::Res { id, status, body };
            let frame = if serde_json::to_vec(&frame).map(|v| v.len()).unwrap_or(0)
                > MAX_PLAINTEXT - 1024
            {
                Frame::Res {
                    id,
                    status: 502,
                    body: json!({"error": "the response is too large for one envelope"}),
                }
            } else {
                frame
            };
            let _ = self.send(device, frame).await;
            return;
        }
        // An artifact: describe it, then send it in chunks.
        let sha256 = hex(&Sha256::digest(&bytes));
        let chunks: Vec<&[u8]> = if bytes.is_empty() {
            vec![&[][..]]
        } else {
            bytes.chunks(BLOB_CHUNK).collect()
        };
        let of = chunks.len() as u32;
        let described = Frame::Res {
            id,
            status,
            body: json!({"blob": {"sha256": sha256, "mime": mime, "size": bytes.len(), "chunks": of}}),
        };
        if self.send(device, described).await.is_err() {
            return;
        }
        for (chunk, data) in chunks.into_iter().enumerate() {
            let frame = Frame::Blob {
                id,
                sha256: sha256.clone(),
                chunk: chunk as u32,
                of,
                data: base64::engine::general_purpose::STANDARD.encode(data),
            };
            if self.send(device, frame).await.is_err() {
                return;
            }
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
                let data: Value = serde_json::from_str(&data).unwrap_or(Value::String(data));
                if let Some(seq) = data["record"]["seq"].as_i64() {
                    last_seq = last_seq.max(seq);
                    if seq <= after {
                        continue;
                    }
                }
                if let Some(session) = &session {
                    self.approvals.observe(session, &data);
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

    async fn send_to(&self, device: &str, x: &[u8; 32], frame: Frame) -> Result<(), SyncError> {
        let plain = serde_json::to_vec(&frame).expect("a frame serialises");
        let mut counter = self.sender.lock().await;
        let header = Header::new(&self.device_id, device, &frame.stream(), counter.advance());
        self.host_counter.store(counter.last(), Ordering::SeqCst);
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
        assert_eq!(clean_label(""), "device");
        assert_eq!(clean_label(&"x".repeat(100)).len(), MAX_LABEL);
    }
}
