//! Sign-in with Google and Apple against a local mock identity provider.

mod common;

use common::idp::{ClientAuth, Grant, MockIdp, ec_key_pair};
use common::*;
use hame_relay::config::{AppleKeyConfig, OidcConfig};
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::collections::HashMap;

const GOOGLE_ID: &str = "google-client-id";
const GOOGLE_SECRET: &str = "google-client-secret";
const APPLE_ID: &str = "com.example.hame.signin";

struct Setup {
    r: TestRelay,
    google: MockIdp,
    apple: MockIdp,
}

async fn setup() -> Setup {
    let google = MockIdp::start(GOOGLE_ID, ClientAuth::Secret(GOOGLE_SECRET.into())).await;
    let (apple_private, apple_public) = ec_key_pair();
    let apple = MockIdp::start(
        APPLE_ID,
        ClientAuth::AppleJwt {
            public_pem: apple_public,
            team_id: "TEAM123456".into(),
            key_id: "KEY1234567".into(),
        },
    )
    .await;
    let (gi, ai) = (google.issuer(), apple.issuer());
    let r = spawn(move |c| {
        c.google = Some(OidcConfig {
            issuer: gi,
            client_id: GOOGLE_ID.into(),
            client_secret: Some(GOOGLE_SECRET.into()),
            apple_key: None,
        });
        c.apple = Some(OidcConfig {
            issuer: ai,
            client_id: APPLE_ID.into(),
            client_secret: None,
            apple_key: Some(AppleKeyConfig {
                team_id: "TEAM123456".into(),
                key_id: "KEY1234567".into(),
                private_key_pem: apple_private,
            }),
        });
        c.oauth_return_urls = vec!["hame://oauth".into()];
    })
    .await;
    Setup { r, google, apple }
}

/// The redirect from `start`: its query parameters and the binding cookie.
struct Started {
    params: HashMap<String, String>,
    cookie: String,
}

async fn start(r: &TestRelay, provider: &str, query: &str) -> Started {
    let res = r
        .http
        .get(r.url(&format!("/v1/auth/oauth/{provider}/start{query}")))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    let location = res.headers()["location"].to_str().unwrap().to_string();
    let cookie = res.headers()["set-cookie"].to_str().unwrap().to_string();
    assert!(cookie.contains("HttpOnly"));
    let cookie = cookie.split(';').next().unwrap().to_string();
    let url = url::Url::parse(&location).unwrap();
    let params: HashMap<String, String> = url.query_pairs().into_owned().collect();
    Started { params, cookie }
}

async fn callback(r: &TestRelay, provider: &str, code: &str, s: &Started) -> (StatusCode, Value) {
    let res = r
        .http
        .get(r.url(&format!("/v1/auth/oauth/{provider}/callback")))
        .query(&[("code", code), ("state", s.params["state"].as_str())])
        .header("cookie", &s.cookie)
        .send()
        .await
        .unwrap();
    let status = res.status();
    (status, res.json().await.unwrap_or(Value::Null))
}

fn grant(idp: &MockIdp, s: &Started, claims: Value) -> Grant {
    let _ = idp;
    Grant {
        claims,
        code_challenge: s.params["code_challenge"].clone(),
        rogue_signature: false,
    }
}

#[tokio::test]
async fn google_sign_in_creates_and_reuses_an_account() {
    let Setup { r, google, .. } = setup().await;
    let s = start(&r, "google", "").await;
    assert_eq!(s.params["response_type"], "code");
    assert_eq!(s.params["client_id"], GOOGLE_ID);
    assert_eq!(s.params["code_challenge_method"], "S256");
    assert_eq!(s.params["scope"], "openid email profile");
    assert_eq!(
        s.params["redirect_uri"],
        r.url("/v1/auth/oauth/google/callback")
    );
    let mut claims = google.claims("g-sub-1", "Nia@Example.com", &s.params["nonce"]);
    claims["name"] = json!("Nia N");
    google.grant("code-1", grant(&google, &s, claims));
    let (status, tokens) = callback(&r, "google", "code-1", &s).await;
    assert_eq!(status, StatusCode::OK, "{tokens}");
    let at = access(&tokens);
    let (_, me) = r.get("/v1/account", Some(&at)).await;
    assert_eq!(me["email"], "nia@example.com");
    assert_eq!(me["name"], "Nia N");
    assert_eq!(me["has_password"], false);
    assert_eq!(me["identities"][0]["provider"], "google");

    // The token request carried the PKCE verifier and the client secret.
    let req = &google.token_requests()[0];
    assert_eq!(req["client_secret"], GOOGLE_SECRET);
    assert!(req["code_verifier"].len() >= 43);

    // Same subject again: same account.
    let s2 = start(&r, "google", "").await;
    google.grant(
        "code-2",
        grant(
            &google,
            &s2,
            google.claims("g-sub-1", "nia@example.com", &s2.params["nonce"]),
        ),
    );
    let (status, again) = callback(&r, "google", "code-2", &s2).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(again["account_id"], tokens["account_id"]);

    // A state is single use.
    let (status, v) = callback(&r, "google", "code-2", &s2).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(v["error"], "invalid_state");
}

