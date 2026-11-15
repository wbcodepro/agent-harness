//! Passkeys: registration and login with a software authenticator.

mod common;

use common::*;
use hame_relay::config::WebauthnConfig;
use reqwest::StatusCode;
use serde_json::{Value, json};
use webauthn_authenticator_rs::WebauthnAuthenticator;
use webauthn_authenticator_rs::prelude::{
    CreationChallengeResponse, RequestChallengeResponse, Url,
};
use webauthn_authenticator_rs::softpasskey::SoftPasskey;

const ORIGIN: &str = "http://localhost:5173";

async fn relay() -> TestRelay {
    spawn(|c| {
        c.webauthn = Some(WebauthnConfig {
            rp_id: "localhost".into(),
            rp_name: "hame test".into(),
            origins: vec![Url::parse(ORIGIN).unwrap()],
        });
    })
    .await
}

async fn register(r: &TestRelay, token: &str, auth: &mut SoftPasskey) -> Value {
    let (s, start) = r
        .post("/v1/auth/passkey/register/start", json!({}), Some(token))
        .await;
    assert_eq!(s, StatusCode::OK, "{start}");
    let options: CreationChallengeResponse =
        serde_json::from_value(start["options"].clone()).unwrap();
    let credential = auth
        .do_registration(Url::parse(ORIGIN).unwrap(), options)
        .unwrap();
    let (s, v) = r
        .post(
            "/v1/auth/passkey/register/finish",
            json!({
                "challenge_id": start["challenge_id"],
                "credential": credential,
                "name": "Test key",
            }),
            Some(token),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    v
}

async fn login_start(r: &TestRelay, email: Option<&str>) -> Value {
    let body = match email {
        Some(e) => json!({ "email": e }),
        None => json!({}),
    };
    let (s, v) = r.post("/v1/auth/passkey/login/start", body, None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    v
}

#[tokio::test]
async fn register_and_sign_in_with_a_passkey() {
    let r = relay().await;
    let login = r.account("Ray", "ray@example.com").await;
    let mut auth = SoftPasskey::new(true);
    let added = register(&r, &access(&login), &mut auth).await;
    assert_eq!(added["name"], "Test key");

    let (_, list) = r.get("/v1/account/passkeys", Some(&access(&login))).await;
    assert_eq!(list["passkeys"].as_array().unwrap().len(), 1);

    let start = login_start(&r, Some("ray@example.com")).await;
    let allow = start["options"]["publicKey"]["allowCredentials"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(allow.len(), 1);
    let options: RequestChallengeResponse =
        serde_json::from_value(start["options"].clone()).unwrap();
    let assertion = auth
        .do_authentication(Url::parse(ORIGIN).unwrap(), options)
        .unwrap();
    let (s, tokens) = r
        .post(
            "/v1/auth/passkey/login/finish",
            json!({ "challenge_id": start["challenge_id"], "credential": assertion }),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{tokens}");
    assert_eq!(tokens["account_id"], login["account_id"]);
    assert_eq!(
        r.get("/v1/account", Some(&access(&tokens))).await.0,
        StatusCode::OK
    );

    // The challenge cannot be replayed.
    let (s, v) = r
        .post(
            "/v1/auth/passkey/login/finish",
            json!({ "challenge_id": start["challenge_id"], "credential": assertion }),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(v["error"], "invalid_challenge");

    // An assertion for one challenge does not satisfy another.
    let other = login_start(&r, Some("ray@example.com")).await;
    let (s, _) = r
        .post(
            "/v1/auth/passkey/login/finish",
            json!({ "challenge_id": other["challenge_id"], "credential": assertion }),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    // A different authenticator is not accepted.
    let start = login_start(&r, Some("ray@example.com")).await;
    let options: RequestChallengeResponse =
        serde_json::from_value(start["options"].clone()).unwrap();
    let mut stranger = SoftPasskey::new(true);
    assert!(
        stranger
            .do_authentication(Url::parse(ORIGIN).unwrap(), options)
            .is_err(),
        "the software authenticator has no matching credential"
    );

    // Removing the passkey.
    let id = added["id"].as_str().unwrap();
    assert_eq!(
        r.delete(&format!("/v1/account/passkeys/{id}"), Some(&access(&login)))
            .await,
        StatusCode::NO_CONTENT
    );
    let (_, list) = r.get("/v1/account/passkeys", Some(&access(&login))).await;
    assert!(list["passkeys"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn unknown_emails_get_a_stable_fake_challenge() {
    let r = relay().await;
    let a = login_start(&r, Some("nobody@example.com")).await;
    let b = login_start(&r, Some("nobody@example.com")).await;
    let ids = |v: &Value| v["options"]["publicKey"]["allowCredentials"].clone();
    assert_eq!(ids(&a).as_array().unwrap().len(), 1);
    assert_eq!(ids(&a), ids(&b));
    assert_ne!(
        a["options"]["publicKey"]["challenge"],
        b["options"]["publicKey"]["challenge"]
    );
    let c = login_start(&r, Some("somebody@example.com")).await;
    assert_ne!(ids(&a), ids(&c));

    // Without an email: a discoverable-credential challenge.
    let d = login_start(&r, None).await;
    assert_eq!(d["options"]["mediation"], "conditional");
    assert!(
        d["options"]["publicKey"]["allowCredentials"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn registration_requires_a_session_and_its_own_challenge() {
    let r = relay().await;
    let (s, _) = r
        .post("/v1/auth/passkey/register/start", json!({}), None)
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    // A challenge started by one account cannot be finished by another.
    let one = r.account("Sam", "sam@example.com").await;
    let two = r.account("Tea", "tea@example.com").await;
    let (_, start) = r
        .post(
            "/v1/auth/passkey/register/start",
            json!({}),
            Some(&access(&one)),
        )
        .await;
    let options: CreationChallengeResponse =
        serde_json::from_value(start["options"].clone()).unwrap();
    let credential = SoftPasskey::new(true)
        .do_registration(Url::parse(ORIGIN).unwrap(), options)
        .unwrap();
    let (s, v) = r
        .post(
            "/v1/auth/passkey/register/finish",
            json!({ "challenge_id": start["challenge_id"], "credential": credential }),
            Some(&access(&two)),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(v["error"], "invalid_challenge");
}

#[tokio::test]
async fn passkeys_are_not_configured_by_default() {
    let r = spawn(|_| {}).await;
    let (s, v) = r
        .post("/v1/auth/passkey/login/start", json!({}), None)
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(v["error"], "not_configured");
}
