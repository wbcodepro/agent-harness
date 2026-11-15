//! WebSocket routing between devices of one account.

mod common;

use common::*;
use hame_relay::util::b64url;
use serde_json::json;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

struct Pair {
    r: TestRelay,
    a_token: String,
    a: String,
    h_token: String,
    h: String,
}

async fn pair() -> Pair {
    let r = spawn(|_| {}).await;
    let login = r.account("Ann", "ann@example.com").await;
    let a_token = access(&login);
    let a = r.device(&a_token, "desktop").await;
    let (h_token, h) = r.second_device("ann@example.com", "host").await;
    Pair {
        r,
        a_token,
        a,
        h_token,
        h,
    }
}

#[tokio::test]
async fn deliver_ack_resume_and_presence() {
    let p = pair().await;
    let mut host = p.r.ws(&p.h, &p.h_token).await;
    ws_send(&mut host, json!({ "op": "resume", "after": 0 })).await;
    let mut app = p.r.ws(&p.a, &p.a_token).await;
    ws_send(&mut app, json!({ "op": "resume", "after": 0 })).await;

    // Each side learns that the other is online.
    let v = ws_recv(&mut app).await;
    assert_eq!(
        v,
        json!({ "op": "presence", "device": p.h, "online": true })
    );
    let v = ws_recv(&mut host).await;
    assert_eq!(
        v,
        json!({ "op": "presence", "device": p.a, "online": true })
    );

    // Send, delivered live.
    let env = b64url(b"opaque ciphertext 1");
    ws_send(
        &mut app,
        json!({ "op": "send", "to": p.h, "envelope": env }),
    )
    .await;
    assert_eq!(
        ws_recv_data(&mut app).await,
        json!({ "op": "sent", "to": p.h, "seq": 1 })
    );
    assert_eq!(
        ws_recv_data(&mut host).await,
        json!({ "op": "deliver", "seq": 1, "from": p.a, "envelope": env })
    );
    ws_send(&mut host, json!({ "op": "ack", "seq": 1 })).await;

    // Ping is answered.
    ws_send(&mut app, json!({ "op": "ping" })).await;
    assert_eq!(ws_recv_data(&mut app).await, json!({ "op": "pong" }));

    // The host goes away; the app is told.
    drop(host);
    let v = ws_recv(&mut app).await;
    assert_eq!(
        v,
        json!({ "op": "presence", "device": p.h, "online": false })
    );

    // Envelopes for the offline host are queued with increasing seq.
    for (i, text) in ["two", "three"].iter().enumerate() {
        ws_send(
            &mut app,
            json!({ "op": "send", "to": p.h, "envelope": b64url(text.as_bytes()) }),
        )
        .await;
        let v = ws_recv_data(&mut app).await;
        assert_eq!(v["op"], "sent");
        assert_eq!(v["seq"], 2 + i as i64);
    }

    // Reconnect and resume after the last ack: both arrive, in order.
    let mut host = p.r.ws(&p.h, &p.h_token).await;
    ws_send(&mut host, json!({ "op": "resume", "after": 1 })).await;
    let v = ws_recv_data(&mut host).await;
    assert_eq!(
        (v["seq"].as_i64(), v["envelope"].as_str()),
        (Some(2), Some(b64url(b"two").as_str()))
    );
    let v = ws_recv_data(&mut host).await;
    assert_eq!(v["seq"], 3);
    assert_eq!(v["from"], p.a.as_str());
    // No ack for 3: it comes again after another reconnect (resume after 2
    // drops 2).
    drop(host);
    let mut host = p.r.ws(&p.h, &p.h_token).await;
    ws_send(&mut host, json!({ "op": "resume", "after": 2 })).await;
    let v = ws_recv_data(&mut host).await;
    assert_eq!(v["seq"], 3);
    ws_send(&mut host, json!({ "op": "ack", "seq": 3 })).await;
    ws_quiet(&mut host, 200).await;
    drop(host);

    // After the ack nothing is redelivered, even when the client does not
    // send resume first (the relay resumes from the last ack).
    let mut host = p.r.ws(&p.h, &p.h_token).await;
    ws_send(&mut host, json!({ "op": "ping" })).await;
    assert_eq!(ws_recv_data(&mut host).await, json!({ "op": "pong" }));
    ws_quiet(&mut host, 200).await;

    // The queue is empty in storage too.
    let left =
        p.r.state
            .repo
            .fetch_after(&p.h, 0, 1 << 30, 10)
            .await
            .unwrap();
    assert!(left.is_empty());

    // The host can answer the app.
    ws_send(
        &mut host,
        json!({ "op": "send", "to": p.a, "envelope": b64url(b"reply") }),
    )
    .await;
    assert_eq!(ws_recv_data(&mut host).await["op"], "sent");
    let v = ws_recv_data(&mut app).await;
    assert_eq!(v["op"], "deliver");
    assert_eq!(v["from"], p.h.as_str());
    assert_eq!(v["seq"], 1, "seq is per recipient");
}

