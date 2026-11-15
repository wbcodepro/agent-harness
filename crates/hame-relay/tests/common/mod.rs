//! Shared test harness: a relay on a random port with recording mailer and
//! push sender, HTTP and WebSocket helpers.
#![allow(dead_code)]

pub mod idp;

use futures_util::{SinkExt, StreamExt};
use hame_relay::config::{Argon2Params, Config, RateLimits};
use hame_relay::mailer::MemoryMailer;
use hame_relay::push::RecordingPush;
use hame_relay::{Relay, Services};
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

pub type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

pub const PASSWORD: &str = "correct horse battery staple";

pub struct TestRelay {
    pub base: String,
    pub http: reqwest::Client,
    pub mailer: Arc<MemoryMailer>,
    pub push: Arc<RecordingPush>,
    pub state: hame_relay::AppState,
    _dir: tempfile::TempDir,
}

/// Starts a relay; `customize` adjusts the test configuration.
pub async fn spawn(customize: impl FnOnce(&mut Config)) -> TestRelay {
    let dir = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base = format!("http://{addr}");
    let mut config = Config::local(dir.path().join("relay.db"), url::Url::parse(&base).unwrap());
    config.app_url = url::Url::parse("https://app.example.test").unwrap();
    // Cheap hashes in tests; production parameters are covered by a unit test.
    config.argon2 = Argon2Params {
        m_kib: 1024,
        t: 1,
        p: 1,
    };
    config.rate_limits = RateLimits {
        auth_per_ip_per_min: 10_000,
        auth_per_account_per_min: 10_000,
        ws_frames_per_sec: 10_000,
        global_failed_logins_per_min: 10_000,
        max_buckets: 100_000,
    };
    config.device_code_interval = Duration::from_secs(1);
    customize(&mut config);
    let mailer = Arc::new(MemoryMailer::default());
    let push = Arc::new(RecordingPush::default());
    let services = Services {
        mailer: mailer.clone(),
        push: push.clone(),
        http: reqwest::Client::new(),
    };
    let relay = Relay::new(config, services).await.unwrap();
    let state = relay.state.clone();
    tokio::spawn(async move {
        relay
            .serve(listener, std::future::pending::<()>())
            .await
            .unwrap();
    });
    TestRelay {
        base,
        http: reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap(),
        mailer,
        push,
        state,
        _dir: dir,
    }
}

