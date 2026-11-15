//! Email and password accounts, tokens, devices and the profile.

mod common;

use common::*;
use reqwest::StatusCode;
use serde_json::json;

#[tokio::test]
async fn signup_verify_login_and_profile() {
    let r = spawn(|_| {}).await;
    let (s, body) = r
        .post(
            "/v1/auth/signup",
            json!({ "name": "Ada", "email": "Ada@Example.com", "password": PASSWORD }),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::ACCEPTED);
    assert_eq!(body["status"], "accepted");

    // Not verified yet: the right password is refused.
    let (s, v) = r
        .post(
            "/v1/auth/login",
            json!({ "email": "ada@example.com", "password": PASSWORD }),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(v["error"], "email_not_verified");

    let mail = r.wait_mail("ada@example.com", 1).await;
    assert!(mail.text.contains("https://app.example.test/verify?token="));
    let token = link_token(&mail.text);
    let (s, _) = r
        .post("/v1/auth/verify", json!({ "token": token }), None)
        .await;
    assert_eq!(s, StatusCode::OK);
    // A verification link works once.
    let (s, v) = r
        .post("/v1/auth/verify", json!({ "token": token }), None)
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(v["error"], "invalid_token");

    let login = r.login("ADA@example.com ", PASSWORD).await;
    assert_eq!(login["token_type"], "Bearer");
    assert_eq!(login["expires_in"], 900);
    let at = access(&login);
    assert!(at.starts_with("hra_"));
    assert!(login["refresh_token"].as_str().unwrap().starts_with("hrr_"));

    let (s, me) = r.get("/v1/account", Some(&at)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(me["name"], "Ada");
    assert_eq!(me["email"], "ada@example.com");
    assert_eq!(me["email_verified"], true);
    assert_eq!(me["has_password"], true);

    let res = r
        .http
        .patch(r.url("/v1/account"))
        .bearer_auth(&at)
        .json(&json!({ "name": "Ada L." }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    let (_, me) = r.get("/v1/account", Some(&at)).await;
    assert_eq!(me["name"], "Ada L.");

    // Tokens are stored hashed: the raw token is nowhere in the database.
    let db = std::fs::read(r.state.config.database_path.clone()).unwrap_or_default();
    let wal_path = format!("{}-wal", r.state.config.database_path.display());
    let wal = std::fs::read(wal_path).unwrap_or_default();
    for blob in [&db, &wal] {
        assert!(!contains(blob, at.as_bytes()));
        assert!(!contains(blob, PASSWORD.as_bytes()));
        assert!(!contains(blob, token.as_bytes()));
    }

    let (_, audit) = r.get("/v1/account/audit", Some(&at)).await;
    let events: Vec<&str> = audit["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["event"].as_str().unwrap())
        .collect();
    assert!(events.contains(&"signup"));
    assert!(events.contains(&"email_verified"));
    assert!(events.contains(&"login"));
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

#[tokio::test]
async fn signup_does_not_reveal_registered_emails() {
    let r = spawn(|_| {}).await;
    r.account("Bo", "bo@example.com").await;
    let (s1, b1) = r
        .post(
            "/v1/auth/signup",
            json!({ "name": "Mallory", "email": "bo@example.com", "password": "another fine password" }),
            None,
        )
        .await;
    let (s2, b2) = r
        .post(
            "/v1/auth/signup",
            json!({ "name": "Cy", "email": "cy@example.com", "password": "another fine password" }),
            None,
        )
        .await;
    assert_eq!(s1, s2);
    assert_eq!(b1, b2);
    // The owner is told, with a reset link instead of a verification link.
    let mail = r.wait_mail("bo@example.com", 2).await;
    assert!(mail.text.contains("already has one"));
    assert!(mail.text.contains("/reset-password?token="));
    // The second sign-up did not change the password.
    r.login("bo@example.com", PASSWORD).await;

    // Login failures look the same for unknown and known addresses.
    let (s1, b1) = r
        .post(
            "/v1/auth/login",
            json!({ "email": "bo@example.com", "password": "wrong password!" }),
            None,
        )
        .await;
    let (s2, b2) = r
        .post(
            "/v1/auth/login",
            json!({ "email": "nobody@example.com", "password": "wrong password!" }),
            None,
        )
        .await;
    assert_eq!(s1, StatusCode::UNAUTHORIZED);
    assert_eq!((s1, b1), (s2, b2));

    // Forgot-password answers the same too.
    let (s1, b1) = r
        .post(
            "/v1/auth/password/forgot",
            json!({ "email": "bo@example.com" }),
            None,
        )
        .await;
    let (s2, b2) = r
        .post(
            "/v1/auth/password/forgot",
            json!({ "email": "nobody@example.com" }),
            None,
        )
        .await;
    assert_eq!(s1, StatusCode::ACCEPTED);
    assert_eq!((s1, b1), (s2, b2));
}

#[tokio::test]
async fn signup_validates_input() {
    let r = spawn(|_| {}).await;
    for (body, code) in [
        (
            json!({ "name": "A", "email": "a@example.com", "password": "short" }),
            "weak_password",
        ),
        (
            json!({ "name": "A", "email": "a@example.com", "password": "password123" }),
            "weak_password",
        ),
        (
            json!({ "name": "A", "email": "not-an-email", "password": PASSWORD }),
            "invalid_email",
        ),
        (
            json!({ "name": " ", "email": "a@example.com", "password": PASSWORD }),
            "invalid_name",
        ),
    ] {
        let (s, v) = r.post("/v1/auth/signup", body, None).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
        assert_eq!(v["error"], code);
    }
    let (s, v) = r
        .post("/v1/auth/signup", json!({ "name": "A" }), None)
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(v["error"], "bad_request");
}

#[tokio::test]
async fn refresh_rotates_and_reuse_revokes_the_family() {
    let r = spawn(|_| {}).await;
    let login = r.account("Di", "di@example.com").await;
    let r1 = login["refresh_token"].as_str().unwrap().to_string();
    let a1 = access(&login);

    let (s, t2) = r
        .post("/v1/auth/refresh", json!({ "refresh_token": r1 }), None)
        .await;
    assert_eq!(s, StatusCode::OK);
    let r2 = t2["refresh_token"].as_str().unwrap().to_string();
    let a2 = access(&t2);
    assert_ne!(r1, r2);
    assert_eq!(t2["session_id"], login["session_id"]);
    assert_eq!(r.get("/v1/account", Some(&a2)).await.0, StatusCode::OK);

    // The old refresh token again: reuse, the whole session is revoked.
    let (s, v) = r
        .post("/v1/auth/refresh", json!({ "refresh_token": r1 }), None)
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(v["error"], "token_reused");
    assert_eq!(
        r.get("/v1/account", Some(&a1)).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        r.get("/v1/account", Some(&a2)).await.0,
        StatusCode::UNAUTHORIZED
    );
    let (s, v) = r
        .post("/v1/auth/refresh", json!({ "refresh_token": r2 }), None)
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(v["error"], "invalid_token");

    // The event is in the audit log.
    let fresh = r.login("di@example.com", PASSWORD).await;
    let (_, audit) = r.get("/v1/account/audit", Some(&access(&fresh))).await;
    assert!(
        audit["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["event"] == "refresh_token_reused")
    );

    // Garbage tokens.
    let (s, _) = r
        .post(
            "/v1/auth/refresh",
            json!({ "refresh_token": "hrr_nope" }),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(
        r.get("/v1/account", Some("hra_nope")).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(r.get("/v1/account", None).await.0, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn access_tokens_expire() {
    let r = spawn(|c| c.access_token_ttl = std::time::Duration::from_secs(1)).await;
    let login = r.account("Ed", "ed@example.com").await;
    let at = access(&login);
    assert_eq!(r.get("/v1/account", Some(&at)).await.0, StatusCode::OK);
    tokio::time::sleep(std::time::Duration::from_millis(2100)).await;
    assert_eq!(
        r.get("/v1/account", Some(&at)).await.0,
        StatusCode::UNAUTHORIZED
    );
    let (s, t) = r
        .post(
            "/v1/auth/refresh",
            json!({ "refresh_token": login["refresh_token"] }),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        r.get("/v1/account", Some(&access(&t))).await.0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn logout_revokes_the_session_only() {
    let r = spawn(|_| {}).await;
    let one = r.account("Fay", "fay@example.com").await;
    let two = r.login("fay@example.com", PASSWORD).await;
    let (s, _) = r
        .post("/v1/auth/logout", json!({}), Some(&access(&one)))
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert_eq!(
        r.get("/v1/account", Some(&access(&one))).await.0,
        StatusCode::UNAUTHORIZED
    );
    let (s, _) = r
        .post(
            "/v1/auth/refresh",
            json!({ "refresh_token": one["refresh_token"] }),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(
        r.get("/v1/account", Some(&access(&two))).await.0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn password_reset_flow() {
    let r = spawn(|_| {}).await;
    let before = r.account("Gus", "gus@example.com").await;
    let (s, _) = r
        .post(
            "/v1/auth/password/forgot",
            json!({ "email": "gus@example.com" }),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::ACCEPTED);
    let mail = r.wait_mail("gus@example.com", 2).await;
    assert!(mail.text.contains("/reset-password?token="));
    let token = link_token(&mail.text);

    // A weak password is refused and the link stays usable.
    let (s, v) = r
        .post(
            "/v1/auth/password/reset",
            json!({ "token": token, "password": "short" }),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(v["error"], "weak_password");

    let new_password = "a much better passphrase";
    let (s, _) = r
        .post(
            "/v1/auth/password/reset",
            json!({ "token": token, "password": new_password }),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    // Used once only.
    let (s, v) = r
        .post(
            "/v1/auth/password/reset",
            json!({ "token": token, "password": new_password }),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(v["error"], "invalid_token");

    // Existing sessions are revoked, the old password no longer works.
    assert_eq!(
        r.get("/v1/account", Some(&access(&before))).await.0,
        StatusCode::UNAUTHORIZED
    );
    let (s, _) = r
        .post(
            "/v1/auth/login",
            json!({ "email": "gus@example.com", "password": PASSWORD }),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    r.login("gus@example.com", new_password).await;
}

#[tokio::test]
async fn change_password_needs_the_current_one() {
    let r = spawn(|_| {}).await;
    let one = r.account("Hal", "hal@example.com").await;
    let two = r.login("hal@example.com", PASSWORD).await;
    let (s, _) = r
        .post(
            "/v1/account/password",
            json!({ "current_password": "wrong one here", "new_password": "a new long passphrase" }),
            Some(&access(&one)),
        )
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _) = r
        .post(
            "/v1/account/password",
            json!({ "current_password": PASSWORD, "new_password": "a new long passphrase" }),
            Some(&access(&one)),
        )
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    // Other sessions are signed out, this one stays.
    assert_eq!(
        r.get("/v1/account", Some(&access(&one))).await.0,
        StatusCode::OK
    );
    assert_eq!(
        r.get("/v1/account", Some(&access(&two))).await.0,
        StatusCode::UNAUTHORIZED
    );
    r.login("hal@example.com", "a new long passphrase").await;
}

#[tokio::test]
async fn devices_hosts_list_and_revoke() {
    let r = spawn(|_| {}).await;
    let web = r.account("Ivy", "ivy@example.com").await;
    let web_token = access(&web);
    let web_dev = r.device(&web_token, "web").await;
    let (host_token, host_dev) = r.second_device("ivy@example.com", "host").await;

    // One device per session.
    let (s, v) = r
        .post(
            "/v1/devices",
            json!({ "name": "again", "kind": "web", "ed25519": hame_relay::util::b64url(&[1; 32]),
                    "x25519": hame_relay::util::b64url(&[2; 32]) }),
            Some(&web_token),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");

    // Bad input.
    let other = access(&r.login("ivy@example.com", PASSWORD).await);
    let (s, v) = r
        .post(
            "/v1/devices",
            json!({ "name": "x", "kind": "toaster", "ed25519": hame_relay::util::b64url(&[1; 32]),
                    "x25519": hame_relay::util::b64url(&[2; 32]) }),
            Some(&other),
        )
        .await;
    assert_eq!(
        (s, v["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("invalid_kind"))
    );
    let (s, v) = r
        .post(
            "/v1/devices",
            json!({ "name": "x", "kind": "web", "ed25519": "AAAA", "x25519": hame_relay::util::b64url(&[2; 32]) }),
            Some(&other),
        )
        .await;
    assert_eq!(
        (s, v["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("invalid_key"))
    );

    let (s, list) = r.get("/v1/devices", Some(&web_token)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(list["devices"].as_array().unwrap().len(), 2);

    let (s, hosts) = r.get("/v1/hosts", Some(&web_token)).await;
    assert_eq!(s, StatusCode::OK);
    let hosts = hosts["hosts"].as_array().unwrap();
    assert_eq!(hosts.len(), 1);
    assert_eq!(hosts[0]["id"], host_dev.as_str());
    assert_eq!(hosts[0]["online"], false);
    assert_eq!(hosts[0]["fingerprint"].as_str().unwrap().len(), 64);
    assert!(hosts[0]["ed25519"].is_string());
    assert!(hosts[0]["x25519"].is_string());

    // Online status follows the WebSocket.
    let mut ws = r.ws(&host_dev, &host_token).await;
    ws_send(&mut ws, json!({ "op": "resume", "after": 0 })).await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let (_, hosts) = r.get("/v1/hosts", Some(&web_token)).await;
    assert_eq!(hosts["hosts"][0]["online"], true);

    // Revoking the host from the web device: its token stops working and
    // its connection is closed.
    let s = r
        .delete(&format!("/v1/devices/{host_dev}"), Some(&web_token))
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert_eq!(
        r.get("/v1/account", Some(&host_token)).await.0,
        StatusCode::UNAUTHORIZED
    );
    let v = ws_recv_data(&mut ws).await;
    assert_eq!(v["op"], "error");
    assert_eq!(v["code"], "forbidden");
    let (_, hosts) = r.get("/v1/hosts", Some(&web_token)).await;
    assert!(hosts["hosts"].as_array().unwrap().is_empty());

    // Another account cannot see or revoke this account's devices.
    let mallory = access(&r.account("Mal", "mal@example.com").await);
    let (_, list) = r.get("/v1/devices", Some(&mallory)).await;
    assert!(list["devices"].as_array().unwrap().is_empty());
    assert_eq!(
        r.delete(&format!("/v1/devices/{web_dev}"), Some(&mallory))
            .await,
        StatusCode::NOT_FOUND
    );

    let (_, audit) = r.get("/v1/account/audit", Some(&web_token)).await;
    let events: Vec<&str> = audit["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["event"].as_str().unwrap())
        .collect();
    assert!(events.contains(&"device_added"));
    assert!(events.contains(&"device_revoked"));
}

#[tokio::test]
async fn device_rebinds_to_a_new_session_with_proof() {
    let r = spawn(|_| {}).await;
    let login = r.account("Jo", "jo@example.com").await;
    let keys = DeviceKeys::generate();
    let dev = r.device_with_keys(&access(&login), "desktop", &keys).await;

    let fresh = r.login("jo@example.com", PASSWORD).await;
    let token = access(&fresh);
    let account = fresh["account_id"].as_str().unwrap();
    let session = fresh["session_id"].as_str().unwrap();
    let ts = hame_relay::util::now();

    // A signature by another key is refused.
    let other = DeviceKeys::generate();
    let msg = format!("hame-relay-rebind:v1\n{account}\n{dev}\n{session}\n{ts}");
    let (s, _) = r
        .post(
            &format!("/v1/devices/{dev}/rebind"),
            json!({ "timestamp": ts, "signature": other.sign(msg.as_bytes()) }),
            Some(&token),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    let (s, _) = r
        .post(
            &format!("/v1/devices/{dev}/rebind"),
            json!({ "timestamp": ts, "signature": keys.sign(msg.as_bytes()) }),
            Some(&token),
        )
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (_, me) = r.get("/v1/account", Some(&token)).await;
    assert_eq!(me["device_id"], dev.as_str());
}

#[tokio::test]
async fn hosts_publish_signed_grants() {
    let r = spawn(|_| {}).await;
    let client = access(&r.account("Kim", "kim@example.com").await);
    let client_dev = r.device(&client, "ios").await;
    let host_login = r.login("kim@example.com", PASSWORD).await;
    let host_token = access(&host_login);
    let keys = DeviceKeys::generate();
    let host = r.device_with_keys(&host_token, "host", &keys).await;

    let doc = format!("{{\"devices\":[\"{client_dev}\"]}}");
    let doc_b64 = hame_relay::util::b64url(doc.as_bytes());
    let signed = |k: &DeviceKeys, version: i64| {
        k.sign(&hame_relay::protocol::grants_message(
            &host,
            version,
            doc.as_bytes(),
        ))
    };
    let publish = |token: String, version: i64, signature: String| {
        let r = &r;
        let host = host.clone();
        let doc_b64 = doc_b64.clone();
        async move {
            r.post(
                &format!("/v1/hosts/{host}/grants"),
                json!({ "version": version, "document": doc_b64, "signature": signature }),
                Some(&token),
            )
            .await
            .0
        }
    };
    // Bad signature.
    let other = DeviceKeys::generate();
    assert_eq!(
        publish(host_token.clone(), 1, signed(&other, 1)).await,
        StatusCode::BAD_REQUEST
    );
    // A signature for another version does not verify.
    assert_eq!(
        publish(host_token.clone(), 2, signed(&keys, 1)).await,
        StatusCode::BAD_REQUEST
    );
    // Only the host itself may publish.
    assert_eq!(
        publish(client.clone(), 1, signed(&keys, 1)).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        publish(host_token.clone(), 2, signed(&keys, 2)).await,
        StatusCode::NO_CONTENT
    );
    // An older or equal version is refused: no rollback.
    assert_eq!(
        publish(host_token.clone(), 1, signed(&keys, 1)).await,
        StatusCode::CONFLICT
    );
    assert_eq!(
        publish(host_token.clone(), 2, signed(&keys, 2)).await,
        StatusCode::CONFLICT
    );
    let (s, g) = r
        .get(&format!("/v1/hosts/{host}/grants"), Some(&client))
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(g["document"], doc_b64.as_str());
    assert_eq!(g["version"], 2);
    assert_eq!(
        publish(host_token.clone(), 3, signed(&keys, 3)).await,
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn push_tokens_belong_to_their_device() {
    let r = spawn(|_| {}).await;
    let phone = access(&r.account("Lu", "lu@example.com").await);
    let phone_dev = r.device(&phone, "ios").await;
    let (desk, _) = r.second_device("lu@example.com", "desktop").await;
    let (s, _) = r
        .post(
            &format!("/v1/devices/{phone_dev}/push"),
            json!({ "provider": "apns", "token": "abc123" }),
            Some(&desk),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = r
        .post(
            &format!("/v1/devices/{phone_dev}/push"),
            json!({ "provider": "carrier-pigeon", "token": "abc123" }),
            Some(&phone),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = r
        .post(
            &format!("/v1/devices/{phone_dev}/push"),
            json!({ "provider": "apns", "token": "abc123" }),
            Some(&phone),
        )
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (_, list) = r.get("/v1/devices", Some(&desk)).await;
    let phone_entry = list["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["id"] == phone_dev.as_str())
        .unwrap()
        .clone();
    assert_eq!(phone_entry["push"], "apns");
}

#[tokio::test]
async fn mail_to_one_address_is_limited() {
    let r = spawn(|_| {}).await;
    r.account("Max", "max@example.com").await; // one verification mail
    for _ in 0..5 {
        let (s, _) = r
            .post(
                "/v1/auth/password/forgot",
                json!({ "email": "max@example.com" }),
                None,
            )
            .await;
        assert_eq!(s, StatusCode::ACCEPTED);
    }
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(r.mailer.messages_to("max@example.com").len(), 3);
}
