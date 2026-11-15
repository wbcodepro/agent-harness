//! A device side in Rust: pairs with a host and calls its API through the
//! relay. The TypeScript package (`packages/sync-js`) is the one apps use;
//! this one serves tests and command-line tools, and follows the same
//! protocol.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::{mpsc, oneshot};

use crate::SyncError;
use crate::envelope::{Counter, Header, Sealed, seal};
use crate::frames::Frame;
use crate::keys::{Identity, PublicIdentity, b64, hex};
use crate::pairing::{Grant, PairingOffer, SignedGrant, pairing_proof, sas};
use crate::relay::{RelayEvent, RelayHandle};

/// An API answer.
#[derive(Clone, Debug, PartialEq)]
pub struct Response {
    pub status: u16,
    pub body: Value,
}

/// An artifact fetched from the host, its hash checked.
#[derive(Clone, Debug, PartialEq)]
pub struct Blob {
    pub mime: String,
    pub bytes: Vec<u8>,
}

struct Inner {
    identity: Identity,
    device_id: String,
    host_id: String,
    host: Mutex<Option<PublicIdentity>>,
    relay: RelayHandle,
    counter: tokio::sync::Mutex<Counter>,
    last_host_counter: Mutex<Option<u64>>,
    next_id: AtomicU64,
    waiters: Mutex<HashMap<u64, mpsc::UnboundedSender<Frame>>>,
    pairing: Mutex<Option<oneshot::Sender<Frame>>>,
    rejected: Mutex<Vec<String>>,
}

#[derive(Clone)]
pub struct RemoteClient {
    inner: Arc<Inner>,
}

impl RemoteClient {
    /// A client for `host_id`, whose keys are pinned already (`host`) or
    /// come from a pairing offer later.
    pub fn new(
        identity: Identity,
        device_id: &str,
        host_id: &str,
        host: Option<PublicIdentity>,
        relay: RelayHandle,
        events: mpsc::Receiver<RelayEvent>,
    ) -> Self {
        let client = Self {
            inner: Arc::new(Inner {
                identity,
                device_id: device_id.into(),
                host_id: host_id.into(),
                host: Mutex::new(host),
                relay,
                counter: tokio::sync::Mutex::new(Counter::default()),
                last_host_counter: Mutex::new(None),
                next_id: AtomicU64::new(1),
                waiters: Mutex::new(HashMap::new()),
                pairing: Mutex::new(None),
                rejected: Mutex::new(Vec::new()),
            }),
        };
        tokio::spawn(Arc::clone(&client.inner).receive(events));
        client
    }

    pub fn public(&self) -> PublicIdentity {
        self.inner.identity.public()
    }

    /// Closes the relay connection for good.
    pub fn close(&self) {
        self.inner.relay.stop();
    }

    /// Why envelopes from the relay were dropped, oldest first.
    pub fn rejected(&self) -> Vec<String> {
        self.inner.rejected.lock().expect("rejected").clone()
    }

    /// Sends a pairing request answering `offer` and returns the six
    /// digits to compare with the host's screen. Await
    /// [`RemoteClient::paired`] for the host's answer.
    pub async fn start_pairing(
        &self,
        offer: &PairingOffer,
        label: &str,
    ) -> Result<(String, oneshot::Receiver<Frame>), SyncError> {
        if offer.expired(crate::pairing::now_unix()) {
            return Err(SyncError::Pairing("the pairing code has expired".into()));
        }
        if offer.host != self.inner.host_id {
            return Err(SyncError::Pairing("the code is for another host".into()));
        }
        *self.inner.host.lock().expect("host") = Some(offer.host_keys);
        let me = self.public();
        let (answer, answered) = oneshot::channel();
        *self.inner.pairing.lock().expect("pairing") = Some(answer);
        let frame = Frame::Pair {
            ed: b64(&me.ed),
            x: b64(&me.x),
            label: label.into(),
            mac: b64(&pairing_proof(&offer.secret, &me)),
        };
        self.send(frame).await?;
        Ok((sas(&offer.secret, &offer.host_keys.ed, &me.ed), answered))
    }

