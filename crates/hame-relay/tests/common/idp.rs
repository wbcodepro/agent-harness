//! A local mock OpenID Connect provider: discovery, JWKS and token
//! endpoints, RS256 ID tokens. Tests register what the token endpoint
//! returns for an authorization code.

use axum::extract::{Form, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub const KID: &str = "test-key-1";

/// What the token endpoint answers for one code.
#[derive(Clone)]
pub struct Grant {
    pub claims: Value,
    /// The PKCE challenge the relay sent to the authorization endpoint.
    pub code_challenge: String,
    /// Sign with a key that is not in the JWKS (same `kid`).
    pub rogue_signature: bool,
}

/// How the mock checks the client secret.
#[derive(Clone)]
pub enum ClientAuth {
    Secret(String),
    /// Apple: an ES256 JWT verified with this public key.
    AppleJwt {
        public_pem: String,
        team_id: String,
        key_id: String,
    },
}

struct Inner {
    issuer: String,
    client_id: String,
    auth: ClientAuth,
    signing: EncodingKey,
    rogue: EncodingKey,
    n: String,
    e: String,
    grants: Mutex<HashMap<String, Grant>>,
    token_requests: Mutex<Vec<HashMap<String, String>>>,
}

#[derive(Clone)]
pub struct MockIdp(Arc<Inner>);

fn rsa_key() -> (EncodingKey, String, String) {
    let rsa = openssl::rsa::Rsa::generate(2048).unwrap();
    let pem = rsa.private_key_to_pem().unwrap();
    let n = hame_relay::util::b64url(&rsa.n().to_vec());
    let e = hame_relay::util::b64url(&rsa.e().to_vec());
    (EncodingKey::from_rsa_pem(&pem).unwrap(), n, e)
}

/// A P-256 key for Apple's client secret: `(private PKCS#8 PEM, public PEM)`.
pub fn ec_key_pair() -> (String, String) {
    let group = openssl::ec::EcGroup::from_curve_name(openssl::nid::Nid::X9_62_PRIME256V1).unwrap();
    let ec = openssl::ec::EcKey::generate(&group).unwrap();
    let pkey = openssl::pkey::PKey::from_ec_key(ec).unwrap();
    let private = String::from_utf8(pkey.private_key_to_pem_pkcs8().unwrap()).unwrap();
    let public = String::from_utf8(pkey.public_key_to_pem().unwrap()).unwrap();
    (private, public)
}

impl MockIdp {
    pub async fn start(client_id: &str, auth: ClientAuth) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let issuer = format!("http://{}", listener.local_addr().unwrap());
        let (signing, n, e) = rsa_key();
        let (rogue, _, _) = rsa_key();
        let idp = MockIdp(Arc::new(Inner {
            issuer,
            client_id: client_id.to_string(),
            auth,
            signing,
            rogue,
            n,
            e,
            grants: Mutex::new(HashMap::new()),
            token_requests: Mutex::new(Vec::new()),
        }));
        let app = Router::new()
            .route("/.well-known/openid-configuration", get(discovery))
            .route("/jwks", get(jwks))
            .route("/token", post(token))
            .with_state(idp.clone());
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        idp
    }

    pub fn issuer(&self) -> String {
        self.0.issuer.clone()
    }

    /// Registers what `/token` returns for `code`.
    pub fn grant(&self, code: &str, grant: Grant) {
        self.0
            .grants
            .lock()
            .unwrap()
            .insert(code.to_string(), grant);
    }

    pub fn token_requests(&self) -> Vec<HashMap<String, String>> {
        self.0.token_requests.lock().unwrap().clone()
    }

    /// Standard valid claims.
    pub fn claims(&self, sub: &str, email: &str, nonce: &str) -> Value {
        let now = hame_relay::util::now();
        json!({
            "iss": self.0.issuer,
            "aud": self.0.client_id,
            "sub": sub,
            "email": email,
            "email_verified": true,
            "nonce": nonce,
            "iat": now,
            "exp": now + 600,
        })
    }
}

async fn discovery(State(idp): State<MockIdp>) -> Json<Value> {
    let i = &idp.0.issuer;
    Json(json!({
        "issuer": i,
        "authorization_endpoint": format!("{i}/authorize"),
        "token_endpoint": format!("{i}/token"),
        "jwks_uri": format!("{i}/jwks"),
    }))
}

async fn jwks(State(idp): State<MockIdp>) -> Json<Value> {
    Json(json!({
        "keys": [{
            "kty": "RSA",
            "kid": KID,
            "use": "sig",
            "alg": "RS256",
            "n": idp.0.n,
            "e": idp.0.e,
        }]
    }))
}

fn reject(msg: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "error": "invalid_grant", "error_description": msg })),
    )
        .into_response()
}

async fn token(State(idp): State<MockIdp>, Form(form): Form<HashMap<String, String>>) -> Response {
    idp.0.token_requests.lock().unwrap().push(form.clone());
    let inner = &idp.0;
    let field = |k: &str| form.get(k).cloned().unwrap_or_default();
    if field("grant_type") != "authorization_code" {
        return reject("grant_type");
    }
    if field("client_id") != inner.client_id {
        return reject("client_id");
    }
    match &inner.auth {
        ClientAuth::Secret(s) => {
            if &field("client_secret") != s {
                return reject("client_secret");
            }
        }
        ClientAuth::AppleJwt {
            public_pem,
            team_id,
            key_id,
        } => {
            let secret = field("client_secret");
            let Ok(header) = jsonwebtoken::decode_header(&secret) else {
                return reject("client_secret header");
            };
            if header.alg != Algorithm::ES256 || header.kid.as_deref() != Some(key_id) {
                return reject("client_secret alg/kid");
            }
            let mut v = Validation::new(Algorithm::ES256);
            v.set_audience(&[inner.issuer.as_str()]);
            v.set_issuer(&[team_id.as_str()]);
            v.sub = Some(inner.client_id.clone());
            let key = DecodingKey::from_ec_pem(public_pem.as_bytes()).unwrap();
            if jsonwebtoken::decode::<Value>(&secret, &key, &v).is_err() {
                return reject("client_secret signature");
            }
        }
    }
    let Some(grant) = inner.grants.lock().unwrap().remove(&field("code")) else {
        return reject("unknown code");
    };
    let verifier = field("code_verifier");
    let challenge = hame_relay::util::b64url(&hame_relay::util::sha256(verifier.as_bytes()));
    if challenge != grant.code_challenge {
        return reject("PKCE verifier");
    }
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(KID.into());
    let key = if grant.rogue_signature {
        &inner.rogue
    } else {
        &inner.signing
    };
    let id_token = jsonwebtoken::encode(&header, &grant.claims, key).unwrap();
    Json(json!({ "access_token": "mock", "token_type": "Bearer", "id_token": id_token }))
        .into_response()
}
