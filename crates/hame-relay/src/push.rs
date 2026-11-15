//! Push notifications. A push carries only opaque ids ("something is
//! waiting for device X from device Y"); the app connects, fetches and
//! decrypts.

use crate::config::{ApnsConfig, FcmConfig, PushConfig};
use async_trait::async_trait;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde_json::json;
use std::sync::{Arc, Mutex};
use tokio::sync::Mutex as AsyncMutex;

/// The provider a push token belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PushProvider {
    Apns,
    Fcm,
}

impl PushProvider {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "apns" => Some(Self::Apns),
            "fcm" => Some(Self::Fcm),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Apns => "apns",
            Self::Fcm => "fcm",
        }
    }
}

/// One notification.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PushMessage {
    pub provider: PushProvider,
    pub token: String,
    /// The device that has something waiting.
    pub recipient: String,
    /// Who sent it.
    pub sender: String,
    /// The queued envelope's sequence number.
    pub seq: i64,
}

/// Why a push failed.
#[derive(Debug, thiserror::Error)]
pub enum PushError {
    /// The provider says the token is no longer valid; it should be dropped.
    #[error("push token is no longer registered")]
    Unregistered,
    #[error("push provider not configured")]
    NotConfigured,
    #[error("push failed: {0}")]
    Failed(String),
}

#[async_trait]
pub trait PushSender: Send + Sync + 'static {
    async fn send(&self, message: &PushMessage) -> Result<(), PushError>;
}

/// Builds the sender the configuration describes; with neither APNs nor
/// FCM configured, a sender that does nothing.
pub fn from_config(cfg: &PushConfig, http: reqwest::Client) -> Result<Arc<dyn PushSender>, String> {
    if cfg.apns.is_none() && cfg.fcm.is_none() {
        return Ok(Arc::new(NoopPush));
    }
    Ok(Arc::new(LivePush {
        apns: cfg
            .apns
            .as_ref()
            .map(|c| ApnsSender::new(c.clone(), http.clone()))
            .transpose()?,
        fcm: cfg
            .fcm
            .as_ref()
            .map(|c| FcmSender::new(c.clone(), http.clone()))
            .transpose()?,
    }))
}

/// Does nothing.
pub struct NoopPush;

#[async_trait]
impl PushSender for NoopPush {
    async fn send(&self, _message: &PushMessage) -> Result<(), PushError> {
        Ok(())
    }
}

/// Records pushes in memory (tests).
#[derive(Default)]
pub struct RecordingPush {
    sent: Mutex<Vec<PushMessage>>,
}

impl RecordingPush {
    pub fn messages(&self) -> Vec<PushMessage> {
        self.sent.lock().map(|v| v.clone()).unwrap_or_default()
    }
}

#[async_trait]
impl PushSender for RecordingPush {
    async fn send(&self, message: &PushMessage) -> Result<(), PushError> {
        if let Ok(mut v) = self.sent.lock() {
            v.push(message.clone());
        }
        Ok(())
    }
}

/// Dispatches to APNs or FCM by the token's provider.
pub struct LivePush {
    apns: Option<ApnsSender>,
    fcm: Option<FcmSender>,
}

#[async_trait]
impl PushSender for LivePush {
    async fn send(&self, message: &PushMessage) -> Result<(), PushError> {
        match message.provider {
            PushProvider::Apns => match &self.apns {
                Some(s) => s.send(message).await,
                None => Err(PushError::NotConfigured),
            },
            PushProvider::Fcm => match &self.fcm {
                Some(s) => s.send(message).await,
                None => Err(PushError::NotConfigured),
            },
        }
    }
}

/// A cached bearer token and when it stops being reused.
struct CachedToken {
    value: String,
    renew_at: i64,
}

/// APNs over HTTP/2 with a provider token (ES256 JWT from the `.p8` key).
pub struct ApnsSender {
    cfg: ApnsConfig,
    key: EncodingKey,
    http: reqwest::Client,
    token: AsyncMutex<Option<CachedToken>>,
}

impl ApnsSender {
    pub fn new(cfg: ApnsConfig, http: reqwest::Client) -> Result<Self, String> {
        let key = EncodingKey::from_ec_pem(cfg.private_key_pem.as_bytes())
            .map_err(|e| format!("APNs key: {e}"))?;
        Ok(Self {
            cfg,
            key,
            http,
            token: AsyncMutex::new(None),
        })
    }

    /// The provider token; Apple accepts one for up to an hour and refuses
    /// refreshing more than every 20 minutes, so it is reused for 50.
    async fn bearer(&self) -> Result<String, PushError> {
        let now = crate::util::now();
        let mut guard = self.token.lock().await;
        if let Some(t) = guard.as_ref()
            && t.renew_at > now
        {
            return Ok(t.value.clone());
        }
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(self.cfg.key_id.clone());
        let claims = json!({ "iss": self.cfg.team_id, "iat": now });
        let value = jsonwebtoken::encode(&header, &claims, &self.key)
            .map_err(|e| PushError::Failed(e.to_string()))?;
        *guard = Some(CachedToken {
            value: value.clone(),
            renew_at: now + 50 * 60,
        });
        Ok(value)
    }