    /// The host's answer to a pairing request: its verified grant.
    pub async fn paired(&self, answered: oneshot::Receiver<Frame>) -> Result<Grant, SyncError> {
        let frame = tokio::time::timeout(Duration::from_secs(600), answered)
            .await
            .map_err(|_| SyncError::Pairing("no answer from the host".into()))?
            .map_err(|_| SyncError::Closed)?;
        match frame {
            Frame::Paired { grant, sig } => {
                let host = self.host()?;
                let grant = SignedGrant { grant, sig }.verify(&host.ed)?;
                if grant.device != self.inner.device_id
                    || grant.host != self.inner.host_id
                    || grant.ed != b64(&self.public().ed)
                {
                    return Err(SyncError::Pairing("the grant names someone else".into()));
                }
                Ok(grant)
            }
            Frame::PairRefused { reason } => Err(SyncError::Pairing(reason)),
            _ => Err(SyncError::Pairing("unexpected answer".into())),
        }
    }

    fn host(&self) -> Result<PublicIdentity, SyncError> {
        self.inner
            .host
            .lock()
            .expect("host")
            .ok_or_else(|| SyncError::Pairing("no host key pinned".into()))
    }

    fn open_stream(&self) -> (u64, mpsc::UnboundedReceiver<Frame>) {
        let id = self.inner.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::unbounded_channel();
        self.inner.waiters.lock().expect("waiters").insert(id, tx);
        (id, rx)
    }

    fn close_stream(&self, id: u64) {
        self.inner.waiters.lock().expect("waiters").remove(&id);
    }

    /// Seals a frame to the host, as the relay would carry it (tests use
    /// it to tamper with or replay an envelope).
    pub async fn seal(&self, frame: &Frame) -> Result<String, SyncError> {
        let mut counter = self.inner.counter.lock().await;
        self.seal_with(&mut counter, frame)
    }

    fn seal_with(&self, counter: &mut Counter, frame: &Frame) -> Result<String, SyncError> {
        let host = self.host()?;
        let plain = serde_json::to_vec(frame).expect("a frame serialises");
        let header = Header::new(
            &self.inner.device_id,
            &self.inner.host_id,
            &frame.stream(),
            counter.advance(),
        );
        seal(&self.inner.identity, &host.x, &header, &plain)
    }

    /// Sends an envelope as is (tests use it to replay or tamper).
    pub async fn send_raw(&self, envelope: String) -> Result<(), SyncError> {
        self.inner
            .relay
            .send(&self.inner.host_id, envelope, false)
            .await
    }

    /// Seals and queues a frame. The counter stays locked until the frame
    /// is queued, so frames reach the host in counter order.
    pub async fn send(&self, frame: Frame) -> Result<(), SyncError> {
        let mut counter = self.inner.counter.lock().await;
        let envelope = self.seal_with(&mut counter, &frame)?;
        self.send_raw(envelope).await
    }

    async fn first_res(
        &self,
        id: u64,
        rx: &mut mpsc::UnboundedReceiver<Frame>,
        wait: Duration,
    ) -> Result<Response, SyncError> {
        loop {
            match tokio::time::timeout(wait, rx.recv()).await {
                Err(_) => {
                    self.close_stream(id);
                    return Err(SyncError::Relay("no answer from the host".into()));
                }
                Ok(None) => return Err(SyncError::Closed),
                Ok(Some(Frame::Res { status, body, .. })) => return Ok(Response { status, body }),
                Ok(Some(_)) => continue,
            }
        }
    }

    /// Calls the host's API.
    pub async fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<Response, SyncError> {
        self.request_within(method, path, body, Duration::from_secs(30))
            .await
    }

    pub async fn request_within(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        wait: Duration,
    ) -> Result<Response, SyncError> {
        let (id, mut rx) = self.open_stream();
        let frame = Frame::Req {
            id,
            method: method.into(),
            path: path.into(),
            body,
        };
        if let Err(e) = self.send(frame).await {
            self.close_stream(id);
            return Err(e);
        }
        let response = self.first_res(id, &mut rx, wait).await;
        self.close_stream(id);
        response
    }

    /// Sends a control frame answered with a `res` (`devices`, `revoke`).
    pub async fn control(&self, make: impl FnOnce(u64) -> Frame) -> Result<Response, SyncError> {
        let (id, mut rx) = self.open_stream();
        self.send(make(id)).await?;
        let response = self.first_res(id, &mut rx, Duration::from_secs(30)).await;
        self.close_stream(id);
        response
    }

    /// Subscribes to an event stream. Records up to `after` are skipped
    /// (-1 for everything). The receiver gets `evt` frames and finally an
    /// `end`.
    pub async fn subscribe(
        &self,
        path: &str,
        after: i64,
    ) -> Result<(u64, mpsc::UnboundedReceiver<Frame>), SyncError> {
        let (id, rx) = self.open_stream();
        self.send(Frame::Sub {
            id,
            path: path.into(),
            after,
        })
        .await?;
        Ok((id, rx))
    }

