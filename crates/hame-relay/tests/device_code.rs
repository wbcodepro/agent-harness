//! Device-code sign-in for hosts and CLIs.

mod common;

use common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::time::Duration;

async fn poll(r: &TestRelay, device_code: &str) -> (StatusCode, Value) {
    r.post(
        "/v1/auth/device/token",
        json!({ "device_code": device_code }),
        None,
    )
    .await
}

#[tokio::test]
async fn host_signs_in_with_a_device_code() {
    let r = spawn(|_| {}).await;
    let user = access(&r.account("Uma", "uma@example.com").await);

    let (s, start) = r
        .post(
            "/v1/auth/device/start",
            json!({ "client_name": "uma-laptop" }),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{start}");
    let device_code = start["device_code"].as_str().unwrap().to_string();
    let user_code = start["user_code"].as_str().unwrap().to_string();
    assert_eq!(user_code.len(), 9);
    assert_eq!(start["interval"], 1);
    assert_eq!(start["expires_in"], 600);
    assert_eq!(start["verification_url"], "https://app.example.test/device");
    assert!(
        start["verification_url_complete"]
            .as_str()
            .unwrap()
            .ends_with(&format!("code={user_code}"))
    );

    let (s, v) = poll(&r, &device_code).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(v["error"], "authorization_pending");

    // The signed-in user sees what is asking (code case and dash do not
    // matter) and approves.
    let typed = user_code.to_lowercase().replace('-', "");
    let (s, info) = r
        .post(
            "/v1/auth/device/lookup",
            json!({ "user_code": typed }),
            Some(&user),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(info["client_name"], "uma-laptop");
    // Approval needs a session.
    let (s, _) = r
        .post(
            "/v1/auth/device/approve",
            json!({ "user_code": user_code }),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _) = r
        .post(
            "/v1/auth/device/approve",
            json!({ "user_code": user_code }),
            Some(&user),
        )
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    // Not twice.
    let (s, _) = r
        .post(
            "/v1/auth/device/approve",
            json!({ "user_code": user_code }),
            Some(&user),
        )
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    tokio::time::sleep(Duration::from_millis(1100)).await;
    let (s, tokens) = poll(&r, &device_code).await;
    assert_eq!(s, StatusCode::OK, "{tokens}");
    let host_token = access(&tokens);
    let host = r.device(&host_token, "host").await;
    let (_, hosts) = r.get("/v1/hosts", Some(&user)).await;
    assert_eq!(hosts["hosts"][0]["id"], host.as_str());

    // The code is spent.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let (s, v) = poll(&r, &device_code).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(v["error"], "invalid_grant");
}

#[tokio::test]
async fn polling_too_fast_gets_slow_down() {
    let r = spawn(|_| {}).await;
    let (_, start) = r.post("/v1/auth/device/start", json!({}), None).await;
    let code = start["device_code"].as_str().unwrap();
    let (_, v) = poll(&r, code).await;
    assert_eq!(v["error"], "authorization_pending");
    let (s, v) = poll(&r, code).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(v["error"], "slow_down");
    assert_eq!(v["interval"], 6);
}

#[tokio::test]
async fn codes_expire_and_can_be_denied() {
    let r = spawn(|c| c.device_code_ttl = Duration::from_secs(1)).await;
    let (_, start) = r.post("/v1/auth/device/start", json!({}), None).await;
    tokio::time::sleep(Duration::from_millis(2100)).await;
    let (s, v) = poll(&r, start["device_code"].as_str().unwrap()).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(v["error"], "expired_token");

    let r = spawn(|_| {}).await;
    let user = access(&r.account("Vic", "vic@example.com").await);
    let (_, start) = r.post("/v1/auth/device/start", json!({}), None).await;
    let (s, _) = r
        .post(
            "/v1/auth/device/approve",
            json!({ "user_code": start["user_code"], "approve": false }),
            Some(&user),
        )
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, v) = poll(&r, start["device_code"].as_str().unwrap()).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(v["error"], "access_denied");

    let (s, v) = poll(&r, "hdc_unknown").await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(v["error"], "invalid_grant");
}