#[tokio::test]
async fn routing_is_refused_across_accounts() {
    let p = pair().await;
    let other = access(&p.r.account("Bob", "bob@example.com").await);
    let bob_dev = p.r.device(&other, "web").await;
    let mut bob = p.r.ws(&bob_dev, &other).await;
    ws_send(&mut bob, json!({ "op": "resume", "after": 0 })).await;
    ws_send(
        &mut bob,
        json!({ "op": "send", "to": p.h, "envelope": b64url(b"x") }),
    )
    .await;
    let v = ws_recv_data(&mut bob).await;
    assert_eq!(v["op"], "error");
    assert_eq!(v["code"], "not_found");
    assert_eq!(v["to"], p.h.as_str());
    // Indistinguishable from a device id that does not exist.
    ws_send(
        &mut bob,
        json!({ "op": "send", "to": "no-such-device", "envelope": b64url(b"x") }),
    )
    .await;
    let w = ws_recv_data(&mut bob).await;
    assert_eq!(
        (v["code"].clone(), v["message"].clone()),
        (w["code"].clone(), w["message"].clone())
    );
    // Nothing was queued for the host.
    assert!(
        p.r.state
            .repo
            .fetch_after(&p.h, 0, 1 << 30, 10)
            .await
            .unwrap()
            .is_empty()
    );

    // Presence of other accounts is not shown.
    let mut host = p.r.ws(&p.h, &p.h_token).await;
    ws_send(&mut host, json!({ "op": "resume", "after": 0 })).await;
    ws_quiet(&mut bob, 300).await;

    // Sending to oneself is refused.
    ws_send(
        &mut bob,
        json!({ "op": "send", "to": bob_dev, "envelope": b64url(b"x") }),
    )
    .await;
    assert_eq!(ws_recv_data(&mut bob).await["code"], "forbidden");
}

#[tokio::test]
async fn envelope_size_limit_is_one_mebibyte() {
    let p = pair().await;
    let mut app = p.r.ws(&p.a, &p.a_token).await;
    ws_send(&mut app, json!({ "op": "resume", "after": 0 })).await;

    let max = vec![7u8; 1024 * 1024];
    ws_send(
        &mut app,
        json!({ "op": "send", "to": p.h, "envelope": b64url(&max) }),
    )
    .await;
    let v = ws_recv_data(&mut app).await;
    assert_eq!(v["op"], "sent", "{v}");

    let over = vec![7u8; 1024 * 1024 + 1];
    ws_send(
        &mut app,
        json!({ "op": "send", "to": p.h, "envelope": b64url(&over) }),
    )
    .await;
    let v = ws_recv_data(&mut app).await;
    assert_eq!(v["op"], "error");
    assert_eq!(v["code"], "too_large");

    // Not base64url, and unknown frames.
    ws_send(
        &mut app,
        json!({ "op": "send", "to": p.h, "envelope": "a+b/" }),
    )
    .await;
    assert_eq!(ws_recv_data(&mut app).await["code"], "bad_request");
    ws_send(&mut app, json!({ "op": "shout" })).await;
    assert_eq!(ws_recv_data(&mut app).await["code"], "bad_request");

    // The queued maximum-size envelope is delivered intact.
    let mut host = p.r.ws(&p.h, &p.h_token).await;
    ws_send(&mut host, json!({ "op": "resume", "after": 0 })).await;
    let v = ws_recv_data(&mut host).await;
    assert_eq!(v["envelope"].as_str().unwrap(), b64url(&max));
}