type Mutation = dyn Fn(&mut Value, &mut Grant);

#[tokio::test]
async fn id_tokens_are_verified() {
    let Setup { r, google, .. } = setup().await;
    let cases: Vec<(&str, Box<Mutation>)> = vec![
        ("bad signature", Box::new(|_, g| g.rogue_signature = true)),
        (
            "wrong nonce",
            Box::new(|c, _| c["nonce"] = json!("another-nonce")),
        ),
        (
            "wrong audience",
            Box::new(|c, _| c["aud"] = json!("someone-else")),
        ),
        (
            "wrong issuer",
            Box::new(|c, _| c["iss"] = json!("https://evil.example")),
        ),
        (
            "expired",
            Box::new(|c, _| {
                let past = hame_relay::util::now() - 3600;
                c["iat"] = json!(past - 600);
                c["exp"] = json!(past);
            }),
        ),
    ];
    for (i, (name, mutate)) in cases.iter().enumerate() {
        let s = start(&r, "google", "").await;
        let mut claims = google.claims("g-sub-x", "x@example.com", &s.params["nonce"]);
        let mut g = grant(&google, &s, json!(null));
        mutate(&mut claims, &mut g);
        g.claims = claims;
        let code = format!("bad-{i}");
        google.grant(&code, g);
        let (status, v) = callback(&r, "google", &code, &s).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{name}: {v}");
        assert_eq!(v["error"], "invalid_id_token", "{name}");
    }
    // No account was created by any of them.
    assert!(
        r.state
            .repo
            .account_by_email("x@example.com")
            .await
            .unwrap()
            .is_none()
    );

    // An unverified email is not accepted for a new identity.
    let s = start(&r, "google", "").await;
    let mut claims = google.claims("g-sub-y", "y@example.com", &s.params["nonce"]);
    claims["email_verified"] = json!(false);
    google.grant("unverified", grant(&google, &s, claims));
    let (status, v) = callback(&r, "google", "unverified", &s).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(v["error"], "email_not_verified");
}