impl TestRelay {
    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    pub async fn post(&self, path: &str, body: Value, token: Option<&str>) -> (StatusCode, Value) {
        let mut req = self.http.post(self.url(path)).json(&body);
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }
        let res = req.send().await.unwrap();
        let status = res.status();
        let text = res.text().await.unwrap();
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    pub async fn get(&self, path: &str, token: Option<&str>) -> (StatusCode, Value) {
        let mut req = self.http.get(self.url(path));
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }
        let res = req.send().await.unwrap();
        let status = res.status();
        let text = res.text().await.unwrap();
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    pub async fn delete(&self, path: &str, token: Option<&str>) -> StatusCode {
        let mut req = self.http.delete(self.url(path));
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }
        req.send().await.unwrap().status()
    }

    /// Waits until the mailer has `n` messages for `to` and returns the last.
    pub async fn wait_mail(&self, to: &str, n: usize) -> hame_relay::mailer::Email {
        for _ in 0..200 {
            let msgs = self.mailer.messages_to(to);
            if msgs.len() >= n {
                return msgs[n - 1].clone();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("no email #{n} for {to}");
    }

    /// Signs up, verifies and logs in; returns the login response.
    pub async fn account(&self, name: &str, email: &str) -> Value {
        let (s, _) = self
            .post(
                "/v1/auth/signup",
                json!({ "name": name, "email": email, "password": PASSWORD }),
                None,
            )
            .await;
        assert_eq!(s, StatusCode::ACCEPTED);
        let mail = self.wait_mail(email, 1).await;
        let token = link_token(&mail.text);
        let (s, _) = self
            .post("/v1/auth/verify", json!({ "token": token }), None)
            .await;
        assert_eq!(s, StatusCode::OK);
        self.login(email, PASSWORD).await
    }

    pub async fn login(&self, email: &str, password: &str) -> Value {
        let (s, v) = self
            .post(
                "/v1/auth/login",
                json!({ "email": email, "password": password }),
                None,
            )
            .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        v
    }

    /// Registers the session's device with random keys; returns its id.
    pub async fn device(&self, token: &str, kind: &str) -> String {
        let keys = DeviceKeys::generate();
        self.device_with_keys(token, kind, &keys).await
    }

    pub async fn device_with_keys(&self, token: &str, kind: &str, keys: &DeviceKeys) -> String {
        let (s, v) = self
            .post(
                "/v1/devices",
                json!({
                    "name": format!("{kind} device"),
                    "kind": kind,
                    "ed25519": keys.ed25519_public(),
                    "x25519": hame_relay::util::b64url(&hame_relay::util::random_bytes::<32>()),
                }),
                Some(token),
            )
            .await;
        assert_eq!(s, StatusCode::CREATED, "{v}");
        v["id"].as_str().unwrap().to_string()
    }

    /// Signs in a second session for the same account and registers a
    /// device for it: `(access_token, device_id)`.
    pub async fn second_device(&self, email: &str, kind: &str) -> (String, String) {
        let login = self.login(email, PASSWORD).await;
        let token = login["access_token"].as_str().unwrap().to_string();
        let device = self.device(&token, kind).await;
        (token, device)
    }

    pub async fn ws(&self, device: &str, token: &str) -> Ws {
        let url = format!(
            "{}/v1/ws?device={device}",
            self.base.replace("http://", "ws://")
        );
        let mut req = url.into_client_request().unwrap();
        req.headers_mut()
            .insert("authorization", format!("Bearer {token}").parse().unwrap());
        let (ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
        ws
    }
}

/// An Ed25519 key pair for a device.
pub struct DeviceKeys {
    pub pair: aws_lc_rs::signature::Ed25519KeyPair,
}

impl DeviceKeys {
    pub fn generate() -> Self {
        Self {
            pair: aws_lc_rs::signature::Ed25519KeyPair::generate().unwrap(),
        }
    }

    pub fn ed25519_public(&self) -> String {
        use aws_lc_rs::signature::KeyPair;
        hame_relay::util::b64url(self.pair.public_key().as_ref())
    }

    pub fn sign(&self, msg: &[u8]) -> String {
        hame_relay::util::b64url(self.pair.sign(msg).as_ref())
    }
}

/// The `token=` value of the first link in an email.
pub fn link_token(text: &str) -> String {
    let start = text.find("token=").expect("link with token") + "token=".len();
    text[start..]
        .split(|c: char| c.is_whitespace() || c == '&')
        .next()
        .unwrap()
        .to_string()
}

pub fn access(v: &Value) -> String {
    v["access_token"].as_str().unwrap().to_string()
}

pub async fn ws_send(ws: &mut Ws, v: Value) {
    ws.send(Message::Text(v.to_string().into())).await.unwrap();
}

/// The next JSON frame, skipping relay pings.
pub async fn ws_recv(ws: &mut Ws) -> Value {
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("timed out waiting for a frame")
            .expect("stream ended")
            .expect("websocket error");
        if let Message::Text(t) = msg {
            let v: Value = serde_json::from_str(t.as_str()).unwrap();
            if v["op"] != "ping" {
                return v;
            }
        }
    }
}

/// The next frame that is not a presence notice.
pub async fn ws_recv_data(ws: &mut Ws) -> Value {
    loop {
        let v = ws_recv(ws).await;
        if v["op"] != "presence" {
            return v;
        }
    }
}

/// Asserts that no non-presence frame arrives within `ms`.
pub async fn ws_quiet(ws: &mut Ws, ms: u64) {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(ms);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            return;
        }
        match tokio::time::timeout(left, ws.next()).await {
            Err(_) => return,
            Ok(Some(Ok(Message::Text(t)))) => {
                let v: Value = serde_json::from_str(t.as_str()).unwrap();
                assert!(
                    v["op"] == "presence" || v["op"] == "ping",
                    "unexpected frame {v}"
                );
            }
            Ok(_) => return,
        }
    }
}
