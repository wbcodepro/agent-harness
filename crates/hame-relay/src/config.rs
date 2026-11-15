//! Configuration, read from the environment (see docs/relay/DEPLOY.md).

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

/// Errors while reading the configuration.
#[derive(Debug, thiserror::Error)]
#[error("configuration: {0}")]
pub struct ConfigError(pub String);

fn bad(msg: impl Into<String>) -> ConfigError {
    ConfigError(msg.into())
}

/// Argon2id cost parameters.
#[derive(Clone, Copy, Debug)]
pub struct Argon2Params {
    /// Memory in KiB.
    pub m_kib: u32,
    /// Iterations.
    pub t: u32,
    /// Parallelism.
    pub p: u32,
}

impl Default for Argon2Params {
    /// The production parameters: m = 64 MiB, t = 3, p = 1.
    fn default() -> Self {
        Self {
            m_kib: 64 * 1024,
            t: 3,
            p: 1,
        }
    }
}

/// How outgoing email is delivered.
#[derive(Clone, Debug)]
pub enum MailerConfig {
    /// Send through an SMTP server.
    Smtp(SmtpConfig),
    /// Write each message to the log (development only: the log then holds
    /// verification and reset links).
    Log,
    /// Drop every message.
    Disabled,
}

/// SMTP connection settings.
#[derive(Clone, Debug)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    /// `starttls` (default), `tls` (implicit, port 465) or `none` (local
    /// relays only).
    pub tls: SmtpTls,
    pub from: String,
}

/// SMTP transport security.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SmtpTls {
    StartTls,
    Tls,
    None,
}

/// An OpenID Connect provider (Google or Apple).
#[derive(Clone, Debug)]
pub struct OidcConfig {
    /// The issuer; discovery is fetched from
    /// `{issuer}/.well-known/openid-configuration`.
    pub issuer: String,
    pub client_id: String,
    /// Google: the client secret. Unused for Apple.
    pub client_secret: Option<String>,
    /// Apple only: the key that signs the client-secret JWT.
    pub apple_key: Option<AppleKeyConfig>,
}

/// The Apple "Sign in with Apple" private key (a `.p8` file, PKCS#8 PEM).
#[derive(Clone)]
pub struct AppleKeyConfig {
    pub team_id: String,
    pub key_id: String,
    pub private_key_pem: String,
}

impl std::fmt::Debug for AppleKeyConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppleKeyConfig")
            .field("team_id", &self.team_id)
            .field("key_id", &self.key_id)
            .field("private_key_pem", &"<redacted>")
            .finish()
    }
}

/// WebAuthn relying-party settings.
#[derive(Clone, Debug)]
pub struct WebauthnConfig {
    /// The relying-party id, a registrable domain such as `example.com`.
    pub rp_id: String,
    pub rp_name: String,
    /// Origins allowed to use passkeys (the web app, the desktop app's
    /// origin, Android `android:apk-key-hash:` origins).
    pub origins: Vec<url::Url>,
}

/// Push notification settings.
#[derive(Clone, Debug, Default)]
pub struct PushConfig {
    pub apns: Option<ApnsConfig>,
    pub fcm: Option<FcmConfig>,
}

/// Apple Push Notification service, token-based authentication.
#[derive(Clone)]
pub struct ApnsConfig {
    pub team_id: String,
    pub key_id: String,
    pub private_key_pem: String,
    /// The app's bundle id.
    pub topic: String,
    /// Base URL; `https://api.push.apple.com` or the sandbox.
    pub endpoint: String,
}

impl std::fmt::Debug for ApnsConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApnsConfig")
            .field("team_id", &self.team_id)
            .field("key_id", &self.key_id)
            .field("topic", &self.topic)
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

/// Firebase Cloud Messaging HTTP v1 with a service account.
#[derive(Clone)]
pub struct FcmConfig {
    pub project_id: String,
    pub client_email: String,
    pub private_key_pem: String,
    pub token_uri: String,
    /// Base URL; `https://fcm.googleapis.com`.
    pub endpoint: String,
}

