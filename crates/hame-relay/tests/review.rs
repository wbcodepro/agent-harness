//! Regression tests for the security review of the relay (H1, M1–M5,
//! L1, L2 and the further findings). The first four started as the
//! reviewer's proofs of concept.

mod common;

use common::idp::{ClientAuth, Grant, MockIdp};
use common::*;
use hame_relay::config::{OidcConfig, WebauthnConfig};
use hame_relay::util::b64url;
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::time::Duration;
use webauthn_authenticator_rs::WebauthnAuthenticator;
use webauthn_authenticator_rs::prelude::{CreationChallengeResponse, Url};
use webauthn_authenticator_rs::softpasskey::SoftPasskey;

const GID: &str = "google-client-id";
const ORIGIN: &str = "http://localhost:5173";

async fn with_google(extra: impl FnOnce(&mut hame_relay::Config) + Send) -> (TestRelay, MockIdp) {
    let google = MockIdp::start(GID, ClientAuth::Secret("s".into())).await;
    let gi = google.issuer();
    let r = spawn(move |c| {
        c.google = Some(OidcConfig {
            issuer: gi,
            client_id: GID.into(),
            client_secret: Some("s".into()),
            apple_key: None,
        });
        c.oauth_return_urls = vec![
            "hame://oauth".into(),
            "https://app.example.test/oauth".into(),
        ];
        extra(c);
    })
    .await;
    (r, google)
}

/// A complete Google sign-in for `email`; returns the tokens.
async fn google_sign_in(r: &TestRelay, google: &MockIdp, sub: &str, email: &str) -> Value {
    let res = r
        .http
        .get(r.url("/v1/auth/oauth/google/start"))
        .send()
        .await
        .unwrap();
    let loc = url::Url::parse(res.headers()["location"].to_str().unwrap()).unwrap();
    let cookie = res.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let p: HashMap<String, String> = loc.query_pairs().into_owned().collect();
    let code = format!(
        "code-{sub}-{}",
        b64url(&hame_relay::util::random_bytes::<6>())
    );
    google.grant(
        &code,
        Grant {
            claims: google.claims(sub, email, &p["nonce"]),
            code_challenge: p["code_challenge"].clone(),
            rogue_signature: false,
        },
    );
    let res = r
        .http
        .get(r.url("/v1/auth/oauth/google/callback"))
        .query(&[("code", code.as_str()), ("state", p["state"].as_str())])
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    res.json().await.unwrap()
}

async fn wait_for_mail(r: &TestRelay, to: &str, subject_part: &str) -> hame_relay::mailer::Email {
    for _ in 0..200 {
        if let Some(m) = r
            .mailer
            .messages_to(to)
            .into_iter()
            .find(|m| m.subject.contains(subject_part))
        {
            return m;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("no mail to {to} with subject containing {subject_part:?}");
}

fn x25519_random() -> String {
    b64url(&hame_relay::util::random_bytes::<32>())
}

// ---- The reviewer's proofs of concept ----

/// L1: a reset link requested before a password change must not work
/// after it.
#[tokio::test]
async fn reset_link_dies_with_a_password_change() {
    let r = spawn(|_| {}).await;
    let login = r.account("Ann", "ann@example.com").await;
    let (s, _) = r
        .post(
            "/v1/auth/password/forgot",
            json!({ "email": "ann@example.com" }),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::ACCEPTED);
    let reset_token = link_token(&r.wait_mail("ann@example.com", 2).await.text);
    let (s, _) = r
        .post(
            "/v1/account/password",
            json!({ "current_password": PASSWORD, "new_password": "brand new secret pass" }),
            Some(&access(&login)),
        )
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, v) = r
        .post(
            "/v1/auth/password/reset",
            json!({ "token": reset_token, "password": "attacker chosen pass" }),
            None,
        )
        .await;
    assert_eq!(
        s,
        StatusCode::BAD_REQUEST,
        "old reset link still valid: {v}"
    );
}

/// L2: a WebSocket must not outlive its access token.
#[tokio::test]
async fn websocket_closes_when_its_token_expires() {
    let r = spawn(|c| c.access_token_ttl = Duration::from_secs(1)).await;
    let login = r.account("Ann", "ann@example.com").await;
    let t = access(&login);
    let d = r.device(&t, "desktop").await;
    let mut ws = r.ws(&d, &t).await;
    ws_send(&mut ws, json!({ "op": "resume", "after": 0 })).await;
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let (s, _) = r.get("/v1/devices", Some(&t)).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "token should be expired");
    ws_send(&mut ws, json!({ "op": "ping" })).await;
    let v = ws_recv_data(&mut ws).await;
    assert_ne!(
        v,
        json!({ "op": "pong" }),
        "socket still serves after expiry"
    );
    assert_eq!(v["code"], "unauthorized");
}