#[tokio::test]
async fn websocket_requires_the_devices_own_token() {
    let p = pair().await;
    let url = format!(
        "{}/v1/ws?device={}",
        p.r.base.replace("http://", "ws://"),
        p.h
    );
    // No token.
    assert!(tokio_tungstenite::connect_async(url.clone()).await.is_err());
    // Another device's token.
    let mut req = url.clone().into_client_request().unwrap();
    req.headers_mut().insert(
        "authorization",
        format!("Bearer {}", p.a_token).parse().unwrap(),
    );
    assert!(tokio_tungstenite::connect_async(req).await.is_err());
    // Browser style: token in the subprotocol list.
    let mut req = url.into_client_request().unwrap();
    req.headers_mut().insert(
        "sec-websocket-protocol",
        format!("hame-relay.v1, bearer.{}", p.h_token)
            .parse()
            .unwrap(),
    );
    let (mut ws, res) = tokio_tungstenite::connect_async(req).await.unwrap();
    assert_eq!(res.headers()["sec-websocket-protocol"], "hame-relay.v1");
    ws_send(&mut ws, json!({ "op": "ping" })).await;
    assert_eq!(ws_recv_data(&mut ws).await, json!({ "op": "pong" }));
}

#[tokio::test]
async fn offline_mobile_devices_get_an_opaque_push() {
    let r = spawn(|_| {}).await;
    let phone_token = access(&r.account("Cat", "cat@example.com").await);
    let phone = r.device(&phone_token, "ios").await;
    let (s, _) = r
        .post(
            &format!("/v1/devices/{phone}/push"),
            json!({ "provider": "apns", "token": "device-token-1" }),
            Some(&phone_token),
        )
        .await;
    assert_eq!(s, reqwest::StatusCode::NO_CONTENT);
    let (host_token, host) = r.second_device("cat@example.com", "host").await;
    let mut ws = r.ws(&host, &host_token).await;
    ws_send(&mut ws, json!({ "op": "resume", "after": 0 })).await;

    // Without the flag: no push.
    ws_send(
        &mut ws,
        json!({ "op": "send", "to": phone, "envelope": b64url(b"a") }),
    )
    .await;
    assert_eq!(ws_recv_data(&mut ws).await["op"], "sent");
    // With the flag: one push carrying only ids.
    ws_send(
        &mut ws,
        json!({ "op": "send", "to": phone, "envelope": b64url(b"secret words"), "push": true }),
    )
    .await;
    assert_eq!(ws_recv_data(&mut ws).await["seq"], 2);
    for _ in 0..100 {
        if !r.push.messages().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let pushes = r.push.messages();
    assert_eq!(pushes.len(), 1);
    assert_eq!(pushes[0].token, "device-token-1");
    assert_eq!(pushes[0].recipient, phone);
    assert_eq!(pushes[0].sender, host);
    assert_eq!(pushes[0].seq, 2);
}

#[tokio::test]
async fn frames_are_rate_limited_per_connection() {
    let r = spawn(|c| c.rate_limits.ws_frames_per_sec = 2).await;
    let t = access(&r.account("Dan", "dan@example.com").await);
    let d = r.device(&t, "web").await;
    let mut ws = r.ws(&d, &t).await;
    // Burst is four times the rate: 8 frames pass, then refusals.
    for _ in 0..12 {
        ws_send(&mut ws, json!({ "op": "ping" })).await;
    }
    let mut limited = 0;
    for _ in 0..12 {
        let v = ws_recv_data(&mut ws).await;
        if v["code"] == "rate_limited" {
            limited += 1;
        }
    }
    assert!(limited >= 3, "{limited}");
}
