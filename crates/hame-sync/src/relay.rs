//! A connection to the relay's WebSocket (`/v1/ws`, §3) that stays up:
//! it sends `resume` with the last acknowledged `seq` as the first frame,
//! pings every 25 seconds, reconnects with jittered exponential backoff when
//! the socket fails or goes quiet. A `send` stays unconfirmed until the
//! relay answers it with `sent` (or an error); unconfirmed sends are written
//! again after a reconnection, since a socket can die with them in flight.
//! A duplicate that reaches the recipient is refused there as a replay.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use rand::Rng;
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;

use crate::SyncError;
use crate::frames::RelayFrame;

const MAX_UNCONFIRMED: usize = 1024;

/// Gives the bearer token to connect with; called before every connection
/// attempt, so it can refresh an expired one.
pub type TokenSource = Arc<
    dyn Fn()
            -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, SyncError>> + Send>>
        + Send
        + Sync,
>;

/// A token that never changes.
pub fn fixed_token(token: impl Into<String>) -> TokenSource {
    let token = token.into();
    Arc::new(move || {
        let token = token.clone();
        Box::pin(async move { Ok(token) })
    })
}

#[derive(Clone)]
pub struct RelayOptions {
    /// The relay's base address, `http(s)://host[:port][/prefix]`.
    pub url: String,
    pub device_id: String,
    pub token: TokenSource,
    pub ping_every: Duration,
    /// Reconnect when nothing arrived for this long.
    pub silence_limit: Duration,
    pub backoff_base: Duration,
    pub backoff_max: Duration,
}

impl RelayOptions {
    pub fn new(url: &str, device_id: &str, token: TokenSource) -> Self {
        Self {
            url: url.trim_end_matches('/').into(),
            device_id: device_id.into(),
            token,
            ping_every: Duration::from_secs(25),
            silence_limit: Duration::from_secs(60),
            backoff_base: Duration::from_millis(500),
            backoff_max: Duration::from_secs(30),
        }
    }

    /// `ws(s)://…/v1/ws?device=<id>`.
    pub fn ws_url(&self) -> Result<String, SyncError> {
        let mut url = url::Url::parse(&self.url).map_err(|e| SyncError::Relay(e.to_string()))?;
        let scheme = match url.scheme() {
            "http" | "ws" => "ws",
            "https" | "wss" => "wss",
            other => return Err(SyncError::Relay(format!("unsupported scheme {other}"))),
        };
        url.set_scheme(scheme)
            .map_err(|_| SyncError::Relay("cannot set scheme".into()))?;
        let path = format!("{}/v1/ws", url.path().trim_end_matches('/'));
        url.set_path(&path);
        url.query_pairs_mut()
            .clear()
            .append_pair("device", &self.device_id);
        Ok(url.into())
    }
}

/// What the connection reports.
#[derive(Clone, Debug, PartialEq)]
pub enum RelayEvent {
    Connected,
    Disconnected(String),
    /// An envelope for this device. Acknowledge it with
    /// [`RelayHandle::ack`] once handled.
    Deliver {
        seq: u64,
        from: String,
        envelope: String,
    },
    Sent {
        to: String,
        seq: u64,
    },
    Presence {
        device: String,
        online: bool,
    },
    Error {
        code: String,
        message: String,
    },
}

/// Sends through the connection. Cheap to clone.
#[derive(Clone)]
pub struct RelayHandle {
    out: mpsc::Sender<RelayFrame>,
    stop: watch::Sender<bool>,
}

impl RelayHandle {
    pub async fn send(&self, to: &str, envelope: String, push: bool) -> Result<(), SyncError> {
        self.out
            .send(RelayFrame::Send {
                to: to.into(),
                envelope,
                push,
            })
            .await
            .map_err(|_| SyncError::Closed)
    }

    /// Acknowledges everything up to `seq`; the relay may drop it.
    pub async fn ack(&self, seq: u64) -> Result<(), SyncError> {
        self.out
            .send(RelayFrame::Ack { seq })
            .await
            .map_err(|_| SyncError::Closed)
    }

    /// Closes the connection for good.
    pub fn stop(&self) {
        let _ = self.stop.send(true);
    }
}

/// Starts the connection in the background.
pub fn connect(options: RelayOptions) -> (RelayHandle, mpsc::Receiver<RelayEvent>) {
    let (out_tx, out_rx) = mpsc::channel(1024);
    let (event_tx, event_rx) = mpsc::channel(1024);
    let (stop_tx, stop_rx) = watch::channel(false);
    tokio::spawn(run(options, out_rx, event_tx, stop_rx));
    (
        RelayHandle {
            out: out_tx,
            stop: stop_tx,
        },
        event_rx,
    )
}

/// Full-jitter exponential backoff: a random wait between half and all of
/// `min(max, base · 2^attempt)`.
pub fn backoff(base: Duration, max: Duration, attempt: u32) -> Duration {
    let cap = base
        .saturating_mul(1u32.checked_shl(attempt.min(16)).unwrap_or(u32::MAX))
        .min(max);
    let millis = cap.as_millis() as u64;
    Duration::from_millis(rand::thread_rng().gen_range(millis / 2..=millis.max(1)))
}

