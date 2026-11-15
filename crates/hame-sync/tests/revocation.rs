//! Revocation from another process (`hame connect revoke`) while the device
//! is active: it is never lost, the device's envelopes are refused at once,
//! its streams stop and the turns it started are cancelled. The first test
//! is the security review's reproduction of the lost-revocation race.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use hame_app::{Options, SandboxMode};
use hame_eval::{Recording, ScriptedLlm, ScriptedResponse};
use hame_session::{Delta, StopReason, Usage};
use hame_sync::client::RemoteClient;
use hame_sync::connector::{ConfirmFn, Notice};
use hame_sync::frames::Frame;
use hame_sync::mock_relay::MockRelay;
use hame_sync::relay::{RelayOptions, connect, fixed_token};
use hame_sync::{Connector, ConnectorOptions, HostStore, Identity, PairingOffer};
use hame_tools::InProcess;
use serde_json::{Value, json};
use tokio::sync::mpsc;

fn fast(url: &str, device: &str, token: &str) -> RelayOptions {
    let mut options = RelayOptions::new(url, device, fixed_token(token));
    options.backoff_base = Duration::from_millis(20);
    options.backoff_max = Duration::from_millis(200);
    options
}

struct Host {
    connector: Connector,
    notices: Arc<Mutex<Vec<Notice>>>,
    prompts: mpsc::UnboundedReceiver<hame_sync::PairingPrompt>,
}

impl Host {
    fn notices(&self) -> Vec<Notice> {
        self.notices.lock().unwrap().clone()
    }

