//! `hame connect` against the mock relay: device-code sign-in, host
//! registration, the pairing QR code, SAS confirmation on the terminal, a
//! remote request, the published grant list, and the `devices` / `revoke`
//! subcommands.

use std::process::Stdio;
use std::time::Duration;

use hame_sync::client::RemoteClient;
use hame_sync::keys::b64;
use hame_sync::mock_relay::MockRelay;
use hame_sync::relay::{RelayOptions, connect, fixed_token};
use hame_sync::{Identity, PairingOffer};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

fn hame(home: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hame"));
    command
        .arg("--home")
        .arg(home)
        .env_remove("HAME_RELAY_URL")
        .current_dir(home);
    command
}

async fn line_with(
    lines: &mut tokio::io::Lines<BufReader<tokio::process::ChildStderr>>,
    needle: &str,
    seen: &mut Vec<String>,
) -> String {
    loop {
        let line = tokio::time::timeout(Duration::from_secs(30), lines.next_line())
            .await
            .unwrap_or_else(|_| panic!("no line with {needle:?}; saw {seen:#?}"))
            .unwrap()
            .unwrap_or_else(|| panic!("hame connect exited; saw {seen:#?}"));
        seen.push(line.clone());
        if line.contains(needle) {
            return line;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hame_connect_signs_in_pairs_a_device_and_serves_it() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let relay = MockRelay::start().await;
    let account = relay.account();
    relay.auto_approve(&account);

    let mut child = hame(&home)
        .args(["connect", "--relay", &relay.url, "--name", "test host"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut lines = BufReader::new(child.stderr.take().unwrap()).lines();
    let mut seen = Vec::new();
    line_with(&mut lines, "enter the code", &mut seen).await;
    line_with(&mut lines, "signed in", &mut seen).await;
    line_with(&mut lines, "Scan this code", &mut seen).await;
    let uri = line_with(&mut lines, "or enter: ", &mut seen).await;
    assert!(
        seen.iter()
            .any(|l| l.contains('▀') || l.contains('▄') || l.contains('█')),
        "a QR code is drawn"
    );
    let offer = PairingOffer::parse(uri.split("or enter: ").nth(1).unwrap().trim()).unwrap();
    assert!(offer.relay.starts_with("http://127.0.0.1:"));

    // The relay knows the host's public keys, nothing more.
    let registered = relay.device(&offer.host).unwrap();
    assert_eq!(registered.kind, "host");
    assert_eq!(registered.ed25519, b64(&offer.host_keys.ed));
    assert_eq!(registered.x25519, b64(&offer.host_keys.x));

    let phone_id = relay.register(&account, "phone", "ios", None);
    let mut options = RelayOptions::new(
        &relay.url,
        &phone_id,
        fixed_token(relay.token_for(&phone_id)),
    );
    options.backoff_base = Duration::from_millis(20);
    let (handle, events) = connect(options);
    let phone = RemoteClient::new(
        Identity::generate(),
        &phone_id,
        &offer.host,
        None,
        handle,
        events,
    );
    let (sas, answered) = phone.start_pairing(&offer, "My phone").await.unwrap();
    let question = line_with(&mut lines, "Does the device show the code", &mut seen).await;
    assert!(question.contains(&sas), "{question} shows {sas}");
    stdin.write_all(b"y\n").await.unwrap();
    stdin.flush().await.unwrap();
    let grant = phone.paired(answered).await.unwrap();
    assert_eq!(grant.label, "My phone");
    line_with(&mut lines, "paired My phone", &mut seen).await;

    let listed = phone.request("GET", "/api/sessions", None).await.unwrap();
    assert_eq!(listed.status, 200, "{:?}", listed.body);

    // The host published its signed list of accepted devices.
    let mut published = None;
    for _ in 0..100 {
        published = relay.grants(&offer.host);
        if published.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let list = published
        .expect("a grant list")
        .verify(&offer.host_keys.ed)
        .unwrap();
    assert_eq!(list.devices.len(), 1);
    assert_eq!(list.devices[0].device, phone_id);

    // Revoking from another terminal takes effect in the running host.
    let output = hame(&home)
        .args(["connect", "devices"])
        .output()
        .await
        .unwrap();
    let devices = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        devices.contains(&phone_id) && devices.contains("My phone"),
        "{devices}"
    );
    let output = hame(&home)
        .args(["connect", "revoke", &phone_id])
        .output()
        .await
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    line_with(&mut lines, "revoked", &mut seen).await;
    let refused = phone
        .request_within("GET", "/api/sessions", None, Duration::from_millis(1500))
        .await;
    assert!(refused.is_err(), "a revoked device gets no answer");
    let output = hame(&home)
        .args(["connect", "devices"])
        .output()
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&output.stdout).contains("no devices"));

    child.kill().await.unwrap();

    // Key files readable by others: refused.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let identity = home.join("sync/identity.json");
        std::fs::set_permissions(&identity, std::fs::Permissions::from_mode(0o644)).unwrap();
        let output = hame(&home).arg("connect").output().await.unwrap();
        assert!(!output.status.success());
        let message = String::from_utf8_lossy(&output.stderr);
        assert!(message.contains("chmod 600"), "{message}");
    }
}