    pub async fn cancel(&self, id: u64) -> Result<(), SyncError> {
        self.send(Frame::Cancel { id }).await
    }

    /// Fetches an artifact of `session` and checks its hash.
    pub async fn fetch_blob(&self, sha256: &str, session: &str) -> Result<Blob, SyncError> {
        let (id, mut rx) = self.open_stream();
        self.send(Frame::Req {
            id,
            method: "GET".into(),
            path: format!("/api/artifacts/{sha256}?session={session}"),
            body: None,
        })
        .await?;
        let wait = Duration::from_secs(60);
        let described = self.first_res(id, &mut rx, wait).await?;
        let meta = &described.body["blob"];
        if described.status != 200 || !meta.is_object() {
            self.close_stream(id);
            return Err(SyncError::Relay(format!(
                "the host answered {}: {}",
                described.status, described.body
            )));
        }
        let of = meta["chunks"].as_u64().unwrap_or(0) as usize;
        let mut chunks: Vec<Option<Vec<u8>>> = vec![None; of];
        let mut have = 0;
        while have < of {
            let frame = tokio::time::timeout(wait, rx.recv())
                .await
                .map_err(|_| SyncError::Relay("the artifact stopped arriving".into()))?
                .ok_or(SyncError::Closed)?;
            if let Frame::Blob {
                chunk,
                data,
                sha256: named,
                ..
            } = frame
            {
                if named != sha256 || chunk as usize >= of {
                    continue;
                }
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .map_err(|_| SyncError::Malformed("bad blob chunk"))?;
                if chunks[chunk as usize].replace(bytes).is_none() {
                    have += 1;
                }
            }
        }
        self.close_stream(id);
        let bytes: Vec<u8> = chunks.into_iter().flatten().flatten().collect();
        if hex(&Sha256::digest(&bytes)) != sha256 {
            return Err(SyncError::Crypto("the artifact does not match its hash"));
        }
        Ok(Blob {
            mime: meta["mime"].as_str().unwrap_or("").into(),
            bytes,
        })
    }
}

impl Inner {
    async fn receive(self: Arc<Self>, mut events: mpsc::Receiver<RelayEvent>) {
        while let Some(event) = events.recv().await {
            let RelayEvent::Deliver {
                seq,
                from,
                envelope,
            } = event
            else {
                continue;
            };
            if let Err(error) = self.accept(&from, &envelope) {
                self.rejected
                    .lock()
                    .expect("rejected")
                    .push(error.to_string());
            }
            let _ = self.relay.ack(seq).await;
        }
        self.waiters.lock().expect("waiters").clear();
    }

    fn accept(&self, from: &str, envelope: &str) -> Result<(), SyncError> {
        let sealed = Sealed::parse(envelope)?;
        if sealed.header.from != self.host_id || from != self.host_id {
            return Err(SyncError::UnknownSender(sealed.header.from.clone()));
        }
        let host = self
            .host
            .lock()
            .expect("host")
            .ok_or_else(|| SyncError::UnknownSender(from.into()))?;
        let last = *self.last_host_counter.lock().expect("counter");
        let plain = sealed.open(&self.identity, &self.device_id, &host, last)?;
        *self.last_host_counter.lock().expect("counter") = Some(sealed.header.counter);
        let frame: Frame =
            serde_json::from_slice(&plain).map_err(|_| SyncError::Malformed("bad frame"))?;
        if frame.stream() != sealed.header.stream {
            return Err(SyncError::Malformed(
                "frame and header name different streams",
            ));
        }
        match frame {
            Frame::Paired { .. } | Frame::PairRefused { .. } => {
                if let Some(waiter) = self.pairing.lock().expect("pairing").take() {
                    let _ = waiter.send(frame);
                }
            }
            Frame::Res { id, .. }
            | Frame::Evt { id, .. }
            | Frame::End { id, .. }
            | Frame::Blob { id, .. } => {
                let mut waiters = self.waiters.lock().expect("waiters");
                let ended = matches!(frame, Frame::End { .. });
                if let Some(waiter) = waiters.get(&id)
                    && waiter.send(frame).is_err()
                {
                    waiters.remove(&id);
                }
                if ended {
                    waiters.remove(&id);
                }
            }
            _ => {}
        }
        Ok(())
    }
}
