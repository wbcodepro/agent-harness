//! A device that pairs again with its own key (it lost or rolled back its
//! sequence) must not reopen its old envelopes to replay: the host keeps the
//! key's replay window and tells the device, in `paired`, where to continue.
//! (The security review's reproduction, turned into a test.)

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hame_sync::connector::{ConfirmFn, Notice};
use hame_sync::envelope::seal;
use hame_sync::frames::Frame;
use hame_sync::keys::b64;
use hame_sync::pairing::pairing_proof;
use hame_sync::relay::{RelayEvent, RelayOptions, connect, fixed_token};
use hame_sync::{Connector, ConnectorOptions, Header, HostStore, Identity};
use tokio::sync::mpsc;

const HOST: &str = "host1";
const PHONE: &str = "phone1";
const RELAY: &str = "http://127.0.0.1:9";

fn envelope(phone: &Identity, host_x: &[u8; 32], frame: &Frame, counter: u64) -> String {
    let plain = serde_json::to_vec(frame).unwrap();
    let header = Header::new(PHONE, HOST, &frame.stream(), counter);
    seal(phone, host_x, &header, &plain).unwrap()
}

fn pair_frame(phone: &Identity, secret: &[u8; 32]) -> Frame {
    let me = phone.public();
    Frame::Pair {
        ed: b64(&me.ed),
        x: b64(&me.x),
        label: "Phone".into(),
        mac: b64(&pairing_proof(secret, &me)),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pairing_again_with_a_lower_counter_does_not_reopen_old_envelopes() {
    let dir = tempfile::tempdir().unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&hits);
    let router = axum::Router::new().route(
        "/api/hit",
        axum::routing::post(move || {
            let counted = Arc::clone(&counted);
            async move {
                counted.fetch_add(1, Ordering::SeqCst);
                axum::Json(serde_json::json!({}))
            }
        }),
    );
    let notices: Arc<Mutex<Vec<Notice>>> = Arc::default();
    let seen = Arc::clone(&notices);
    let confirm: ConfirmFn = Arc::new(|_| Box::pin(async { true }));
    // Host sends go nowhere (no relay listening); they queue harmlessly.
    let (handle, _ignored) = connect(RelayOptions::new(RELAY, HOST, fixed_token("t")));
    let connector = Connector::new(
        ConnectorOptions {
            device_id: HOST.into(),
            router,
            token: "web".into(),
            store: HostStore::new(dir.path()),
            confirm,
            notice: Arc::new(move |n| seen.lock().unwrap().push(n)),
            grants_changed: None,
            publish_labels: false,
        },
        handle,
    )
    .unwrap();
    let (tx, rx) = mpsc::channel(4096);
    let running = connector.clone();
    tokio::spawn(async move { running.run(rx).await });
    let host_x = connector.public().x;
    let phone = Identity::generate();
    let mut seq = 0u64;
    let mut deliver = |envelope: String| {
        seq += 1;
        let tx = tx.clone();
        async move {
            tx.send(RelayEvent::Deliver {
                seq,
                from: PHONE.into(),
                envelope,
            })
            .await
            .unwrap();
        }
    };
    let paired = |n: &Notice| matches!(n, Notice::Paired { .. });
    let wait = |what: &'static str, done: Box<dyn Fn() -> bool>| async move {
        for _ in 0..200 {
            if done() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("timed out waiting for {what}");
    };

    // 1. Pairing (QR offer, proof, confirmed on the host), counter 1.
    let offer = connector.offer_pairing(RELAY);
    deliver(envelope(
        &phone,
        &host_x,
        &pair_frame(&phone, &offer.secret),
        1,
    ))
    .await;
    let n = Arc::clone(&notices);
    wait(
        "pairing",
        Box::new(move || n.lock().unwrap().iter().filter(|x| paired(x)).count() == 1),
    )
    .await;

    // 2. The phone's normal traffic, counters 2..=40. The relay keeps a copy.
    let mut kept = Vec::new();
    for counter in 2..=40u64 {
        let frame = Frame::Req {
            id: counter,
            method: "POST".into(),
            path: "/api/hit".into(),
            body: None,
        };
        let text = envelope(&phone, &host_x, &frame, counter);
        kept.push(text.clone());
        deliver(text).await;
    }
    let h = Arc::clone(&hits);
    wait(
        "39 requests",
        Box::new(move || h.load(Ordering::SeqCst) == 39),
    )
    .await;

    // Control: replaying now is refused.
    deliver(kept[38].clone()).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        hits.load(Ordering::SeqCst),
        39,
        "replay refused before re-pairing"
    );

    // 3. The phone's storage is restored from an older backup (same keys,
    //    counter back at 2). It can only pair again; the host user confirms.
    let offer = connector.offer_pairing(RELAY);
    deliver(envelope(
        &phone,
        &host_x,
        &pair_frame(&phone, &offer.secret),
        3,
    ))
    .await;
    let n = Arc::clone(&notices);
    wait(
        "re-pairing",
        Box::new(move || n.lock().unwrap().iter().filter(|x| paired(x)).count() == 2),
    )
    .await;

    // 4. The relay replays everything it kept with a counter above 3.
    for text in &kept[2..] {
        deliver(text.clone()).await;
    }
    tokio::time::sleep(Duration::from_millis(800)).await;
    let total = hits.load(Ordering::SeqCst);
    assert_eq!(
        total, 39,
        "old envelopes were accepted again after re-pairing"
    );

    // 5. The device continues above the counter the host reported (40).
    let frame = Frame::Req {
        id: 41,
        method: "POST".into(),
        path: "/api/hit".into(),
        body: None,
    };
    deliver(envelope(&phone, &host_x, &frame, 41)).await;
    let h = Arc::clone(&hits);
    wait(
        "the next request",
        Box::new(move || h.load(Ordering::SeqCst) == 40),
    )
    .await;
    let window = HostStore::new(dir.path()).allowlist().unwrap().devices[0].window;
    assert_eq!(window.last, 41);
}
