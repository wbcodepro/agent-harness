//! OpenID Connect client for Google and Apple: discovery, the token
//! exchange (with PKCE), Apple's client-secret JWT and ID-token
//! verification against the provider's cached JWKS.

use crate::config::OidcConfig;
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::Deserialize;
use serde_json::json;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Which provider.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderKind {
    Google,
    Apple,
}

impl ProviderKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "google" => Some(Self::Google),
            "apple" => Some(Self::Apple),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Google => "google",
            Self::Apple => "apple",
        }
    }
}

/// An OIDC failure. Messages are safe to show to the client.
#[derive(Debug, thiserror::Error)]
pub enum OidcError {
    #[error("identity provider unavailable: {0}")]
    Provider(String),
    #[error("invalid ID token: {0}")]
    InvalidToken(String),
}

#[derive(Clone, Debug, Deserialize)]
pub struct Discovery {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
}

/// The claims the relay uses from a verified ID token.
#[derive(Clone, Debug, Deserialize)]
pub struct IdClaims {
    pub sub: String,
    #[serde(default)]
    pub email: Option<String>,
    /// A boolean (Google) or the string "true"/"false" (Apple).
    #[serde(default)]
    pub email_verified: Option<serde_json::Value>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub nonce: Option<String>,
}

impl IdClaims {
    pub fn email_is_verified(&self) -> bool {
        match &self.email_verified {
            Some(serde_json::Value::Bool(b)) => *b,
            Some(serde_json::Value::String(s)) => s == "true",
            _ => false,
        }
    }
}

struct JwksCache {
    set: JwkSet,
    fetched: Instant,
}

const DISCOVERY_TTL: Duration = Duration::from_secs(24 * 3600);
const JWKS_TTL: Duration = Duration::from_secs(3600);
/// An unknown `kid` triggers a refetch at most this often.
const JWKS_MIN_REFETCH: Duration = Duration::from_secs(30);

/// One configured provider.
pub struct OidcProvider {
    pub kind: ProviderKind,
    cfg: OidcConfig,
    apple_key: Option<EncodingKey>,
    http: reqwest::Client,
    discovery: RwLock<Option<(Discovery, Instant)>>,
    jwks: RwLock<Option<JwksCache>>,
}

impl OidcProvider {
    pub fn new(kind: ProviderKind, cfg: OidcConfig, http: reqwest::Client) -> Result<Self, String> {
        let apple_key = match (&kind, &cfg.apple_key) {
            (ProviderKind::Apple, Some(k)) => Some(
                EncodingKey::from_ec_pem(k.private_key_pem.as_bytes())
                    .map_err(|e| format!("Apple private key: {e}"))?,
            ),
            (ProviderKind::Apple, None) => return Err("Apple needs a private key".into()),
            _ => None,
        };
        if kind == ProviderKind::Google && cfg.client_secret.is_none() {
            return Err("Google needs a client secret".into());
        }
        Ok(Self {
            kind,
            cfg,
            apple_key,
            http,
            discovery: RwLock::new(None),
            jwks: RwLock::new(None),
        })
    }

    pub fn client_id(&self) -> &str {
        &self.cfg.client_id
    }

