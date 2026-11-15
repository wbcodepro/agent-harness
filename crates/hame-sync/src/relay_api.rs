//! The parts of the relay's HTTP API (§3) a host uses: device-code sign-in,
//! token refresh, device registration and publishing grants.
//!
//! Shapes as in the relay's docs/relay/API.md (crates/hame-relay); the mock
//! relay implements the same:
//! - `POST /v1/auth/device/start {client_name}` →
//!   `{device_code, user_code, verification_url, verification_url_complete,
//!   expires_in, interval}`;
//! - `POST /v1/auth/device/token {device_code}` → tokens, or 400 with
//!   `error`: `authorization_pending`, `slow_down` (with a new `interval`),
//!   `access_denied`, `expired_token`, `invalid_grant`;
//! - `POST /v1/auth/refresh {refresh_token}` → tokens (the refresh token
//!   rotates; presenting a used one revokes the session);
//! - `POST /v1/devices {name, kind, ed25519, x25519}` (bearer) → `201`
//!   device `{id, …, fingerprint}`; binds the session to the device, so the
//!   same tokens then open that device's WebSocket;
//! - `POST /v1/hosts/{id}/grants {version, document, signature}` (bearer,
//!   the host itself) → `204`, `409 stale_version`;
//!   `GET /v1/hosts/{id}/grants` → `{host_id, version, document, signature}`.
//!
//! Device-code sessions have the relay's `host` scope: enough for all of
//! the above, nothing more. Answers are read up to 64 KiB.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::SyncError;
use crate::keys::{PublicIdentity, b64};
use crate::pairing::SignedGrantList;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceCode {
    pub user_code: String,
    pub verification_url: String,
    /// The verification page with the code filled in, when the relay sends it.
    #[serde(default)]
    pub verification_url_complete: String,
    pub device_code: String,
    #[serde(default = "default_interval")]
    pub interval: u64,
    #[serde(default = "default_expiry")]
    pub expires_in: u64,
}

fn default_interval() -> u64 {
    5
}

