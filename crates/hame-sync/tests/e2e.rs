//! End to end: a mock relay, a hame web router with a scripted model, the
//! host connector and a remote client. Pairing with SAS confirmation, a
//! remote turn with streamed events and a bound approval, an artifact in
//! chunks, resumption, and every refusal the design asks for.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use hame_app::{Options, SandboxMode};
use hame_eval::{Recording, ScriptedLlm, ScriptedResponse};
use hame_session::{Delta, StopReason, Usage};
use hame_sync::approval::args_sha256;
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

fn write_call(path: &str) -> ScriptedResponse {
    ScriptedResponse {
        deltas: vec![
            Delta::ToolUseStart {
                id: "c1".into(),
                name: "write".into(),
            },
            Delta::ToolInput {
                json: json!({"path": path, "content": "from afar"}).to_string(),
            },
        ],
        stop: StopReason::ToolUse,
        usage: Usage::default(),
    }
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
    // The digits exist only once the host's nonce arrived.
    let sas = pairing.sas().await?;
    assert_eq!(sas.len(), 8);
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
async fn a_paired_device_runs_a_turn_remotely_and_everything_else_is_refused() {
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
        vec![write_call("a.txt"), ScriptedResponse::text("Written.")],
    )
    .await;

    // Pairing: QR offer, HMAC proof, SAS confirmed on the host, signed grant.
    let offer = host.connector.offer_pairing(&relay.url);
    let offer = PairingOffer::parse(&offer.to_uri()).unwrap();
    let phone = client(
        &relay,
        Identity::generate(),
        &phone_id,
        &host_id,
        &relay.token_for(&phone_id),
        None,
    );
    let grant = pair(&mut host, &phone, &offer, "Phone").await.unwrap();
    assert_eq!(grant.device, phone_id);
    assert_eq!(host.connector.devices().len(), 1);
    assert!(!host.connector.pairing_open(), "an offer is used once");

    // The allowlist and identity are private files.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for name in ["identity.json", "allowlist.json"] {
            let path = dir.path().join("home/sync").join(name);
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "{name}");
        }
    }

    // A remote turn.
    let created = phone
        .request("POST", "/api/sessions", Some(json!({})))
        .await
        .unwrap();
    assert_eq!(created.status, 200, "{:?}", created.body);
    let session = created.body["id"].as_str().unwrap().to_string();
    let events_path = format!("/api/sessions/{session}/events");
    let (sub, mut events) = phone.subscribe(&events_path, -1).await.unwrap();
    let started = phone
        .request(
            "POST",
            &format!("/api/sessions/{session}/prompt"),
            Some(json!({"text": "write a.txt"})),
        )
        .await
        .unwrap();
    assert_eq!(started.status, 200, "{:?}", started.body);
    let before = events_until(&mut events, "the approval question", |e| {
        e["kind"] == "approval"
    })
    .await;
    let question = before.last().unwrap().1.clone();
    assert_eq!(question["tool_call_id"], "c1");
    // The device sees the exact arguments and can recompute the hash over
    // their canonical JSON.
    let arguments = json!({"path": "a.txt", "content": "from afar"});
    assert_eq!(question["arguments"], arguments);
    let hash = args_sha256(&arguments).unwrap();
    assert_eq!(question["args_sha256"], hash.as_str());
    assert!(!dir.path().join("work/a.txt").exists());

    // hame web sees the connector's requests as remote (RemoteCaller): an
    // answer naming other arguments, another call or no hash is refused,
    // and so is a session-wide "always allow".
    let approval = format!("/api/sessions/{session}/approval");
    for wrong in [
        json!({"id": question["id"], "allow": true, "tool_call_id": "c1", "args_sha256": "0".repeat(64)}),
        json!({"id": question["id"], "allow": true, "tool_call_id": "c2", "args_sha256": hash}),
        json!({"id": question["id"], "allow": true, "tool_call_id": "c1"}),
        json!({"id": question["id"], "allow": true, "tool_call_id": "c1", "args_sha256": hash, "always": true}),
    ] {
        let refused = phone
            .request("POST", &approval, Some(wrong.clone()))
            .await
            .unwrap();
        assert!(
            (400..500).contains(&refused.status),
            "{wrong}: {} {:?}",
            refused.status,
            refused.body
        );
    }
    assert!(!dir.path().join("work/a.txt").exists());
    // Nor can the check be dodged with a query string or an encoded path.
    for path in [
        format!("/api/sessions/{session}/approval?via=query"),
        format!("/api/sessions/{session}/%61pproval"),
    ] {
        let dodged = phone
            .request(
                "POST",
                &path,
                Some(json!({"id": question["id"], "allow": true, "tool_call_id": "c1"})),
            )
            .await
            .unwrap();
        assert!(
            (400..500).contains(&dodged.status),
            "{path}: {:?}",
            dodged.body
        );
    }
    assert!(!dir.path().join("work/a.txt").exists());
    let answered = phone
        .request(
            "POST",
            &approval,
            Some(json!({"id": question["id"], "allow": true, "tool_call_id": "c1", "args_sha256": hash})),
        )
        .await
        .unwrap();
    assert_eq!(answered.status, 200, "{:?}", answered.body);
    let rest = events_until(&mut events, "the end of the turn", |e| {
        e["kind"] == "turn" && e["state"] == "ended"
    })
    .await;
    assert_eq!(rest.last().unwrap().1["error"], Value::Null);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("work/a.txt")).unwrap(),
        "from afar"
    );
    let types: Vec<&str> = before
        .iter()
        .chain(&rest)
        .filter_map(|(_, e)| e["record"]["type"].as_str())
        .collect();
    assert!(types.contains(&"tool/call") && types.contains(&"tool/result"));
    assert_eq!(types.last(), Some(&"turn/end"));
    let last_seq = rest.iter().map(|(seq, _)| *seq).max().unwrap();

    // hame web's remote policy: no "Full access", no session outside the
    // remote roots; the connector adds a limit on new sessions.
    let full = phone
        .request(
            "POST",
            "/api/sessions",
            Some(json!({"mode": "full-access"})),
        )
        .await
        .unwrap();
    assert_eq!(full.status, 403, "{:?}", full.body);
    let outside = phone
        .request("POST", "/api/sessions", Some(json!({"cwd": "/"})))
        .await
        .unwrap();
    assert_eq!(outside.status, 403, "{:?}", outside.body);
    let mut limited = false;
    for _ in 0..hame_sync::connector::SESSIONS_PER_MINUTE {
        let created = phone
            .request("POST", "/api/sessions", Some(json!({})))
            .await
            .unwrap();
        if created.status == 429 {
            limited = true;
            break;
        }
        assert_eq!(created.status, 200);
    }
    assert!(limited, "session creation is rate-limited");

    // Cancelling the subscription ends it.
    phone.cancel(sub).await.unwrap();
    loop {
        if let Frame::End { error, .. } = next_frame(&mut events).await {
            assert_eq!(error.as_deref(), Some("cancelled"));
            break;
        }
    }

    // After a dropped connection the client reconnects, and a stream
    // resumed with `after` sends only what is newer.
    relay.kick(&phone_id);
    relay.kick(&host_id);
    let listed = phone.request("GET", "/api/sessions", None).await.unwrap();
    assert_eq!(listed.status, 200);
    let (_, mut resumed) = phone.subscribe(&events_path, last_seq - 2).await.unwrap();
    let replay = events_until(&mut resumed, "the replayed end of turn", |e| {
        e["record"]["type"] == "turn/end"
    })
    .await;
    let seqs: Vec<i64> = replay
        .iter()
        .filter_map(|(_, e)| e["record"]["seq"].as_i64())
        .collect();
    assert_eq!(seqs, vec![last_seq - 1, last_seq]);

    // An artifact larger than one chunk arrives whole and checked.
    let store = hame_session::SessionStore::new(dir.path().join("home/sessions"));
    let mut pdf = b"%PDF-1.4\n".to_vec();
    pdf.extend((0..600_000u32).map(|n| (n % 251) as u8));
    let artifact = store.artifacts().put(&pdf, "application/pdf").unwrap();
    let mut writer = store
        .create(hame_session::CreateOptions {
            id: None,
            cwd: dir.path().join("work").to_string_lossy().into_owned(),
        })
        .unwrap();
    let with_artifact = writer.id().to_string();
    writer
        .append(hame_session::Event::UserMessage {
            content: vec![hame_session::Block::Artifact {
                artifact: artifact.clone(),
            }],
        })
        .unwrap();
    drop(writer);
    let blob = phone
        .fetch_blob(&artifact.sha256, &with_artifact)
        .await
        .unwrap();
    assert_eq!(blob.bytes, pdf);
    assert_eq!(blob.mime, "application/pdf");
    let chunks = relay
        .routed()
        .iter()
        .filter(|r| r.from == host_id && r.to == phone_id)
        .count();
    assert!(chunks > 3);
    // The artifact is not served for a session that does not refer to it.
    assert!(phone.fetch_blob(&artifact.sha256, &session).await.is_err());
    // No route outside the API.
    let outside = phone.request("GET", "/", None).await.unwrap();
    assert_eq!(outside.status, 400);
    let traversal = phone.request("GET", "/api/../x", None).await.unwrap();
    assert_eq!(traversal.status, 400);

    // The relay saw only ciphertext: no prompt, path or file content.
    for routed in relay.routed() {
        let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&routed.envelope)
            .unwrap();
        let text = String::from_utf8_lossy(&raw);
        for secret in [
            "write a.txt",
            "from afar",
            "Written",
            "/api/sessions",
            "%PDF-1.4",
        ] {
            assert!(!text.contains(secret), "the relay could read {secret:?}");
        }
    }

    // --- Refusals ---

    // An unpaired device of the same account is ignored.
    let stranger_id = relay.register(&account, "stranger", "web", None);
    let stranger = client(
        &relay,
        Identity::generate(),
        &stranger_id,
        &host_id,
        &relay.token_for(&stranger_id),
        Some(host.connector.public()),
    );
    silent(&stranger, "GET", "/api/sessions").await;
    host.wait_notice("unknown sender", |n| {
        matches!(n, Notice::Rejected { from, reason } if from == &stranger_id && reason.contains("unknown sender"))
    })
    .await;
    // A second device answering the used pairing code is refused, and
    // not enrolled.
    let late = stranger.start_pairing(&offer, "late").await.unwrap();
    let refused = stranger.paired(late).await.unwrap_err();
    assert!(refused.to_string().contains("already used"), "{refused}");
    host.wait_notice(
        "the refusal",
        |n| matches!(n, Notice::PairingRefused { reason, .. } if reason.contains("already used")),
    )
    .await;
    silent(&stranger, "GET", "/api/sessions").await;
    assert_eq!(host.connector.devices().len(), 1);

    // A replayed envelope is refused: the relay re-queues one it carried.
    let replayed = relay
        .routed()
        .into_iter()
        .rev()
        .find(|r| r.from == phone_id && r.to == host_id)
        .unwrap();
    relay.inject(&phone_id, &host_id, &replayed.envelope);
    host.wait_notice(
        "replay",
        |n| matches!(n, Notice::Rejected { reason, .. } if reason.contains("replayed")),
    )
    .await;

    // A tampered ciphertext is refused.
    let frame = Frame::Req {
        id: 900,
        method: "GET".into(),
        path: "/api/sessions".into(),
        body: None,
    };
    let sealed = phone.seal(&frame).await.unwrap();
    let mut raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(&sealed)
        .unwrap();
    let at = raw.len() - 80;
    raw[at] ^= 0x01;
    phone
        .send_raw(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&raw))
        .await
        .unwrap();
    host.wait_notice("bad signature", |n| {
        matches!(n, Notice::Rejected { from, reason } if from == &phone_id && reason.contains("bad signature"))
    })
    .await;

    // A different key claiming to be the phone is refused, with a warning
    // when it tries to pair.
    let impostor = client(
        &relay,
        Identity::generate(),
        &phone_id,
        &host_id,
        &relay.token_for(&phone_id),
        Some(host.connector.public()),
    );
    silent(&impostor, "GET", "/api/sessions").await;
    let fresh = host.connector.offer_pairing(&relay.url);
    let _ = impostor.start_pairing(&fresh, "Phone").await.unwrap();
    host.wait_notice(
        "key changed",
        |n| matches!(n, Notice::KeyChanged { device } if device == &phone_id),
    )
    .await;
    assert_eq!(
        host.connector.devices()[0].keys().unwrap(),
        phone.public(),
        "the pinned key is unchanged"
    );
    impostor.close();
    relay.kick(&phone_id);

    // The real phone still works (its connection comes back).
    let still = phone.request("GET", "/api/sessions", None).await.unwrap();
    assert_eq!(still.status, 200);

    // A pairing the host user declines enrols nothing.
    let tablet_id = relay.register(&account, "tablet", "android", None);
    let tablet = client(
        &relay,
        Identity::generate(),
        &tablet_id,
        &host_id,
        &relay.token_for(&tablet_id),
        None,
    );
    let offer = host.connector.offer_pairing(&relay.url);
    let refused = pair(&mut host, &tablet, &offer, "Refuse me").await;
    assert!(refused.is_err());
    assert_eq!(host.connector.devices().len(), 1);

    // A second device pairs, then the phone revokes it remotely.
    let offer = host.connector.offer_pairing(&relay.url);
    let laptop_id = relay.register(&account, "other laptop", "desktop", None);
    let laptop_identity = Identity::generate();
    let laptop_keys = laptop_identity.public();
    let laptop = client(
        &relay,
        laptop_identity,
        &laptop_id,
        &host_id,
        &relay.token_for(&laptop_id),
        None,
    );
    pair(&mut host, &laptop, &offer, "Laptop").await.unwrap();
    let listed = phone.control(|id| Frame::Devices { id }).await.unwrap();
    assert_eq!(listed.body["devices"].as_array().unwrap().len(), 2);
    assert_eq!(
        laptop
            .request("GET", "/api/sessions", None)
            .await
            .unwrap()
            .status,
        200
    );
    let revoked = phone
        .control(|id| Frame::Revoke {
            id,
            device: laptop_id.clone(),
        })
        .await
        .unwrap();
    assert_eq!(revoked.status, 200);
    silent(&laptop, "GET", "/api/sessions").await;
    // The revoked key cannot pair again.
    let offer = host.connector.offer_pairing(&relay.url);
    let again = laptop.start_pairing(&offer, "Laptop").await.unwrap();
    assert!(laptop.paired(again).await.is_err());
    assert!(
        host.connector
            .devices()
            .iter()
            .all(|d| d.device != laptop_id)
    );
    let _ = laptop_keys;

    // A reloaded connector keeps its allowlist and counters: the phone's
    // old envelope is still a replay.
    let allowlist = HostStore::new(&dir.path().join("home"))
        .allowlist()
        .unwrap();
    assert_eq!(allowlist.devices.len(), 1);
    assert!(allowlist.devices[0].window.last > 0);
    assert_eq!(allowlist.revoked.len(), 1);
}
