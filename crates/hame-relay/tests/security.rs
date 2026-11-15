//! Rate limits, CORS, security headers, request size limits, health.

mod common;

use common::*;
use reqwest::StatusCode;
use serde_json::json;

#[tokio::test]
async fn auth_routes_are_rate_limited_per_ip() {
    let r = spawn(|c| c.rate_limits.auth_per_ip_per_min = 5).await;
    for _ in 0..5 {
        let (s, _) = r
            .post(
                "/v1/auth/login",
                json!({ "email": "a@example.com", "password": "nope nope nope" }),
                None,
            )
            .await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
    }
    let res = r
        .http
        .post(r.url("/v1/auth/password/forgot"))
        .json(&json!({ "email": "b@example.com" }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
    let retry: u64 = res.headers()["retry-after"]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!((1..=60).contains(&retry));
    let v: serde_json::Value = res.json().await.unwrap();
    assert_eq!(v["error"], "rate_limited");

    // Behind a trusted proxy, the forwarded address is the key.
    let r = spawn(|c| {
        c.rate_limits.auth_per_ip_per_min = 2;
        c.trust_proxy = true;
    })
    .await;
    let send = |ip: &'static str| {
        r.http
            .post(r.url("/v1/auth/password/forgot"))
            .header("x-forwarded-for", format!("198.51.100.1, {ip}"))
            .json(&json!({ "email": "c@example.com" }))
            .send()
    };
    assert_eq!(
        send("203.0.113.1").await.unwrap().status(),
        StatusCode::ACCEPTED
    );
    assert_eq!(
        send("203.0.113.1").await.unwrap().status(),
        StatusCode::ACCEPTED
    );
    assert_eq!(
        send("203.0.113.1").await.unwrap().status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        send("203.0.113.2").await.unwrap().status(),
        StatusCode::ACCEPTED
    );
}

#[tokio::test]
async fn login_is_rate_limited_per_account() {
    let r = spawn(|c| c.rate_limits.auth_per_account_per_min = 3).await;
    r.account("Wes", "wes@example.com").await; // one login used
    for _ in 0..2 {
        let (s, _) = r
            .post(
                "/v1/auth/login",
                json!({ "email": "wes@example.com", "password": "guess guess guess" }),
                None,
            )
            .await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
    }
    // Even the right password is refused now.
    let (s, v) = r
        .post(
            "/v1/auth/login",
            json!({ "email": "wes@example.com", "password": PASSWORD }),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(v["error"], "rate_limited");
    // Other accounts are unaffected.
    let (s, _) = r
        .post(
            "/v1/auth/login",
            json!({ "email": "other@example.com", "password": "guess guess guess" }),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    // Failed attempts are audited.
    let audit = r
        .state
        .repo
        .audit_for_account(
            &r.state
                .repo
                .account_by_email("wes@example.com")
                .await
                .unwrap()
                .unwrap()
                .id,
            50,
        )
        .await
        .unwrap();
    assert_eq!(
        audit.iter().filter(|e| e.event == "login_failed").count(),
        2
    );
}

#[tokio::test]
async fn security_headers_cors_and_body_limit() {
    let r = spawn(|c| {
        c.cors_origins = vec!["https://app.example.test".into()];
        c.hsts = true;
    })
    .await;
    let res = r.http.get(r.url("/healthz")).send().await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let h = res.headers();
    assert_eq!(h["x-content-type-options"], "nosniff");
    assert_eq!(h["x-frame-options"], "DENY");
    assert_eq!(h["referrer-policy"], "no-referrer");
    assert_eq!(h["cache-control"], "no-store");
    assert!(
        h["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("default-src 'none'")
    );
    assert!(
        h["strict-transport-security"]
            .to_str()
            .unwrap()
            .contains("max-age=")
    );
    let v: serde_json::Value = res.json().await.unwrap();
    assert_eq!(v["status"], "ok");

    // CORS: the configured origin only.
    let preflight = |origin: &'static str| {
        r.http
            .request(reqwest::Method::OPTIONS, r.url("/v1/auth/login"))
            .header("origin", origin)
            .header("access-control-request-method", "POST")
            .header("access-control-request-headers", "content-type")
            .send()
    };
    let ok = preflight("https://app.example.test").await.unwrap();
    assert_eq!(
        ok.headers()["access-control-allow-origin"],
        "https://app.example.test"
    );
    let bad = preflight("https://evil.example").await.unwrap();
    assert!(bad.headers().get("access-control-allow-origin").is_none());

    // Bodies over the limit are refused.
    let big = "x".repeat(100 * 1024);
    let (s, v) = r
        .post(
            "/v1/auth/signup",
            json!({ "name": "A", "email": "a@example.com", "password": big }),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::PAYLOAD_TOO_LARGE, "{v}");

    // Unknown routes are JSON 404s.
    let (s, v) = r.get("/v1/nothing-here", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(v["error"], "not_found");
}