/// H1: devices per account are capped.
#[tokio::test]
async fn devices_per_account_are_capped() {
    let r = spawn(|_| {}).await;
    r.account("Ann", "ann@example.com").await;
    let mut created = 0;
    let mut refused = None;
    for _ in 0..25 {
        let login = r.login("ann@example.com", PASSWORD).await;
        let keys = DeviceKeys::generate();
        let (s, v) = r
            .post(
                "/v1/devices",
                json!({ "name": "x", "kind": "host", "ed25519": keys.ed25519_public(),
                        "x25519": b64url(&[7u8; 32]) }),
                Some(&access(&login)),
            )
            .await;
        if s == StatusCode::CREATED {
            created += 1;
        } else {
            refused = Some((s, v));
        }
    }
    assert_eq!(created, 20, "default cap is 20");
    let (s, v) = refused.unwrap();
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(v["error"], "too_many_devices");
}

/// M1: an attacker registers the victim's address with a password, the
/// victim clicks the verification mail and later signs in with Google: the
/// attacker's password must not survive into the merged account.
#[tokio::test]
async fn pre_registered_password_does_not_survive_a_provider_claim() {
    let (r, google) = with_google(|_| {}).await;
    let (s, _) = r
        .post(
            "/v1/auth/signup",
            json!({ "name": "Vic", "email": "vic@example.com", "password": PASSWORD }),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::ACCEPTED);
    let mail = r.wait_mail("vic@example.com", 1).await;
    let (s, _) = r
        .post(
            "/v1/auth/verify",
            json!({ "token": link_token(&mail.text) }),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    let victim = google_sign_in(&r, &google, "g-vic", "vic@example.com").await;
    let (s, v) = r
        .post(
            "/v1/auth/login",
            json!({ "email": "vic@example.com", "password": PASSWORD }),
            None,
        )
        .await;
    assert!(
        !(s == StatusCode::OK && v["account_id"] == victim["account_id"]),
        "attacker password logs into the victim's Google-linked account"
    );
    // The owner is told that a provider was linked.
    let notice = wait_for_mail(&r, "vic@example.com", "was linked").await;
    assert!(notice.text.contains("Google"));
}

// ---- H1: disk exhaustion ----

/// `(app token, app device, host token, host device)`.
async fn host_and_app(r: &TestRelay, email: &str) -> (String, String, String, String) {
    let app_token = access(&r.account("Q", email).await);
    let app = r.device(&app_token, "desktop").await;
    let (host_token, host) = r.second_device(email, "host").await;
    (app_token, app, host_token, host)
}

#[tokio::test]
async fn queued_bytes_are_capped_per_device_and_per_account() {
    let r = spawn(|c| {
        c.mailbox_max_bytes_per_device = 3 * 1024;
        c.mailbox_max_bytes_per_account = 4608;
    })
    .await;
    let (t, app, _, host) = host_and_app(&r, "q@example.com").await;
    let (_, host2) = r.second_device("q@example.com", "host").await;
    let mut ws = r.ws(&app, &t).await;
    ws_send(&mut ws, json!({ "op": "resume", "after": 0 })).await;
    let two_k = b64url(&[1u8; 2048]);
    ws_send(
        &mut ws,
        json!({ "op": "send", "to": host, "envelope": two_k }),
    )
    .await;
    assert_eq!(ws_recv_data(&mut ws).await["op"], "sent");
    // 4 KiB for one device: over its 3 KiB quota.
    ws_send(
        &mut ws,
        json!({ "op": "send", "to": host, "envelope": two_k }),
    )
    .await;
    let v = ws_recv_data(&mut ws).await;
    assert_eq!(v["code"], "rate_limited");
    assert!(v["message"].as_str().unwrap().contains("recipient"), "{v}");
    // Another device: 2 + 2 KiB fits the account's 4.5 KiB.
    ws_send(
        &mut ws,
        json!({ "op": "send", "to": host2, "envelope": two_k }),
    )
    .await;
    assert_eq!(ws_recv_data(&mut ws).await["op"], "sent");
    // 5 KiB for the account (3 KiB for that device, within its quota): refused.
    ws_send(
        &mut ws,
        json!({ "op": "send", "to": host2, "envelope": b64url(&[1u8; 1024]) }),
    )
    .await;
    let v = ws_recv_data(&mut ws).await;
    assert_eq!(v["code"], "rate_limited");
    assert!(v["message"].as_str().unwrap().contains("account"), "{v}");

    // Acknowledging frees the quota.
    let first = r
        .state
        .repo
        .fetch_after(&host, 0, 1 << 30, 10)
        .await
        .unwrap();
    assert_eq!(first.len(), 1);
    r.state.repo.ack(&host, first[0].seq).await.unwrap();
    ws_send(
        &mut ws,
        json!({ "op": "send", "to": host, "envelope": two_k }),
    )
    .await;
    assert_eq!(ws_recv_data(&mut ws).await["op"], "sent");
}

#[tokio::test]
async fn envelopes_are_refused_below_the_free_disk_floor() {
    let r = spawn(|c| c.min_free_disk_bytes = u64::MAX).await;
    let (t, app, _, host) = host_and_app(&r, "disk@example.com").await;
    let mut ws = r.ws(&app, &t).await;
    ws_send(&mut ws, json!({ "op": "resume", "after": 0 })).await;
    ws_send(
        &mut ws,
        json!({ "op": "send", "to": host, "envelope": b64url(b"x") }),
    )
    .await;
    let v = ws_recv_data(&mut ws).await;
    assert_eq!(v["code"], "unavailable", "{v}");
    assert!(
        r.state
            .repo
            .fetch_after(&host, 0, 1 << 30, 10)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn websocket_connections_per_account_are_capped() {
    let r = spawn(|c| c.max_ws_per_account = 1).await;
    let (t, app, _, _) = host_and_app(&r, "ws@example.com").await;
    let (t2, d2) = r.second_device("ws@example.com", "web").await;
    let _first = r.ws(&app, &t).await;
    let url = format!("{}/v1/ws?device={d2}", r.base.replace("http://", "ws://"));
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut req = url.into_client_request().unwrap();
    req.headers_mut()
        .insert("authorization", format!("Bearer {t2}").parse().unwrap());
    let err = tokio_tungstenite::connect_async(req).await.unwrap_err();
    assert!(err.to_string().contains("429"), "{err}");
    // The same device may reconnect (it replaces its old connection).
    let _again = r.ws(&app, &t).await;
}

#[tokio::test]
async fn delivery_reads_the_queue_in_bounded_batches() {
    let r = spawn(|_| {}).await;
    let (t, app, host_token, host) = host_and_app(&r, "batch@example.com").await;
    let mut ws = r.ws(&app, &t).await;
    ws_send(&mut ws, json!({ "op": "resume", "after": 0 })).await;
    let mib = b64url(&vec![3u8; 1024 * 1024]);
    for _ in 0..6 {
        ws_send(
            &mut ws,
            json!({ "op": "send", "to": host, "envelope": mib }),
        )
        .await;
        assert_eq!(ws_recv_data(&mut ws).await["op"], "sent");
    }
    // One batch holds at most 4 MiB.
    let batch = r
        .state
        .repo
        .fetch_after(&host, 0, hame_relay::ws::DELIVERY_BATCH_BYTES, 256)
        .await
        .unwrap();
    assert_eq!(batch.len(), 4);
    // A single envelope larger than the budget still comes alone.
    let one = r.state.repo.fetch_after(&host, 0, 10, 256).await.unwrap();
    assert_eq!(one.len(), 1);
    // The host still gets all six, in order, across batches.
    let mut hws = r.ws(&host, &host_token).await;
    ws_send(&mut hws, json!({ "op": "resume", "after": 0 })).await;
    for seq in 1..=6 {
        let v = ws_recv_data(&mut hws).await;
        assert_eq!(v["op"], "deliver");
        assert_eq!(v["seq"], seq);
    }
}

// ---- M2: persistence after reset ----

async fn passkey_relay(extra: impl FnOnce(&mut hame_relay::Config) + Send) -> TestRelay {
    spawn(move |c| {
        c.webauthn = Some(WebauthnConfig {
            rp_id: "localhost".into(),
            rp_name: "hame test".into(),
            origins: vec![Url::parse(ORIGIN).unwrap()],
        });
        extra(c);
    })
    .await
}

async fn add_passkey(r: &TestRelay, token: &str) -> (StatusCode, Value) {
    let (s, start) = r
        .post("/v1/auth/passkey/register/start", json!({}), Some(token))
        .await;
    if s != StatusCode::OK {
        return (s, start);
    }
    let options: CreationChallengeResponse =
        serde_json::from_value(start["options"].clone()).unwrap();
    let credential = SoftPasskey::new(true)
        .do_registration(Url::parse(ORIGIN).unwrap(), options)
        .unwrap();
    r.post(
        "/v1/auth/passkey/register/finish",
        json!({ "challenge_id": start["challenge_id"], "credential": credential }),
        Some(token),
    )
    .await
}

#[tokio::test]
async fn adding_a_passkey_needs_a_recent_sign_in() {
    let r = passkey_relay(|c| c.recent_auth_window = Duration::from_secs(1)).await;
    let login = r.account("Pat", "pat@example.com").await;
    let t = access(&login);
    tokio::time::sleep(Duration::from_millis(2100)).await;
    let (s, v) = add_passkey(&r, &t).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(v["error"], "reauth_required");
    // Re-authenticating with the password makes it recent again.
    let (s, _) = r
        .post(
            "/v1/auth/reauth",
            json!({ "password": "wrong password!!" }),
            Some(&t),
        )
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _) = r
        .post("/v1/auth/reauth", json!({ "password": PASSWORD }), Some(&t))
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, v) = add_passkey(&r, &t).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let notice = wait_for_mail(&r, "pat@example.com", "passkey was added").await;
    assert!(notice.text.contains("passkey"));
}

#[tokio::test]
async fn reset_removes_passkeys_and_every_session() {
    let r = passkey_relay(|_| {}).await;
    let login = r.account("Rae", "rae@example.com").await;
    let other = r.login("rae@example.com", PASSWORD).await;
    let (s, _) = add_passkey(&r, &access(&login)).await;
    assert_eq!(s, StatusCode::OK);
    let account_id = login["account_id"].as_str().unwrap();
    assert_eq!(r.state.repo.passkeys(account_id).await.unwrap().len(), 1);
    let (s, _) = r
        .post(
            "/v1/auth/password/forgot",
            json!({ "email": "rae@example.com" }),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::ACCEPTED);
    let mail = wait_for_mail(&r, "rae@example.com", "Your hame account").await;
    let (s, v) = r
        .post(
            "/v1/auth/password/reset",
            json!({ "token": link_token(&mail.text), "password": "a fresh new passphrase" }),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["passkeys_removed"], 1);
    assert!(r.state.repo.passkeys(account_id).await.unwrap().is_empty());
    for t in [&login, &other] {
        assert_eq!(
            r.get("/v1/account", Some(&access(t))).await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    // Change-password can remove passkeys too.
    let fresh = r.login("rae@example.com", "a fresh new passphrase").await;
    let (s, _) = add_passkey(&r, &access(&fresh)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(r.state.repo.passkeys(account_id).await.unwrap().len(), 1);
    let (s, _) = r
        .post(
            "/v1/account/password",
            json!({ "current_password": "a fresh new passphrase",
                    "new_password": "yet another passphrase", "remove_passkeys": true }),
            Some(&access(&fresh)),
        )
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert!(r.state.repo.passkeys(account_id).await.unwrap().is_empty());
}

#[tokio::test]
async fn sessions_are_listed_and_revoked_one_by_one() {
    let r = spawn(|_| {}).await;
    let one = r.account("Sol", "sol@example.com").await;
    let (two_token, two_dev) = r.second_device("sol@example.com", "desktop").await;
    let mut ws = r.ws(&two_dev, &two_token).await;
    ws_send(&mut ws, json!({ "op": "resume", "after": 0 })).await;
    let (s, list) = r.get("/v1/account/sessions", Some(&access(&one))).await;
    assert_eq!(s, StatusCode::OK);
    let sessions = list["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 2);
    let current: Vec<_> = sessions.iter().filter(|s| s["current"] == true).collect();
    assert_eq!(current.len(), 1);
    let other = sessions
        .iter()
        .find(|s| s["device_id"] == two_dev.as_str())
        .unwrap();
    let id = other["id"].as_str().unwrap();
    assert_eq!(
        r.delete(&format!("/v1/account/sessions/{id}"), Some(&access(&one)))
            .await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        r.get("/v1/account", Some(&two_token)).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(ws_recv_data(&mut ws).await["code"], "forbidden");
    // Sessions of other accounts are not found.
    let mal = access(&r.account("Mal", "mal@example.com").await);
    let sid = one["session_id"].as_str().unwrap();
    assert_eq!(
        r.delete(&format!("/v1/account/sessions/{sid}"), Some(&mal))
            .await,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn device_code_sessions_are_limited_to_hosts() {
    let r = passkey_relay(|_| {}).await;
    let user_login = r.account("Ty", "ty@example.com").await;
    let user = access(&user_login);
    let user_dev = r.device(&user, "web").await;
    let (_, start) = r
        .post(
            "/v1/auth/device/start",
            json!({ "client_name": "box" }),
            None,
        )
        .await;
    let (s, _) = r
        .post(
            "/v1/auth/device/approve",
            json!({ "user_code": start["user_code"] }),
            Some(&user),
        )
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, tokens) = r
        .post(
            "/v1/auth/device/token",
            json!({ "device_code": start["device_code"] }),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    let host = access(&tokens);

    // Not allowed: passkeys, approving codes, revoking devices, account
    // changes, sessions, a non-host device.
    let (s, v) = add_passkey(&r, &host).await;
    assert_eq!(
        (s, v["error"].as_str()),
        (StatusCode::FORBIDDEN, Some("insufficient_scope"))
    );
    let (_, start2) = r.post("/v1/auth/device/start", json!({}), None).await;
    let (s, _) = r
        .post(
            "/v1/auth/device/approve",
            json!({ "user_code": start2["user_code"] }),
            Some(&host),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(
        r.delete(&format!("/v1/devices/{user_dev}"), Some(&host))
            .await,
        StatusCode::FORBIDDEN
    );
    let res = r
        .http
        .patch(r.url("/v1/account"))
        .bearer_auth(&host)
        .json(&json!({ "name": "owned" }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        r.get("/v1/account/sessions", Some(&host)).await.0,
        StatusCode::FORBIDDEN
    );
    let (s, _) = r
        .post(
            "/v1/devices",
            json!({ "name": "w", "kind": "web", "ed25519": DeviceKeys::generate().ed25519_public(),
                    "x25519": x25519_random() }),
            Some(&host),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // Allowed: registering and connecting a host.
    let host_dev = r.device(&host, "host").await;
    let mut ws = r.ws(&host_dev, &host).await;
    ws_send(&mut ws, json!({ "op": "ping" })).await;
    assert_eq!(ws_recv_data(&mut ws).await, json!({ "op": "pong" }));

    // Upgraded with the password, the session is full.
    let (s, _) = r
        .post(
            "/v1/auth/reauth",
            json!({ "password": PASSWORD }),
            Some(&host),
        )
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert_eq!(
        r.get("/v1/account/sessions", Some(&host)).await.0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn owners_are_told_about_new_devices_and_sign_ins_from_new_networks() {
    let r = spawn(|c| c.trust_proxy = true).await;
    r.account("Uli", "uli@example.com").await;
    // A device added.
    let (_, _) = r.second_device("uli@example.com", "desktop").await;
    wait_for_mail(&r, "uli@example.com", "device was added").await;
    // Same network: no notice. New network: notice.
    let login_from = |ip: &'static str| {
        r.http
            .post(r.url("/v1/auth/login"))
            .header("x-forwarded-for", ip)
            .json(&json!({ "email": "uli@example.com", "password": PASSWORD }))
            .send()
    };
    assert_eq!(
        login_from("127.0.0.9").await.unwrap().status(),
        StatusCode::OK
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !r.mailer
            .messages_to("uli@example.com")
            .iter()
            .any(|m| m.subject.contains("New sign-in"))
    );
    assert_eq!(
        login_from("198.51.100.20").await.unwrap().status(),
        StatusCode::OK
    );
    let notice = wait_for_mail(&r, "uli@example.com", "New sign-in").await;
    assert!(notice.text.contains("198.51.100.20"));
}

// ---- M4 ----

#[tokio::test]
async fn failed_logins_are_limited_globally() {
    let r = spawn(|c| c.rate_limits.global_failed_logins_per_min = 3).await;
    for i in 0..3 {
        let (s, _) = r
            .post(
                "/v1/auth/login",
                json!({ "email": format!("u{i}@example.com"), "password": "nope nope nope" }),
                None,
            )
            .await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
    }
    let (s, v) = r
        .post(
            "/v1/auth/login",
            json!({ "email": "fresh@example.com", "password": "nope nope nope" }),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(v["error"], "rate_limited");
}

#[tokio::test]
async fn ipv6_clients_share_a_bucket_per_64() {
    let r = spawn(|c| {
        c.rate_limits.auth_per_ip_per_min = 2;
        c.trust_proxy = true;
    })
    .await;
    let send = |ip: &'static str| {
        r.http
            .post(r.url("/v1/auth/password/forgot"))
            .header("x-forwarded-for", ip)
            .json(&json!({ "email": "c@example.com" }))
            .send()
    };
    assert_eq!(
        send("2001:db8:1:2::1").await.unwrap().status(),
        StatusCode::ACCEPTED
    );
    assert_eq!(
        send("2001:db8:1:2::2").await.unwrap().status(),
        StatusCode::ACCEPTED
    );
    // Another address in the same /64: same bucket.
    assert_eq!(
        send("2001:db8:1:2:dead:beef:1:1").await.unwrap().status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        send("2001:db8:1:3::1").await.unwrap().status(),
        StatusCode::ACCEPTED
    );
}

// ---- M5 ----

#[tokio::test]
async fn custom_scheme_return_needs_the_app_code_challenge() {
    let (r, _google) = with_google(|_| {}).await;
    let res = r
        .http
        .get(r.url("/v1/auth/oauth/google/start?return_to=hame://oauth"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let v: Value = res.json().await.unwrap();
    assert_eq!(v["error"], "code_challenge_required");
    // An https return does not need it.
    let res = r
        .http
        .get(r.url("/v1/auth/oauth/google/start?return_to=https://app.example.test/oauth"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
}

// ---- L1 ----

#[tokio::test]
async fn a_provider_claim_deletes_outstanding_reset_links() {
    let (r, google) = with_google(|_| {}).await;
    // Verified, password never used: the reset link was requested by
    // whoever registered.
    let (s, _) = r
        .post(
            "/v1/auth/signup",
            json!({ "name": "Wyn", "email": "wyn@example.com", "password": PASSWORD }),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::ACCEPTED);
    let verify = r.wait_mail("wyn@example.com", 1).await;
    r.post(
        "/v1/auth/verify",
        json!({ "token": link_token(&verify.text) }),
        None,
    )
    .await;
    r.post(
        "/v1/auth/password/forgot",
        json!({ "email": "wyn@example.com" }),
        None,
    )
    .await;
    let reset = wait_for_mail(&r, "wyn@example.com", "Your hame account").await;
    google_sign_in(&r, &google, "g-wyn", "wyn@example.com").await;
    let (s, _) = r
        .post(
            "/v1/auth/password/reset",
            json!({ "token": link_token(&reset.text), "password": "squatter new pass" }),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_proven_password_survives_a_provider_link() {
    let (r, google) = with_google(|_| {}).await;
    let pw = r.account("Xan", "xan@example.com").await; // signed in once
    let g = google_sign_in(&r, &google, "g-xan", "xan@example.com").await;
    assert_eq!(g["account_id"], pw["account_id"]);
    r.login("xan@example.com", PASSWORD).await;
}

// ---- L2 ----

#[tokio::test]
async fn revoked_sessions_are_closed_by_the_periodic_check() {
    let r = spawn(|c| c.ws_ping_interval = Duration::from_millis(200)).await;
    let login = r.account("Yul", "yul@example.com").await;
    let t = access(&login);
    let d = r.device(&t, "desktop").await;
    let mut ws = r.ws(&d, &t).await;
    ws_send(&mut ws, json!({ "op": "resume", "after": 0 })).await;
    // Revoked in storage only (no kick), as if the kick were lost.
    r.state
        .repo
        .revoke_session(
            login["session_id"].as_str().unwrap(),
            "test",
            hame_relay::util::now(),
        )
        .await
        .unwrap();
    let v = ws_recv_data(&mut ws).await;
    assert_eq!(v["code"], "unauthorized");
}

#[tokio::test]
async fn a_connection_can_renew_its_token() {
    let r = spawn(|c| c.access_token_ttl = Duration::from_secs(4)).await;
    let login = r.account("Zed", "zed@example.com").await;
    let t = access(&login);
    let d = r.device(&t, "desktop").await;
    let mut ws = r.ws(&d, &t).await;
    ws_send(&mut ws, json!({ "op": "resume", "after": 0 })).await;
    let start = tokio::time::Instant::now();
    tokio::time::sleep(Duration::from_millis(2000)).await;
    let (_, fresh) = r
        .post(
            "/v1/auth/refresh",
            json!({ "refresh_token": login["refresh_token"] }),
            None,
        )
        .await;
    // A token of another device is refused.
    let other = access(&r.login("zed@example.com", PASSWORD).await);
    ws_send(&mut ws, json!({ "op": "auth", "token": other })).await;
    assert_eq!(ws_recv_data(&mut ws).await["code"], "unauthorized");
    ws_send(&mut ws, json!({ "op": "auth", "token": access(&fresh) })).await;
    assert_eq!(ws_recv_data(&mut ws).await["op"], "auth");
    // The first token expires at most 4 s after the start, the renewed one
    // more than 5 s after: at 4.3 s the connection must still serve.
    tokio::time::sleep_until(start + Duration::from_millis(2300)).await;
    ws_send(&mut ws, json!({ "op": "ping" })).await;
    assert_eq!(ws_recv_data(&mut ws).await, json!({ "op": "pong" }));
}

// ---- Further findings ----

#[tokio::test]
async fn concurrent_refreshes_cannot_both_win() {
    let r = spawn(|_| {}).await;
    let login = r.account("Abe", "abe@example.com").await;
    let body = json!({ "refresh_token": login["refresh_token"] });
    let (a, b) = tokio::join!(
        r.post("/v1/auth/refresh", body.clone(), None),
        r.post("/v1/auth/refresh", body.clone(), None),
    );
    let ok = [a.0, b.0].iter().filter(|s| **s == StatusCode::OK).count();
    assert!(ok <= 1, "both refreshes succeeded");
}

#[tokio::test]
async fn device_keys_must_be_valid_points() {
    let r = spawn(|_| {}).await;
    r.account("Bea", "bea@example.com").await;
    let ed_ok = DeviceKeys::generate().ed25519_public();
    // The Ed25519 identity point (small order) and an encoding that is not
    // a point at all.
    let mut identity = [0u8; 32];
    identity[0] = 1;
    let mut not_a_point = [0xffu8; 32];
    not_a_point[31] = 0x7f;
    // X25519: all zero, and the low-order point of order 8.
    let low_order_x: [u8; 32] = [
        0xe0, 0xeb, 0x7a, 0x7c, 0x3b, 0x41, 0xb8, 0xae, 0x16, 0x56, 0xe3, 0xfa, 0xf1, 0x9f, 0xc4,
        0x6a, 0xda, 0x09, 0x8d, 0xeb, 0x9c, 0x32, 0xb1, 0xfd, 0x86, 0x62, 0x05, 0x16, 0x5f, 0x49,
        0xb8, 0x00,
    ];
    for (ed, x) in [
        (b64url(&identity), x25519_random()),
        (b64url(&not_a_point), x25519_random()),
        (ed_ok.clone(), b64url(&[0u8; 32])),
        (ed_ok.clone(), b64url(&low_order_x)),
    ] {
        let t = access(&r.login("bea@example.com", PASSWORD).await);
        let (s, v) = r
            .post(
                "/v1/devices",
                json!({ "name": "k", "kind": "web", "ed25519": ed, "x25519": x }),
                Some(&t),
            )
            .await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
        assert_eq!(v["error"], "invalid_key");
    }
    assert!(hame_relay::protocol::valid_ed25519_key(
        &hame_relay::util::b64url_decode(&ed_ok).unwrap()
    ));
}

#[tokio::test]
async fn a_saturated_password_hasher_answers_503() {
    let r = spawn(|c| {
        c.argon2_concurrency = 1;
        c.argon2_queue_timeout = Duration::from_millis(1);
        c.argon2 = hame_relay::config::Argon2Params {
            m_kib: 64 * 1024,
            t: 3,
            p: 1,
        };
    })
    .await;
    // Several logins at once against one slot that is held for a while.
    let mut tasks = Vec::new();
    for i in 0..6 {
        let http = r.http.clone();
        let url = r.url("/v1/auth/login");
        tasks.push(tokio::spawn(async move {
            http.post(url)
                .json(
                    &json!({ "email": format!("n{i}@example.com"), "password": "pw pw pw pw pw" }),
                )
                .send()
                .await
                .unwrap()
                .status()
        }));
    }
    let mut statuses = Vec::new();
    for t in tasks {
        statuses.push(t.await.unwrap());
    }
    assert!(
        statuses.contains(&StatusCode::SERVICE_UNAVAILABLE),
        "{statuses:?}"
    );
}