#[tokio::test]
async fn the_flow_is_bound_to_the_browser_and_pkce() {
    let Setup { r, google, .. } = setup().await;
    // Without the cookie from start (login CSRF).
    let s = start(&r, "google", "").await;
    google.grant(
        "c",
        grant(
            &google,
            &s,
            google.claims("g", "z@example.com", &s.params["nonce"]),
        ),
    );
    let res = r
        .http
        .get(r.url("/v1/auth/oauth/google/callback"))
        .query(&[("code", "c"), ("state", s.params["state"].as_str())])
        .header("cookie", "hame_oauth=forged")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    // A code issued for a different PKCE challenge is refused by the
    // provider (the relay sends its own verifier).
    let s = start(&r, "google", "").await;
    let mut g = grant(
        &google,
        &s,
        google.claims("g", "z@example.com", &s.params["nonce"]),
    );
    g.code_challenge = "not-the-challenge".into();
    google.grant("c2", g);
    let (status, v) = callback(&r, "google", "c2", &s).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(v["error"], "provider_error");

    // Unknown provider and disallowed return_to.
    let res = r
        .http
        .get(r.url("/v1/auth/oauth/myspace/start"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    let res = r
        .http
        .get(r.url("/v1/auth/oauth/google/start?return_to=https://evil.example/"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn links_by_verified_email_and_claims_unverified_accounts() {
    let Setup { r, google, .. } = setup().await;
    // A verified password account links to the Google identity.
    let pw = r.account("Oli", "oli@example.com").await;
    let s = start(&r, "google", "").await;
    google.grant(
        "l1",
        grant(
            &google,
            &s,
            google.claims("g-oli", "oli@example.com", &s.params["nonce"]),
        ),
    );
    let (status, t) = callback(&r, "google", "l1", &s).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(t["account_id"], pw["account_id"]);
    r.login("oli@example.com", PASSWORD).await;

    // An account someone registered but never verified: the provider's
    // verified owner takes it over and the squatter's password is gone.
    let (st, _) = r
        .post(
            "/v1/auth/signup",
            json!({ "name": "Squatter", "email": "pat@example.com", "password": PASSWORD }),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::ACCEPTED);
    let s = start(&r, "google", "").await;
    google.grant(
        "l2",
        grant(
            &google,
            &s,
            google.claims("g-pat", "pat@example.com", &s.params["nonce"]),
        ),
    );
    let (status, t) = callback(&r, "google", "l2", &s).await;
    assert_eq!(status, StatusCode::OK);
    let (_, me) = r.get("/v1/account", Some(&access(&t))).await;
    assert_eq!(me["email_verified"], true);
    assert_eq!(me["has_password"], false);
    let (st, _) = r
        .post(
            "/v1/auth/login",
            json!({ "email": "pat@example.com", "password": PASSWORD }),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn apple_form_post_with_client_secret_jwt() {
    let Setup { r, apple, .. } = setup().await;
    let s = start(&r, "apple", "").await;
    assert_eq!(s.params["response_mode"], "form_post");
    let mut claims = apple.claims(
        "apple-sub-1",
        "relay@privaterelay.appleid.com",
        &s.params["nonce"],
    );
    // Apple sends email_verified as a string.
    claims["email_verified"] = json!("true");
    apple.grant("a1", grant(&apple, &s, claims));
    let user =
        json!({ "name": { "firstName": "Quinn", "lastName": "Q" }, "email": "x" }).to_string();
    let res = r
        .http
        .post(r.url("/v1/auth/oauth/apple/callback"))
        .header("cookie", &s.cookie)
        .form(&[
            ("code", "a1"),
            ("state", s.params["state"].as_str()),
            ("user", user.as_str()),
        ])
        .send()
        .await
        .unwrap();
    let status = res.status();
    let tokens: Value = res.json().await.unwrap();
    assert_eq!(status, StatusCode::OK, "{tokens}");
    let (_, me) = r.get("/v1/account", Some(&access(&tokens))).await;
    assert_eq!(me["name"], "Quinn Q");
    assert_eq!(me["identities"][0]["provider"], "apple");
    // The mock verified the ES256 client secret (it rejects otherwise).
    assert_eq!(apple.token_requests().len(), 1);
}

#[tokio::test]
async fn return_to_hands_over_a_one_time_code() {
    let Setup { r, google, .. } = setup().await;
    let verifier = "app-verifier-0123456789-0123456789-0123456789";
    let challenge = hame_relay::util::b64url(&hame_relay::util::sha256(verifier.as_bytes()));
    let s = start(
        &r,
        "google",
        &format!("?return_to=hame://oauth&code_challenge={challenge}"),
    )
    .await;
    google.grant(
        "r1",
        grant(
            &google,
            &s,
            google.claims("g-r", "rae@example.com", &s.params["nonce"]),
        ),
    );
    let res = r
        .http
        .get(r.url("/v1/auth/oauth/google/callback"))
        .query(&[("code", "r1"), ("state", s.params["state"].as_str())])
        .header("cookie", &s.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    let location = res.headers()["location"].to_str().unwrap().to_string();
    assert!(location.starts_with("hame://oauth#code="), "{location}");
    let code = location.split("code=").nth(1).unwrap().to_string();

    // Without the app's verifier: refused, and the code is spent.
    let (st, _) = r
        .post("/v1/auth/oauth/exchange", json!({ "code": code }), None)
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // Again with a fresh flow and the verifier.
    let s = start(
        &r,
        "google",
        &format!("?return_to=hame://oauth&code_challenge={challenge}"),
    )
    .await;
    google.grant(
        "r2",
        grant(
            &google,
            &s,
            google.claims("g-r", "rae@example.com", &s.params["nonce"]),
        ),
    );
    let res = r
        .http
        .get(r.url("/v1/auth/oauth/google/callback"))
        .query(&[("code", "r2"), ("state", s.params["state"].as_str())])
        .header("cookie", &s.cookie)
        .send()
        .await
        .unwrap();
    let location = res.headers()["location"].to_str().unwrap().to_string();
    let code = location.split("code=").nth(1).unwrap().to_string();
    let (st, t) = r
        .post(
            "/v1/auth/oauth/exchange",
            json!({ "code": code, "code_verifier": verifier }),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{t}");
    assert!(t["access_token"].as_str().unwrap().starts_with("hra_"));
    let (st, _) = r
        .post(
            "/v1/auth/oauth/exchange",
            json!({ "code": code, "code_verifier": verifier }),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "codes are single use");
}
