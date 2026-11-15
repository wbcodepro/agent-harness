//! Shared application state.

use crate::config::Config;
use crate::mailer::{Email, Mailer};
use crate::oidc::{OidcProvider, ProviderKind};
use crate::password::PasswordHasher;
use crate::push::PushSender;
use crate::ratelimit::{Limit, RateLimiter};
use crate::repo::{AuditEntry, Repository};
use crate::ws::Hub;
use std::net::IpAddr;
use std::ops::Deref;
use std::sync::Arc;
use webauthn_rs::Webauthn;

/// The services the relay depends on, injectable for tests.
pub struct Services {
    pub mailer: Arc<dyn Mailer>,
    pub push: Arc<dyn PushSender>,
    /// HTTP client for identity providers and push services.
    pub http: reqwest::Client,
}

/// Cheaply clonable handle to the state.
#[derive(Clone)]
pub struct AppState(Arc<Inner>);

pub struct Inner {
    pub config: Config,
    pub repo: Arc<dyn Repository>,
    pub mailer: Arc<dyn Mailer>,
    pub push: Arc<dyn PushSender>,
    pub hub: Hub,
    pub limiter: RateLimiter,
    pub hasher: PasswordHasher,
    pub webauthn: Option<Webauthn>,
    pub google: Option<OidcProvider>,
    pub apple: Option<OidcProvider>,
    pub enumeration_key: [u8; 32],
}

impl Deref for AppState {
    type Target = Inner;
    fn deref(&self) -> &Inner {
        &self.0
    }
}

impl AppState {
    pub fn new(inner: Inner) -> Self {
        Self(Arc::new(inner))
    }

    pub fn provider(&self, kind: ProviderKind) -> Option<&OidcProvider> {
        match kind {
            ProviderKind::Google => self.google.as_ref(),
            ProviderKind::Apple => self.apple.as_ref(),
        }
    }

    /// Records an audit event. Failures are logged, never returned.
    pub async fn audit(
        &self,
        event: &str,
        account_id: Option<&str>,
        device_id: Option<&str>,
        ip: Option<IpAddr>,
        detail: Option<String>,
    ) {
        let entry = AuditEntry {
            at: crate::util::now(),
            event: event.to_string(),
            account_id: account_id.map(str::to_string),
            device_id: device_id.map(str::to_string),
            ip: ip.map(|i| i.to_string()),
            detail,
        };
        tracing::info!(
            event,
            account = account_id.unwrap_or("-"),
            device = device_id.unwrap_or("-"),
            "audit"
        );
        if let Err(e) = self.repo.audit(&entry).await {
            tracing::error!(error = %e, "audit log write failed");
        }
    }

    /// Sends an email in the background, so that response time does not
    /// depend on whether a message was sent. At most three messages at once
    /// and six an hour go to one address; further ones are dropped silently
    /// (the HTTP answer stays the same).
    pub fn send_email(&self, email: Email) {
        let limit = Limit {
            burst: 3,
            per_hour: 6,
        };
        if self
            .limiter
            .check(&format!("mail:{}", email.to), limit)
            .is_err()
        {
            tracing::warn!("email to one address rate limited; not sent");
            return;
        }
        let mailer = self.mailer.clone();
        tokio::spawn(async move {
            if let Err(e) = mailer.send(email).await {
                tracing::error!(error = %e, "sending email failed");
            }
        });
    }

    /// Per-account (or per-email) limit on authentication attempts.
    pub fn check_account_limit(
        &self,
        scope: &str,
        key: &str,
    ) -> Result<(), crate::error::ApiError> {
        let limit = Limit::per_minute(self.config.rate_limits.auth_per_account_per_min);
        self.limiter
            .check(&format!("{scope}:{key}"), limit)
            .map_err(crate::error::ApiError::rate_limited)
    }

    /// A URL in the web app.
    pub fn app_link(&self, path: &str, query: &[(&str, &str)]) -> String {
        let mut url = self.config.app_url.clone();
        let base = url.path().trim_end_matches('/').to_string();
        url.set_path(&format!("{base}{path}"));
        if !query.is_empty() {
            url.query_pairs_mut().extend_pairs(query);
        }
        url.to_string()
    }

    /// A URL on the relay itself.
    pub fn public_link(&self, path: &str) -> String {
        let mut url = self.config.public_url.clone();
        let base = url.path().trim_end_matches('/').to_string();
        url.set_path(&format!("{base}{path}"));
        url.to_string()
    }
}