async fn run(
    options: RelayOptions,
    mut out: mpsc::Receiver<RelayFrame>,
    events: mpsc::Sender<RelayEvent>,
    mut stop: watch::Receiver<bool>,
) {
    let mut last_acked: u64 = 0;
    let mut unconfirmed: VecDeque<RelayFrame> = VecDeque::new();
    let mut attempt: u32 = 0;
    loop {
        if *stop.borrow() {
            return;
        }
        let reason = match session(
            &options,
            &mut out,
            &events,
            &mut stop,
            &mut last_acked,
            &mut unconfirmed,
            &mut attempt,
        )
        .await
        {
            Ok(()) => return,
            Err(reason) => reason,
        };
        if events.send(RelayEvent::Disconnected(reason)).await.is_err() {
            return;
        }
        let wait = backoff(options.backoff_base, options.backoff_max, attempt);
        attempt = attempt.saturating_add(1);
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = stop.changed() => return,
        }
    }
}

/// One connection, until it fails (`Err` with the reason) or is stopped.
async fn session(
    options: &RelayOptions,
    out: &mut mpsc::Receiver<RelayFrame>,
    events: &mpsc::Sender<RelayEvent>,
    stop: &mut watch::Receiver<bool>,
    last_acked: &mut u64,
    unconfirmed: &mut VecDeque<RelayFrame>,
    attempt: &mut u32,
) -> Result<(), String> {
    let token = (options.token)().await.map_err(|e| e.to_string())?;
    let url = options.ws_url().map_err(|e| e.to_string())?;
    let mut request = url.into_client_request().map_err(|e| e.to_string())?;
    request.headers_mut().insert(
        "authorization",
        HeaderValue::from_str(&format!("Bearer {token}")).map_err(|e| e.to_string())?,
    );
    let (socket, _) = tokio::time::timeout(
        Duration::from_secs(20),
        tokio_tungstenite::connect_async(request),
    )
    .await
    .map_err(|_| "connection timed out".to_string())?
    .map_err(|e| e.to_string())?;
    let (mut sink, mut stream) = socket.split();
    let text = |frame: &RelayFrame| Message::text(serde_json::to_string(frame).expect("frame"));
    sink.send(text(&RelayFrame::Resume { after: *last_acked }))
        .await
        .map_err(|e| e.to_string())?;
    for frame in unconfirmed.iter() {
        sink.send(text(frame)).await.map_err(|e| e.to_string())?;
    }
    let _ = events.send(RelayEvent::Connected).await;
    let mut last_heard = Instant::now();
    let mut ping = tokio::time::interval(options.ping_every);
    ping.tick().await;
    loop {
        tokio::select! {
            _ = stop.changed() => {
                let _ = sink.send(Message::Close(None)).await;
                return Ok(());
            }
            frame = out.recv() => {
                let Some(frame) = frame else {
                    let _ = sink.send(Message::Close(None)).await;
                    return Ok(());
                };
                let written = sink.send(text(&frame)).await;
                match frame {
                    RelayFrame::Ack { seq } => *last_acked = (*last_acked).max(seq),
                    RelayFrame::Send { .. } => {
                        unconfirmed.push_back(frame);
                        if unconfirmed.len() > MAX_UNCONFIRMED {
                            unconfirmed.pop_front();
                        }
                    }
                    _ => {}
                }
                written.map_err(|e| e.to_string())?;
            }
            _ = ping.tick() => {
                if last_heard.elapsed() > options.silence_limit {
                    return Err("the relay went quiet".into());
                }
                sink.send(text(&RelayFrame::Ping)).await.map_err(|e| e.to_string())?;
            }
            message = stream.next() => {
                let message = match message {
                    None => return Err("the relay closed the connection".into()),
                    Some(Err(e)) => return Err(e.to_string()),
                    Some(Ok(message)) => message,
                };
                last_heard = Instant::now();
                // A connection that delivers is healthy: the next failure
                // starts the backoff from the beginning.
                *attempt = 0;
                let text = match message {
                    Message::Text(text) => text,
                    Message::Close(_) => return Err("the relay closed the connection".into()),
                    _ => continue,
                };
                let Ok(frame) = serde_json::from_str::<RelayFrame>(&text) else {
                    continue;
                };
                let event = match frame {
                    RelayFrame::Ping => {
                        sink.send(Message::text(r#"{"op":"pong"}"#)).await.map_err(|e| e.to_string())?;
                        continue;
                    }
                    RelayFrame::Deliver { seq, from, envelope } => {
                        RelayEvent::Deliver { seq, from, envelope }
                    }
                    RelayFrame::Sent { to, seq } => {
                        unconfirmed.pop_front();
                        RelayEvent::Sent { to, seq }
                    }
                    RelayFrame::Presence { device, online } => RelayEvent::Presence { device, online },
                    RelayFrame::Error { code, message, to } => {
                        // A refused send is answered with an error naming
                        // its recipient instead of `sent`.
                        if to.is_some() {
                            unconfirmed.pop_front();
                        }
                        RelayEvent::Error { code, message }
                    }
                    _ => continue,
                };
                if events.send(event).await.is_err() {
                    return Ok(());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn websocket_urls_follow_the_relay_address() {
        let options = RelayOptions::new("https://relay.example/base/", "dev 1", fixed_token("t"));
        assert_eq!(
            options.ws_url().unwrap(),
            "wss://relay.example/base/v1/ws?device=dev+1"
        );
        let local = RelayOptions::new("http://127.0.0.1:9", "d", fixed_token("t"));
        assert_eq!(local.ws_url().unwrap(), "ws://127.0.0.1:9/v1/ws?device=d");
    }

    #[test]
    fn backoff_grows_with_jitter_and_stays_capped() {
        let base = Duration::from_millis(100);
        let max = Duration::from_secs(5);
        for attempt in 0..40 {
            let wait = backoff(base, max, attempt);
            assert!(wait <= max);
            let cap = base
                .saturating_mul(1u32.checked_shl(attempt.min(16)).unwrap_or(u32::MAX))
                .min(max);
            assert!(wait >= cap / 2);
        }
    }
}