    async fn wait_notice(&self, what: &str, matches: impl Fn(&Notice) -> bool) -> Notice {
        for _ in 0..200 {
            if let Some(found) = self.notices().into_iter().find(|n| matches(n)) {
                return found;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("no notice {what}; saw {:?}", self.notices());
    }
}

async fn start_host(
    relay: &MockRelay,
    home: &std::path::Path,
    host_id: &str,
    token: &str,
    responses: Vec<ScriptedResponse>,
) -> Host {
    let work = home.join("work");
    std::fs::create_dir_all(&work).unwrap();
    let options = Options {
        // As `hame connect` builds remote sessions.
        sandbox: Some(SandboxMode::WorkspaceWrite),
        ask_before_changes: true,
        max_steps: 10,
        llm: Some(Arc::new(ScriptedLlm::new(Recording { responses }))),
        runner: Some(Arc::new(InProcess)),
        ..Options::default()
    };
    let (router, web_token) = hame_web::router(home.join("home"), work, options);
    let notices = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&notices);
    let (prompt_tx, prompts) = mpsc::unbounded_channel();
    // The host user confirms every pairing except one labelled "Refuse me".
    let confirm: ConfirmFn = Arc::new(move |prompt: hame_sync::PairingPrompt| {
        let accept = prompt.label != "Refuse me";
        let _ = prompt_tx.send(prompt);
        Box::pin(async move { accept })
    });
    let (handle, events) = connect(fast(&relay.url, host_id, token));
    let connector = Connector::new(
        ConnectorOptions {
            device_id: host_id.into(),
            router,
            token: web_token,
            store: HostStore::new(&home.join("home")),
            confirm,
            notice: Arc::new(move |notice| seen.lock().unwrap().push(notice)),
            grants_changed: None,
            publish_labels: false,
        },
        handle,
    )
    .unwrap();
    let running = connector.clone();
    tokio::spawn(async move { running.run(events).await });
    Host {
        connector,
        notices,
        prompts,
    }
}

fn client(
    relay: &MockRelay,
    identity: Identity,
    device: &str,
    host_id: &str,
    token: &str,
    host_keys: Option<hame_sync::PublicIdentity>,
) -> RemoteClient {
    let (handle, events) = connect(fast(&relay.url, device, token));
    RemoteClient::new(identity, device, host_id, host_keys, handle, events)
}

async fn next_frame(rx: &mut mpsc::UnboundedReceiver<Frame>) -> Frame {
    tokio::time::timeout(Duration::from_secs(20), rx.recv())
        .await
        .expect("a frame in time")
        .expect("the stream stays open")
}

async fn events_until(
    rx: &mut mpsc::UnboundedReceiver<Frame>,
    what: &str,
    done: impl Fn(&Value) -> bool,
) -> Vec<(i64, Value)> {
    let mut seen = Vec::new();
    for _ in 0..5000 {
        match next_frame(rx).await {
            Frame::Evt { seq, data, .. } => {
                let stop = done(&data);
                seen.push((seq, data));
                if stop {
                    return seen;
                }
            }
            Frame::End { error, .. } => panic!("stream ended ({error:?}) before {what}"),
            _ => {}
        }
    }
    panic!("never saw {what}");
}

async fn pair(
    host: &mut Host,
    remote: &RemoteClient,
    offer: &PairingOffer,
    label: &str,
) -> Result<hame_sync::pairing::Grant, hame_sync::SyncError> {
    let mut pairing = remote.start_pairing(offer, label).await?;
    let sas = pairing.sas().await?;
    let prompt = tokio::time::timeout(Duration::from_secs(10), host.prompts.recv())
        .await
        .expect("the host asks")
        .unwrap();
    assert_eq!(prompt.sas, sas, "both screens show the same digits");
    assert_eq!(prompt.label, label);
    assert_eq!(prompt.fingerprint, remote.public().display_fingerprint());
    remote.paired(pairing).await
}

/// Nothing comes back within a while.
async fn silent(remote: &RemoteClient, method: &str, path: &str) {
    let answer = remote
        .request_within(method, path, None, Duration::from_millis(800))
        .await;
    assert!(answer.is_err(), "expected no answer, got {answer:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn review_revocation_from_cli_is_lost_while_device_is_active() {
    let dir = tempfile::tempdir().unwrap();
    let relay = MockRelay::start().await;
    let account = relay.account();
    let host_id = relay.register(&account, "laptop", "host", None);
    let phone_id = relay.register(&account, "phone", "ios", None);
    let mut host = start_host(
        &relay,
        dir.path(),
        &host_id,
        &relay.token_for(&host_id),
        vec![],
    )
    .await;
    let offer = host.connector.offer_pairing(&relay.url);
    let offer = PairingOffer::parse(&offer.to_uri()).unwrap();
    let phone = Arc::new(client(
        &relay,
        Identity::generate(),
        &phone_id,
        &host_id,
        &relay.token_for(&phone_id),
        None,
    ));
    pair(&mut host, &phone, &offer, "Phone").await.unwrap();

    // The (compromised) phone keeps sending cheap frames.
    let flooder = Arc::clone(&phone);
    let flood = tokio::spawn(async move {
        for id in 0..20_000u64 {
            if flooder.send(Frame::Devices { id }).await.is_err() {
                break;
            }
        }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    // `hame connect revoke <phone>` from another terminal, exactly as connect.rs does it.
    let store = HostStore::new(&dir.path().join("home"));
    let mut lost = 0;
    let attempts = 20;
    for _ in 0..attempts {
        let mut list = store.allowlist().unwrap();
        if list.get(&phone_id).is_none() {
            break;
        }
        assert!(list.revoke(&phone_id));
        store.save_allowlist(&list).unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        if store.allowlist().unwrap().get(&phone_id).is_some() {
            lost += 1; // the running connector wrote its stale copy back
        } else {
            break;
        }
    }
    flood.abort();
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let still = store.allowlist().unwrap().get(&phone_id).is_some();
    eprintln!("REVIEW: revocations overwritten: {lost}; device still enrolled at end: {still}");
    assert_eq!(
        lost, 0,
        "a CLI revocation was silently undone by the running connector"
    );
}

fn slow_text() -> ScriptedResponse {
    ScriptedResponse {
        deltas: (0..2000)
            .map(|n| Delta::Text {
                text: format!("w{n} "),
            })
            .collect(),
        stop: StopReason::EndTurn,
        usage: Usage::default(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_revoked_device_loses_its_streams_and_its_running_turn_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let relay = MockRelay::start().await;
    let account = relay.account();
    let host_id = relay.register(&account, "laptop", "host", None);
    let phone_id = relay.register(&account, "phone", "ios", None);
    let work = dir.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    // A slow turn, so it is still running when the device is revoked.
    let options = Options {
        // As `hame connect` builds remote sessions.
        sandbox: Some(SandboxMode::WorkspaceWrite),
        ask_before_changes: true,
        max_steps: 10,
        llm: Some(Arc::new(
            ScriptedLlm::new(Recording {
                responses: vec![slow_text()],
            })
            .with_delay(Duration::from_millis(5)),
        )),
        runner: Some(Arc::new(InProcess)),
        ..Options::default()
    };
    let (router, web_token) = hame_web::router(dir.path().join("home"), work, options);
    let notices = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&notices);
    let (prompt_tx, prompts) = mpsc::unbounded_channel();
    let confirm: ConfirmFn = Arc::new(move |prompt: hame_sync::PairingPrompt| {
        let _ = prompt_tx.send(prompt);
        Box::pin(async move { true })
    });
    let (handle, events) = connect(fast(&relay.url, &host_id, &relay.token_for(&host_id)));
    let connector = Connector::new(
        ConnectorOptions {
            device_id: host_id.clone(),
            router,
            token: web_token,
            store: HostStore::new(&dir.path().join("home")),
            confirm,
            notice: Arc::new(move |notice| seen.lock().unwrap().push(notice)),
            grants_changed: None,
            publish_labels: false,
        },
        handle,
    )
    .unwrap();
    let running = connector.clone();
    tokio::spawn(async move { running.run(events).await });
    let mut host = Host {
        connector,
        notices,
        prompts,
    };
    let offer = host.connector.offer_pairing(&relay.url);
    let phone = client(
        &relay,
        Identity::generate(),
        &phone_id,
        &host_id,
        &relay.token_for(&phone_id),
        None,
    );
    pair(&mut host, &phone, &offer, "Phone").await.unwrap();

    let created = phone
        .request("POST", "/api/sessions", Some(json!({})))
        .await
        .unwrap();
    let session = created.body["id"].as_str().unwrap().to_string();
    let events_path = format!("/api/sessions/{session}/events");

    // At most eight subscriptions at once.
    let mut subs = Vec::new();
    for _ in 0..hame_sync::connector::MAX_SUBSCRIPTIONS_PER_DEVICE {
        subs.push(phone.subscribe(&events_path, -1).await.unwrap());
    }
    let (_, mut extra) = phone.subscribe(&events_path, -1).await.unwrap();
    loop {
        if let Frame::End { error, .. } = next_frame(&mut extra).await {
            assert_eq!(error.as_deref(), Some("too many subscriptions"));
            break;
        }
    }
    let (_, mut stream) = subs.pop().unwrap();
    for (id, _) in &subs {
        phone.cancel(*id).await.unwrap();
    }

    let started = phone
        .request(
            "POST",
            &format!("/api/sessions/{session}/prompt"),
            Some(json!({"text": "talk"})),
        )
        .await
        .unwrap();
    assert_eq!(started.status, 200);
    events_until(&mut stream, "streamed text", |e| {
        e["record"]["type"] == "assistant/delta"
    })
    .await;

    // `hame connect revoke <phone>` in another terminal.
    let store = HostStore::new(&dir.path().join("home"));
    store
        .update_allowlist(|list| Ok(list.revoke(&phone_id)))
        .unwrap();
    host.wait_notice(
        "the revocation",
        |n| matches!(n, Notice::Revoked { device, .. } if device == &phone_id),
    )
    .await;

    // The stream stops (no more events after a short drain), the turn is
    // cancelled, and the device gets no answer any more.
    tokio::time::sleep(Duration::from_millis(300)).await;
    while stream.try_recv().is_ok() {}
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(stream.try_recv().is_err(), "the stream stopped");
    let records = hame_session::SessionStore::new(dir.path().join("home/sessions"))
        .read(&hame_session::SessionId::parse(&session).unwrap())
        .unwrap();
    let ended = records
        .iter()
        .filter_map(|r| serde_json::to_value(r).ok())
        .any(|r| r["type"] == "turn/end" && r["data"]["reason"] == "cancelled");
    assert!(ended, "the device's turn was cancelled");
    silent(&phone, "GET", "/api/sessions").await;
    assert!(host.connector.devices().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unanswered_pairing_prompt_is_a_refusal() {
    let dir = tempfile::tempdir().unwrap();
    let relay = MockRelay::start().await;
    let account = relay.account();
    let host_id = relay.register(&account, "laptop", "host", None);
    let phone_id = relay.register(&account, "phone", "ios", None);
    let (router, web_token) = hame_web::router(
        dir.path().join("home"),
        dir.path().to_path_buf(),
        Options::default(),
    );
    // The host user never answers.
    let confirm: ConfirmFn = Arc::new(|_| Box::pin(std::future::pending::<bool>()));
    let (handle, events) = connect(fast(&relay.url, &host_id, &relay.token_for(&host_id)));
    let connector = Connector::new(
        ConnectorOptions {
            device_id: host_id.clone(),
            router,
            token: web_token,
            store: HostStore::new(&dir.path().join("home")),
            confirm,
            notice: Arc::new(|_| {}),
            grants_changed: None,
            publish_labels: false,
        },
        handle,
    )
    .unwrap();
    connector.set_pairing_prompt_limit(Duration::from_millis(300));
    let running = connector.clone();
    tokio::spawn(async move { running.run(events).await });
    let offer = connector.offer_pairing(&relay.url);
    let phone = client(
        &relay,
        Identity::generate(),
        &phone_id,
        &host_id,
        &relay.token_for(&phone_id),
        None,
    );
    let mut pairing = phone.start_pairing(&offer, "Phone").await.unwrap();
    pairing.sas().await.unwrap();
    let refused = tokio::time::timeout(Duration::from_secs(10), phone.paired(pairing))
        .await
        .expect("an answer in time");
    assert!(refused.is_err());
    assert!(connector.devices().is_empty());
}