    /// The provider's discovery document (cached for a day).
    pub async fn discovery(&self) -> Result<Discovery, OidcError> {
        if let Some((d, at)) = self.discovery.read().await.as_ref()
            && at.elapsed() < DISCOVERY_TTL
        {
            return Ok(d.clone());
        }
        let url = format!(
            "{}/.well-known/openid-configuration",
            self.cfg.issuer.trim_end_matches('/')
        );
        let d: Discovery = self
            .http
            .get(&url)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| OidcError::Provider(e.to_string()))?
            .json()
            .await
            .map_err(|e| OidcError::Provider(e.to_string()))?;
        if d.issuer.trim_end_matches('/') != self.cfg.issuer.trim_end_matches('/') {
            return Err(OidcError::Provider(
                "discovery issuer does not match the configured issuer".into(),
            ));
        }
        *self.discovery.write().await = Some((d.clone(), Instant::now()));
        Ok(d)
    }

    /// The URL that starts the authorization-code flow.
    pub async fn authorization_url(
        &self,
        redirect_uri: &str,
        state: &str,
        nonce: &str,
        code_challenge: &str,
    ) -> Result<String, OidcError> {
        let d = self.discovery().await?;
        let mut url = url::Url::parse(&d.authorization_endpoint)
            .map_err(|e| OidcError::Provider(e.to_string()))?;
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("response_type", "code")
                .append_pair("client_id", &self.cfg.client_id)
                .append_pair("redirect_uri", redirect_uri)
                .append_pair("state", state)
                .append_pair("nonce", nonce)
                .append_pair("code_challenge", code_challenge)
                .append_pair("code_challenge_method", "S256");
            match self.kind {
                ProviderKind::Google => {
                    q.append_pair("scope", "openid email profile");
                }
                ProviderKind::Apple => {
                    // Apple posts the result back when name or email is
                    // requested.
                    q.append_pair("scope", "name email")
                        .append_pair("response_mode", "form_post");
                }
            }
        }
        Ok(url.to_string())
    }

    /// Apple's client secret: an ES256 JWT signed with the configured key.
    pub fn apple_client_secret(&self, now: i64) -> Result<String, OidcError> {
        let (Some(key), Some(k)) = (&self.apple_key, &self.cfg.apple_key) else {
            return Err(OidcError::Provider("Apple key not configured".into()));
        };
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(k.key_id.clone());
        let claims = json!({
            "iss": k.team_id,
            "iat": now,
            "exp": now + 300,
            "aud": self.cfg.issuer.trim_end_matches('/'),
            "sub": self.cfg.client_id,
        });
        jsonwebtoken::encode(&header, &claims, key).map_err(|e| OidcError::Provider(e.to_string()))
    }

    /// Exchanges the authorization code (with the PKCE verifier) for an ID
    /// token, and verifies it.
    pub async fn exchange_and_verify(
        &self,
        code: &str,
        redirect_uri: &str,
        code_verifier: &str,
        nonce: &str,
    ) -> Result<IdClaims, OidcError> {
        let d = self.discovery().await?;
        let secret = match self.kind {
            ProviderKind::Google => self.cfg.client_secret.clone().unwrap_or_default(),
            ProviderKind::Apple => self.apple_client_secret(crate::util::now())?,
        };
        let res = self
            .http
            .post(&d.token_endpoint)
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", redirect_uri),
                ("client_id", self.cfg.client_id.as_str()),
                ("client_secret", secret.as_str()),
                ("code_verifier", code_verifier),
            ])
            .send()
            .await
            .map_err(|e| OidcError::Provider(e.to_string()))?;
        if !res.status().is_success() {
            return Err(OidcError::Provider(format!(
                "token endpoint returned {}",
                res.status()
            )));
        }
        #[derive(Deserialize)]
        struct TokenResponse {
            id_token: String,
        }
        let t: TokenResponse = res
            .json()
            .await
            .map_err(|e| OidcError::Provider(e.to_string()))?;
        self.verify_id_token(&t.id_token, nonce, &d).await
    }

    async fn jwks(&self, d: &Discovery, force: bool) -> Result<JwkSet, OidcError> {
        if let Some(c) = self.jwks.read().await.as_ref() {
            let fresh = c.fetched.elapsed() < JWKS_TTL;
            let recently = c.fetched.elapsed() < JWKS_MIN_REFETCH;
            if (fresh && !force) || recently {
                return Ok(c.set.clone());
            }
        }
        let set: JwkSet = self
            .http
            .get(&d.jwks_uri)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| OidcError::Provider(e.to_string()))?
            .json()
            .await
            .map_err(|e| OidcError::Provider(e.to_string()))?;
        *self.jwks.write().await = Some(JwksCache {
            set: set.clone(),
            fetched: Instant::now(),
        });
        Ok(set)
    }

    /// Verifies signature (against the JWKS), `iss`, `aud`, `exp` and
    /// `nonce`.
    pub async fn verify_id_token(
        &self,
        token: &str,
        nonce: &str,
        d: &Discovery,
    ) -> Result<IdClaims, OidcError> {
        let bad = |m: &str| OidcError::InvalidToken(m.to_string());
        let header = jsonwebtoken::decode_header(token).map_err(|_| bad("malformed"))?;
        if !matches!(header.alg, Algorithm::RS256 | Algorithm::ES256) {
            return Err(bad("unsupported algorithm"));
        }
        let kid = header.kid.ok_or_else(|| bad("no key id"))?;
        let mut set = self.jwks(d, false).await?;
        if set.find(&kid).is_none() {
            set = self.jwks(d, true).await?;
        }
        let jwk = set.find(&kid).ok_or_else(|| bad("unknown key id"))?;
        let key = DecodingKey::from_jwk(jwk).map_err(|_| bad("unusable key"))?;

        let mut v = Validation::new(header.alg);
        v.leeway = 60;
        v.set_audience(&[self.cfg.client_id.as_str()]);
        let issuer = d.issuer.trim_end_matches('/').to_string();
        let mut issuers = vec![issuer.clone()];
        if self.kind == ProviderKind::Google
            && let Some(host) = issuer.strip_prefix("https://")
        {
            // Google also issues tokens with the bare host as issuer.
            issuers.push(host.to_string());
        }
        v.set_issuer(&issuers);
        v.set_required_spec_claims(&["exp", "iat", "iss", "aud", "sub"]);
        let data = jsonwebtoken::decode::<IdClaims>(token, &key, &v).map_err(|e| {
            use jsonwebtoken::errors::ErrorKind;
            match e.kind() {
                ErrorKind::InvalidSignature => bad("bad signature"),
                ErrorKind::ExpiredSignature => bad("expired"),
                ErrorKind::InvalidAudience => bad("wrong audience"),
                ErrorKind::InvalidIssuer => bad("wrong issuer"),
                _ => bad("rejected"),
            }
        })?;
        let claims = data.claims;
        match &claims.nonce {
            Some(n) if crate::util::ct_eq(n.as_bytes(), nonce.as_bytes()) => {}
            _ => return Err(bad("nonce mismatch")),
        }
        Ok(claims)
    }
}