fn default_expiry() -> u64 {
    600
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tokens {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: String,
}

/// Largest answer body read from the relay.
pub const MAX_ANSWER: usize = 64 * 1024;

#[derive(Clone)]
pub struct RelayApi {
    http: reqwest::Client,
    base: String,
}

fn relay_error(error: impl std::fmt::Display) -> SyncError {
    SyncError::Relay(error.to_string())
}

impl RelayApi {
    /// A client for the relay at `base`; refuses unencrypted addresses
    /// other than this machine's.
    pub fn new(base: &str) -> Result<Self, SyncError> {
        crate::relay::check_relay_url(base)?;
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("an HTTP client"),
            base: base.trim_end_matches('/').into(),
        })
    }

    /// Reads an answer body, at most [`MAX_ANSWER`] bytes.
    async fn read(response: reqwest::Response) -> Result<(u16, Value), SyncError> {
        let status = response.status().as_u16();
        let mut response = response;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(relay_error)? {
            bytes.extend_from_slice(&chunk);
            if bytes.len() > MAX_ANSWER {
                return Err(SyncError::Relay("the relay's answer is too large".into()));
            }
        }
        let text = String::from_utf8_lossy(&bytes).into_owned();
        Ok((
            status,
            serde_json::from_str(&text).unwrap_or(Value::String(text)),
        ))
    }

    async fn post(
        &self,
        path: &str,
        token: Option<&str>,
        body: &Value,
    ) -> Result<(u16, Value), SyncError> {
        let mut request = self
            .http
            .post(format!("{}{path}", self.base))
            .header("content-type", "application/json")
            .body(body.to_string());
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.map_err(relay_error)?;
        Self::read(response).await
    }

    fn expect<T: serde::de::DeserializeOwned>(
        what: &str,
        (status, body): (u16, Value),
    ) -> Result<T, SyncError> {
        if !(200..300).contains(&status) {
            return Err(SyncError::Relay(format!("{what}: {status} {body}")));
        }
        serde_json::from_value(body).map_err(|e| SyncError::Relay(format!("{what}: {e}")))
    }

    /// Starts a device-code sign-in for a host named `name`.
    pub async fn device_start(&self, name: &str) -> Result<DeviceCode, SyncError> {
        let answer = self
            .post("/v1/auth/device/start", None, &json!({"client_name": name}))
            .await?;
        Self::expect("device sign-in", answer)
    }

    /// Polls until the user approves the code in a signed-in app.
    pub async fn device_wait(&self, code: &DeviceCode) -> Result<Tokens, SyncError> {
        let mut interval = Duration::from_secs(code.interval.max(1));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(code.expires_in.max(1));
        loop {
            let (status, body) = self
                .post(
                    "/v1/auth/device/token",
                    None,
                    &json!({"device_code": code.device_code}),
                )
                .await?;
            if (200..300).contains(&status) {
                return Self::expect("device sign-in", (status, body));
            }
            match body["error"].as_str() {
                Some("authorization_pending") => {}
                Some("slow_down") => {
                    interval = body["interval"]
                        .as_u64()
                        .map(Duration::from_secs)
                        .unwrap_or(interval + Duration::from_secs(5));
                }
                Some("expired_token") | Some("invalid_grant") => {
                    return Err(SyncError::Relay("the sign-in code expired".into()));
                }
                Some("access_denied") => {
                    return Err(SyncError::Relay("the sign-in was denied".into()));
                }
                _ => return Err(SyncError::Relay(format!("device sign-in: {status} {body}"))),
            }
            if tokio::time::Instant::now() + interval > deadline {
                return Err(SyncError::Relay("the sign-in code expired".into()));
            }
            tokio::time::sleep(interval).await;
        }
    }

    pub async fn refresh(&self, refresh_token: &str) -> Result<Tokens, SyncError> {
        let (status, answer) = self
            .post(
                "/v1/auth/refresh",
                None,
                &json!({"refresh_token": refresh_token}),
            )
            .await?;
        if status == 401 {
            return Err(SyncError::Unauthorized);
        }
        Self::expect("token refresh", (status, answer))
    }

    /// Registers this host's public keys; returns its device id.
    pub async fn register_host(
        &self,
        token: &str,
        name: &str,
        keys: &PublicIdentity,
    ) -> Result<String, SyncError> {
        let answer = self
            .post(
                "/v1/devices",
                Some(token),
                &json!({
                    "name": name,
                    "kind": "host",
                    "ed25519": b64(&keys.ed),
                    "x25519": b64(&keys.x),
                }),
            )
            .await?;
        let body: Value = Self::expect("device registration", answer)?;
        body["id"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| SyncError::Relay("device registration: no id".into()))
    }

    /// Publishes a grant list. `Ok(None)` when published; `Ok(Some(v))`
    /// when the relay holds version `v` already (`409 stale_version`), so
    /// the caller can publish again above it; `Err(Unauthorized)` for an
    /// expired token.
    pub async fn post_grants(
        &self,
        token: &str,
        host: &str,
        grants: &SignedGrantList,
    ) -> Result<Option<u64>, SyncError> {
        let body = serde_json::to_value(grants).expect("serialises");
        let (status, answer) = self
            .post(&format!("/v1/hosts/{host}/grants"), Some(token), &body)
            .await?;
        match status {
            200..=299 => Ok(None),
            401 => Err(SyncError::Unauthorized),
            409 if answer["error"] == "stale_version" => {
                Ok(Some(self.grants_version(token, host).await?))
            }
            _ => Err(SyncError::Relay(format!(
                "publishing grants: {status} {answer}"
            ))),
        }
    }

    /// The version of the grant list the relay holds for `host` (0: none).
    pub async fn grants_version(&self, token: &str, host: &str) -> Result<u64, SyncError> {
        let response = self
            .http
            .get(format!("{}/v1/hosts/{host}/grants", self.base))
            .bearer_auth(token)
            .send()
            .await
            .map_err(relay_error)?;
        let (status, answer) = Self::read(response).await?;
        match status {
            200..=299 => Ok(answer["version"].as_u64().unwrap_or(0)),
            404 => Ok(0),
            401 => Err(SyncError::Unauthorized),
            _ => Err(SyncError::Relay(format!(
                "reading grants: {status} {answer}"
            ))),
        }
    }
}