impl std::fmt::Debug for FcmConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FcmConfig")
            .field("project_id", &self.project_id)
            .field("client_email", &self.client_email)
            .field("token_uri", &self.token_uri)
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

/// Request rate limits.
#[derive(Clone, Copy, Debug)]
pub struct RateLimits {
    /// Requests per minute per client IP on authentication routes.
    pub auth_per_ip_per_min: u32,
    /// Attempts per minute per account (or email address) on routes that
    /// name one.
    pub auth_per_account_per_min: u32,
    /// WebSocket frames per second per connection (burst is four times).
    pub ws_frames_per_sec: u32,
}

impl Default for RateLimits {
    fn default() -> Self {
        Self {
            auth_per_ip_per_min: 30,
            auth_per_account_per_min: 10,
            ws_frames_per_sec: 50,
        }
    }
}

/// The relay configuration.
#[derive(Clone, Debug)]
pub struct Config {
    pub bind: SocketAddr,
    /// SQLite database file.
    pub database_path: PathBuf,
    /// The public base URL of the relay, e.g. `https://relay.example.com`.
    pub public_url: url::Url,
    /// The base URL of the web app (email links, device approval page).
    pub app_url: url::Url,
    /// Origins allowed by CORS. Empty: no cross-origin access.
    pub cors_origins: Vec<String>,
    /// Trust the last `X-Forwarded-For` entry (set when behind a reverse
    /// proxy that overwrites or appends it).
    pub trust_proxy: bool,
    /// Send `Strict-Transport-Security`.
    pub hsts: bool,
    pub max_body_bytes: usize,
    pub mailer: MailerConfig,
    pub google: Option<OidcConfig>,
    pub apple: Option<OidcConfig>,
    /// Where the OAuth callback may send the browser afterwards (exact
    /// match of scheme, host, port and path).
    pub oauth_return_urls: Vec<String>,
    pub webauthn: Option<WebauthnConfig>,
    pub push: PushConfig,
    pub rate_limits: RateLimits,
    pub argon2: Argon2Params,
    /// How many password hashes may run at once (each takes `m_kib` memory).
    pub argon2_concurrency: usize,
    /// Key for deriving stable fake passkey credential ids for unknown
    /// emails (hides which addresses have accounts). Random per process when
    /// not set.
    pub enumeration_key: Option<[u8; 32]>,
    pub access_token_ttl: Duration,
    pub refresh_token_ttl: Duration,
    /// How long undelivered or unacknowledged envelopes are kept.
    pub mailbox_ttl: Duration,
    /// Envelopes queued per recipient at most.
    pub mailbox_max_per_device: i64,
    /// Interval between WebSocket pings from the relay.
    pub ws_ping_interval: Duration,
    /// A WebSocket that sends nothing for this long is closed.
    pub ws_idle_timeout: Duration,
    /// Device-code flow: minimum polling interval and code lifetime.
    pub device_code_interval: Duration,
    pub device_code_ttl: Duration,
}

impl Config {
    /// A configuration for local development and tests: no external
    /// services, in-process defaults.
    pub fn local(database_path: PathBuf, public_url: url::Url) -> Self {
        Self {
            bind: "127.0.0.1:8080".parse().expect("valid address"),
            database_path,
            app_url: public_url.clone(),
            public_url,
            cors_origins: Vec::new(),
            trust_proxy: false,
            hsts: false,
            max_body_bytes: 64 * 1024,
            mailer: MailerConfig::Log,
            google: None,
            apple: None,
            oauth_return_urls: Vec::new(),
            webauthn: None,
            push: PushConfig::default(),
            rate_limits: RateLimits::default(),
            argon2: Argon2Params::default(),
            argon2_concurrency: 4,
            enumeration_key: None,
            access_token_ttl: Duration::from_secs(15 * 60),
            refresh_token_ttl: Duration::from_secs(30 * 24 * 3600),
            mailbox_ttl: Duration::from_secs(24 * 3600),
            mailbox_max_per_device: 10_000,
            ws_ping_interval: Duration::from_secs(25),
            ws_idle_timeout: Duration::from_secs(75),
            device_code_interval: Duration::from_secs(5),
            device_code_ttl: Duration::from_secs(600),
        }
    }