    async fn send(&self, m: &PushMessage) -> Result<(), PushError> {
        let bearer = self.bearer().await?;
        let url = format!(
            "{}/3/device/{}",
            self.cfg.endpoint.trim_end_matches('/'),
            m.token
        );
        let body = json!({
            "aps": {
                "alert": { "title": "hame", "body": "A session needs you" },
                "sound": "default",
                "mutable-content": 1
            },
            "r": m.recipient,
            "f": m.sender,
            "s": m.seq,
        });
        let res = self
            .http
            .post(url)
            .bearer_auth(bearer)
            .header("apns-topic", &self.cfg.topic)
            .header("apns-push-type", "alert")
            .header("apns-priority", "10")
            .json(&body)
            .send()
            .await
            .map_err(|e| PushError::Failed(e.to_string()))?;
        match res.status().as_u16() {
            200 => Ok(()),
            410 => Err(PushError::Unregistered),
            400 => {
                let text = res.text().await.unwrap_or_default();
                if text.contains("BadDeviceToken") {
                    Err(PushError::Unregistered)
                } else {
                    Err(PushError::Failed(format!("APNs 400: {text}")))
                }
            }
            s => Err(PushError::Failed(format!("APNs status {s}"))),
        }
    }
}

/// FCM HTTP v1 with a service-account OAuth token.
pub struct FcmSender {
    cfg: FcmConfig,
    key: EncodingKey,
    http: reqwest::Client,
    token: AsyncMutex<Option<CachedToken>>,
}

impl FcmSender {
    pub fn new(cfg: FcmConfig, http: reqwest::Client) -> Result<Self, String> {
        let key = EncodingKey::from_rsa_pem(cfg.private_key_pem.as_bytes())
            .map_err(|e| format!("FCM key: {e}"))?;
        Ok(Self {
            cfg,
            key,
            http,
            token: AsyncMutex::new(None),
        })
    }

    async fn bearer(&self) -> Result<String, PushError> {
        let now = crate::util::now();
        let mut guard = self.token.lock().await;
        if let Some(t) = guard.as_ref()
            && t.renew_at > now
        {
            return Ok(t.value.clone());
        }
        let claims = json!({
            "iss": self.cfg.client_email,
            "scope": "https://www.googleapis.com/auth/firebase.messaging",
            "aud": self.cfg.token_uri,
            "iat": now,
            "exp": now + 3600,
        });
        let assertion = jsonwebtoken::encode(&Header::new(Algorithm::RS256), &claims, &self.key)
            .map_err(|e| PushError::Failed(e.to_string()))?;
        let res = self
            .http
            .post(&self.cfg.token_uri)
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", assertion.as_str()),
            ])
            .send()
            .await
            .map_err(|e| PushError::Failed(e.to_string()))?;
        if !res.status().is_success() {
            return Err(PushError::Failed(format!(
                "FCM token endpoint status {}",
                res.status()
            )));
        }
        let v: serde_json::Value = res
            .json()
            .await
            .map_err(|e| PushError::Failed(e.to_string()))?;
        let value = v
            .get("access_token")
            .and_then(|t| t.as_str())
            .ok_or_else(|| PushError::Failed("FCM token response without access_token".into()))?
            .to_string();
        let expires_in = v.get("expires_in").and_then(|e| e.as_i64()).unwrap_or(3600);
        *guard = Some(CachedToken {
            value: value.clone(),
            renew_at: now + (expires_in - 300).max(60),
        });
        Ok(value)
    }

    async fn send(&self, m: &PushMessage) -> Result<(), PushError> {
        let bearer = self.bearer().await?;
        let url = format!(
            "{}/v1/projects/{}/messages:send",
            self.cfg.endpoint.trim_end_matches('/'),
            self.cfg.project_id
        );
        // Data-only: the app decides what to show after decrypting.
        let body = json!({
            "message": {
                "token": m.token,
                "data": { "r": m.recipient, "f": m.sender, "s": m.seq.to_string() },
                "android": { "priority": "high" }
            }
        });
        let res = self
            .http
            .post(url)
            .bearer_auth(bearer)
            .json(&body)
            .send()
            .await
            .map_err(|e| PushError::Failed(e.to_string()))?;
        match res.status().as_u16() {
            200 => Ok(()),
            404 => Err(PushError::Unregistered),
            400 => {
                let text = res.text().await.unwrap_or_default();
                if text.contains("UNREGISTERED") || text.contains("INVALID_ARGUMENT") {
                    Err(PushError::Unregistered)
                } else {
                    Err(PushError::Failed(format!("FCM 400: {text}")))
                }
            }
            s => Err(PushError::Failed(format!("FCM status {s}"))),
        }
    }
}