    /// Read the configuration from `RELAY_*` environment variables.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    /// Read the configuration through a lookup function (testable).
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let var = |k: &str| {
            get(k)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        // A secret may come from NAME or from the file named by NAME_FILE
        // (Docker secrets).
        let secret = |k: &str| -> Result<Option<String>, ConfigError> {
            if let Some(v) = get(k).filter(|v| !v.trim().is_empty()) {
                return Ok(Some(v));
            }
            match var(&format!("{k}_FILE")) {
                Some(path) => std::fs::read_to_string(&path)
                    .map(|s| Some(s.trim_end().to_string()))
                    .map_err(|e| bad(format!("{k}_FILE: cannot read {path}: {e}"))),
                None => Ok(None),
            }
        };
        let flag = |k: &str| -> Result<Option<bool>, ConfigError> {
            match var(k).as_deref() {
                None => Ok(None),
                Some("1" | "true" | "yes" | "on") => Ok(Some(true)),
                Some("0" | "false" | "no" | "off") => Ok(Some(false)),
                Some(other) => Err(bad(format!("{k}: expected true or false, got {other:?}"))),
            }
        };
        fn num<T: std::str::FromStr>(k: &str, v: Option<String>) -> Result<Option<T>, ConfigError> {
            v.map(|s| {
                s.parse::<T>()
                    .map_err(|_| bad(format!("{k}: not a number")))
            })
            .transpose()
        }
        let list = |k: &str| -> Vec<String> {
            var(k)
                .map(|v| {
                    v.split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default()
        };
        let parse_url = |k: &str, v: &str| -> Result<url::Url, ConfigError> {
            url::Url::parse(v).map_err(|e| bad(format!("{k}: {e}")))
        };

        let public_url = parse_url(
            "RELAY_PUBLIC_URL",
            &var("RELAY_PUBLIC_URL").ok_or_else(|| bad("RELAY_PUBLIC_URL is required"))?,
        )?;
        let database_path =
            PathBuf::from(var("RELAY_DATABASE_PATH").unwrap_or_else(|| "relay.db".into()));
        let mut cfg = Self::local(database_path, public_url.clone());

        if let Some(bind) = var("RELAY_BIND") {
            cfg.bind = bind
                .parse()
                .map_err(|_| bad("RELAY_BIND: expected host:port"))?;
        }
        if let Some(app) = var("RELAY_APP_URL") {
            cfg.app_url = parse_url("RELAY_APP_URL", &app)?;
        }
        cfg.cors_origins = list("RELAY_CORS_ORIGINS");
        cfg.trust_proxy = flag("RELAY_TRUST_PROXY")?.unwrap_or(false);
        cfg.hsts = flag("RELAY_HSTS")?.unwrap_or(public_url.scheme() == "https");
        if let Some(n) = num::<usize>("RELAY_MAX_BODY_BYTES", var("RELAY_MAX_BODY_BYTES"))? {
            cfg.max_body_bytes = n;
        }

        cfg.mailer = match var("RELAY_MAILER").as_deref().unwrap_or("log") {
            "smtp" => MailerConfig::Smtp(SmtpConfig {
                host: var("RELAY_SMTP_HOST").ok_or_else(|| bad("RELAY_SMTP_HOST is required"))?,
                port: num::<u16>("RELAY_SMTP_PORT", var("RELAY_SMTP_PORT"))?.unwrap_or(587),
                username: var("RELAY_SMTP_USERNAME"),
                password: secret("RELAY_SMTP_PASSWORD")?,
                tls: match var("RELAY_SMTP_TLS").as_deref().unwrap_or("starttls") {
                    "starttls" => SmtpTls::StartTls,
                    "tls" => SmtpTls::Tls,
                    "none" => SmtpTls::None,
                    other => return Err(bad(format!("RELAY_SMTP_TLS: unknown value {other:?}"))),
                },
                from: var("RELAY_MAIL_FROM").ok_or_else(|| bad("RELAY_MAIL_FROM is required"))?,
            }),
            "log" => MailerConfig::Log,
            "disabled" => MailerConfig::Disabled,
            other => return Err(bad(format!("RELAY_MAILER: unknown value {other:?}"))),
        };

        if let Some(client_id) = var("RELAY_GOOGLE_CLIENT_ID") {
            cfg.google = Some(OidcConfig {
                issuer: var("RELAY_GOOGLE_ISSUER")
                    .unwrap_or_else(|| "https://accounts.google.com".into()),
                client_id,
                client_secret: Some(
                    secret("RELAY_GOOGLE_CLIENT_SECRET")?
                        .ok_or_else(|| bad("RELAY_GOOGLE_CLIENT_SECRET is required"))?,
                ),
                apple_key: None,
            });
        }
        if let Some(client_id) = var("RELAY_APPLE_CLIENT_ID") {
            cfg.apple = Some(OidcConfig {
                issuer: var("RELAY_APPLE_ISSUER")
                    .unwrap_or_else(|| "https://appleid.apple.com".into()),
                client_id,
                client_secret: None,
                apple_key: Some(AppleKeyConfig {
                    team_id: var("RELAY_APPLE_TEAM_ID")
                        .ok_or_else(|| bad("RELAY_APPLE_TEAM_ID is required"))?,
                    key_id: var("RELAY_APPLE_KEY_ID")
                        .ok_or_else(|| bad("RELAY_APPLE_KEY_ID is required"))?,
                    private_key_pem: secret("RELAY_APPLE_PRIVATE_KEY")?
                        .ok_or_else(|| bad("RELAY_APPLE_PRIVATE_KEY(_FILE) is required"))?,
                }),
            });
        }
        cfg.oauth_return_urls = list("RELAY_OAUTH_RETURN_URLS");

        if let Some(rp_id) = var("RELAY_WEBAUTHN_RP_ID") {
            let origins = list("RELAY_WEBAUTHN_ORIGINS")
                .iter()
                .map(|o| parse_url("RELAY_WEBAUTHN_ORIGINS", o))
                .collect::<Result<Vec<_>, _>>()?;
            if origins.is_empty() {
                return Err(bad(
                    "RELAY_WEBAUTHN_ORIGINS is required with RELAY_WEBAUTHN_RP_ID",
                ));
            }
            cfg.webauthn = Some(WebauthnConfig {
                rp_id,
                rp_name: var("RELAY_WEBAUTHN_RP_NAME").unwrap_or_else(|| "hame".into()),
                origins,
            });
        }

        if let Some(topic) = var("RELAY_APNS_TOPIC") {
            let sandbox = flag("RELAY_APNS_SANDBOX")?.unwrap_or(false);
            cfg.push.apns = Some(ApnsConfig {
                team_id: var("RELAY_APNS_TEAM_ID")
                    .ok_or_else(|| bad("RELAY_APNS_TEAM_ID is required"))?,
                key_id: var("RELAY_APNS_KEY_ID")
                    .ok_or_else(|| bad("RELAY_APNS_KEY_ID is required"))?,
                private_key_pem: secret("RELAY_APNS_PRIVATE_KEY")?
                    .ok_or_else(|| bad("RELAY_APNS_PRIVATE_KEY(_FILE) is required"))?,
                topic,
                endpoint: if sandbox {
                    "https://api.sandbox.push.apple.com".into()
                } else {
                    "https://api.push.apple.com".into()
                },
            });
        }
        if let Some(sa) = secret("RELAY_FCM_SERVICE_ACCOUNT")? {
            let v: serde_json::Value = serde_json::from_str(&sa)
                .map_err(|e| bad(format!("RELAY_FCM_SERVICE_ACCOUNT: not JSON: {e}")))?;
            let field = |name: &str| -> Result<String, ConfigError> {
                v.get(name)
                    .and_then(|x| x.as_str())
                    .map(str::to_string)
                    .ok_or_else(|| bad(format!("RELAY_FCM_SERVICE_ACCOUNT: missing {name}")))
            };
            cfg.push.fcm = Some(FcmConfig {
                project_id: field("project_id")?,
                client_email: field("client_email")?,
                private_key_pem: field("private_key")?,
                token_uri: field("token_uri")
                    .unwrap_or_else(|_| "https://oauth2.googleapis.com/token".into()),
                endpoint: "https://fcm.googleapis.com".into(),
            });
        }

        if let Some(n) = num::<u32>(
            "RELAY_RATE_AUTH_IP_PER_MIN",
            var("RELAY_RATE_AUTH_IP_PER_MIN"),
        )? {
            cfg.rate_limits.auth_per_ip_per_min = n;
        }
        if let Some(n) = num::<u32>(
            "RELAY_RATE_AUTH_ACCOUNT_PER_MIN",
            var("RELAY_RATE_AUTH_ACCOUNT_PER_MIN"),
        )? {
            cfg.rate_limits.auth_per_account_per_min = n;
        }
        if let Some(n) = num::<u32>(
            "RELAY_RATE_WS_FRAMES_PER_SEC",
            var("RELAY_RATE_WS_FRAMES_PER_SEC"),
        )? {
            cfg.rate_limits.ws_frames_per_sec = n;
        }
        if let Some(n) = num::<usize>("RELAY_ARGON2_CONCURRENCY", var("RELAY_ARGON2_CONCURRENCY"))?
        {
            cfg.argon2_concurrency = n.max(1);
        }
        if let Some(k) = secret("RELAY_ENUMERATION_KEY")? {
            let bytes = crate::util::b64url_decode(&k)
                .filter(|b| b.len() == 32)
                .ok_or_else(|| bad("RELAY_ENUMERATION_KEY: expected 32 bytes, base64url"))?;
            let mut key = [0u8; 32];
            key.copy_from_slice(&bytes);
            cfg.enumeration_key = Some(key);
        }
        Ok(cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn cfg(vars: &[(&str, &str)]) -> Result<Config, ConfigError> {
        let map: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Config::from_lookup(|k| map.get(k).cloned())
    }

    #[test]
    fn public_url_is_required() {
        assert!(cfg(&[]).is_err());
        let c = cfg(&[("RELAY_PUBLIC_URL", "https://relay.example.com")]).unwrap();
        assert!(c.hsts);
        assert!(c.google.is_none());
        assert_eq!(c.argon2.m_kib, 65536);
        assert_eq!(c.argon2.t, 3);
        assert_eq!(c.argon2.p, 1);
    }

    #[test]
    fn providers_and_lists_are_parsed() {
        let c = cfg(&[
            ("RELAY_PUBLIC_URL", "http://localhost:8080"),
            (
                "RELAY_CORS_ORIGINS",
                "https://app.example.com, https://x.example.com",
            ),
            ("RELAY_GOOGLE_CLIENT_ID", "gid"),
            ("RELAY_GOOGLE_CLIENT_SECRET", "gsecret"),
            ("RELAY_WEBAUTHN_RP_ID", "example.com"),
            ("RELAY_WEBAUTHN_ORIGINS", "https://app.example.com"),
        ])
        .unwrap();
        assert_eq!(c.cors_origins.len(), 2);
        assert!(!c.hsts);
        assert_eq!(c.google.unwrap().client_id, "gid");
        assert_eq!(c.webauthn.unwrap().rp_id, "example.com");
        assert!(
            cfg(&[
                ("RELAY_PUBLIC_URL", "http://localhost:8080"),
                ("RELAY_GOOGLE_CLIENT_ID", "gid"),
            ])
            .is_err()
        );
    }
}
