//! `hame connect`: makes this computer a host that signed-in apps can reach
//! through the relay, end-to-end encrypted (docs/design/APPS-AND-SYNC.md).
//!
//! Remote policy, until hame-web enforces its own for requests marked with
//! `RemoteCaller`: the host refuses to start when its configuration would
//! let approvals be answered without the user (`yes`, the reviewer model,
//! `[approval] default = "allow"`) or would run tools with full access;
//! remote requests asking for full access are refused by the connector.

use std::io::{BufRead, Write};
use std::path::Path;
use std::process::ExitCode;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use clap::{Args, Subcommand};
use hame_sync::connector::{ConfirmFn, Notice, device_fingerprint};
use hame_sync::relay::{RelayOptions, TokenSource};
use hame_sync::relay_api::RelayApi;
use hame_sync::store::RelayConfig;
use hame_sync::text::{printable, valid_id};
use hame_sync::{Connector, ConnectorOptions, HostStore, PairingPrompt, SyncError};

#[derive(Args)]
#[command(args_conflicts_with_subcommands = true)]
pub struct ConnectArgs {
    #[command(subcommand)]
    command: Option<ConnectCommand>,
    /// The relay to use. Defaults to the one this host is registered with,
    /// then $HAME_RELAY_URL. Must be https (http only for this machine).
    #[arg(long)]
    relay: Option<String>,
    /// Show a pairing code to add a device, even if devices are enrolled.
    #[arg(long)]
    pair: bool,
    /// Also print the pairing code as text. It holds the pairing secret:
    /// anyone who reads it within ten minutes can ask to pair.
    #[arg(long)]
    show_uri: bool,
    /// Include device labels in the device list stored at the relay
    /// (by default the relay gets ids and keys only).
    #[arg(long)]
    publish_labels: bool,
    /// The name this host shows in the apps. Defaults to the computer's.
    #[arg(long)]
    name: Option<String>,
}

#[derive(Subcommand)]
enum ConnectCommand {
    /// List the devices that may control this host.
    Devices,
    /// Remove a device: its key is refused from now on, also by a running
    /// `hame connect`, which stops its streams and cancels its turns.
    Revoke { device: String },
}

fn display(error: impl std::fmt::Display) -> String {
    error.to_string()
}

pub async fn connect(home: &Path, args: ConnectArgs) -> Result<ExitCode, String> {
    let store = HostStore::new(home);
    match args.command {
        Some(ConnectCommand::Devices) => {
            let allowlist = store.allowlist().map_err(display)?;
            if allowlist.devices.is_empty() {
                println!("no devices; `hame connect --pair` adds one");
            }
            for device in &allowlist.devices {
                println!(
                    "{}\t{}\t{}",
                    printable(&device.device),
                    printable(&device.label),
                    device_fingerprint(device)
                );
            }
            return Ok(ExitCode::SUCCESS);
        }
        Some(ConnectCommand::Revoke { device }) => {
            if !valid_id(&device) {
                return Err("not a device id".into());
            }
            // Under the store's lock, and logged in the append-only
            // revocation list: a running connector cannot undo it.
            let (_, revoked) = store
                .update_allowlist(|list| Ok(list.revoke(&device)))
                .map_err(display)?;
            if !revoked {
                return Err(format!("no enrolled device {device}"));
            }
            println!("revoked {device}");
            return Ok(ExitCode::SUCCESS);
        }
        None => {}
    }

    check_remote_policy(home)?;
    // The keys first: this refuses to go on with readable key files.
    let identity = store.identity().map_err(display)?;
    let name = args.name.clone().unwrap_or_else(host_name);
    let wanted = args
        .relay
        .clone()
        .or_else(|| std::env::var("HAME_RELAY_URL").ok());
    let config = match (store.relay().map_err(display)?, wanted) {
        (Some(config), None) => config,
        (Some(config), Some(url)) if same_url(&config.url, &url) => config,
        (_, Some(url)) => sign_in(&store, &url, &name, &identity.public()).await?,
        (None, None) => {
            return Err("no relay configured: pass --relay <url> or set HAME_RELAY_URL".into());
        }
    };
    drop(identity);
    run(home, store, config, &args).await
}

/// Refuses configurations that would answer remote approvals without the
/// user or run remote turns with full access (interim, see the module
/// documentation).
fn check_remote_policy(home: &Path) -> Result<(), String> {
    let config = hame_app::load_config(home)?;
    if config.approval.reviewer {
        return Err(
            "[approval] reviewer = true lets a model answer approval questions; hame connect \
             does not run with it until the remote policy lands. Turn it off to connect."
                .into(),
        );
    }
    if config.approval.default == Some(hame_policy::RuleAction::Allow) {
        return Err(
            "[approval] default = \"allow\" would let remote turns run without asking; hame \
             connect does not run with it."
                .into(),
        );
    }
    if config.sandbox.mode == hame_app::SandboxMode::DangerFullAccess {
        return Err(
            "the sandbox is configured for full access; hame connect does not give remote \
             devices full access. Choose a confined sandbox mode to connect."
                .into(),
        );
    }
    Ok(())
}

fn same_url(a: &str, b: &str) -> bool {
    a.trim_end_matches('/') == b.trim_end_matches('/')
}

fn host_name() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "hame host".into())
}

/// Device-code sign-in, then registration of this host's public keys.
async fn sign_in(
    store: &HostStore,
    url: &str,
    name: &str,
    keys: &hame_sync::PublicIdentity,
) -> Result<RelayConfig, String> {
    let api = RelayApi::new(url).map_err(display)?;
    let code = api.device_start(name).await.map_err(display)?;
    // Everything the relay says is printed without control or
    // bidirectional formatting characters.
    eprintln!(
        "hame connect: to sign in, open {} in a signed-in app or browser and enter the code\n\n    {}\n",
        printable(&code.verification_url),
        printable(&code.user_code)
    );
    if !code.verification_url_complete.is_empty() {
        eprintln!(
            "hame connect: or open {} directly",
            printable(&code.verification_url_complete)
        );
    }
    let tokens = api.device_wait(&code).await.map_err(display)?;
    let device_id = api
        .register_host(&tokens.access_token, name, keys)
        .await
        .map_err(display)?;
    if !valid_id(&device_id) {
        return Err("the relay assigned an invalid device id".into());
    }
    let config = RelayConfig {
        url: url.trim_end_matches('/').into(),
        device_id,
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
    };
    store.save_relay(&config).map_err(display)?;
    eprintln!(
        "hame connect: signed in; this host is {} (key {})",
        config.device_id,
        keys.display_fingerprint()
    );
    Ok(config)
}

/// The host's relay tokens: the stored access token first, then a refresh
/// (rotating the refresh token) for every reconnection and renewal. One
/// refresh at a time, and the rotated pair is saved before it is used. A
/// failed refresh is an error, never a silent fallback to an old token.
#[derive(Clone)]
struct Tokens {
    store: HostStore,
    config: Arc<tokio::sync::Mutex<RelayConfig>>,
    fresh: Arc<std::sync::atomic::AtomicBool>,
}

impl Tokens {
    fn new(store: HostStore, config: RelayConfig) -> Self {
        Self {
            store,
            config: Arc::new(tokio::sync::Mutex::new(config)),
            fresh: Arc::new(std::sync::atomic::AtomicBool::new(true)),
        }
    }

    async fn current(&self) -> String {
        self.config.lock().await.access_token.clone()
    }

    /// A token for a new connection or an `auth` renewal.
    async fn next(&self) -> Result<String, SyncError> {
        if self.fresh.swap(false, std::sync::atomic::Ordering::SeqCst) {
            return Ok(self.current().await);
        }
        self.refresh().await
    }

    async fn refresh(&self) -> Result<String, SyncError> {
        let mut config = self.config.lock().await;
        if config.refresh_token.is_empty() {
            return Err(SyncError::Unauthorized);
        }
        let tokens = RelayApi::new(&config.url)?
            .refresh(&config.refresh_token)
            .await?;
        let mut updated = config.clone();
        updated.access_token = tokens.access_token.clone();
        if !tokens.refresh_token.is_empty() {
            updated.refresh_token = tokens.refresh_token;
        }
        self.store.save_relay(&updated)?;
        *config = updated;
        Ok(tokens.access_token)
    }

    fn source(&self) -> TokenSource {
        let tokens = self.clone();
        Arc::new(move || {
            let tokens = tokens.clone();
            Box::pin(async move {
                let result = tokens.next().await;
                if let Err(error) = &result {
                    eprintln!("hame connect: {}", printable(&error.to_string()));
                }
                result
            })
        })
    }
}

/// Publishes a grant list: refreshes the token when it expired, publishes
/// again above the relay's version when it holds a newer one, and retries a
/// few times on other failures.
async fn publish(
    api: RelayApi,
    tokens: Tokens,
    host: String,
    grants: hame_sync::pairing::SignedGrantList,
    connector: Arc<OnceLock<Connector>>,
) {
    for attempt in 0..4u32 {
        let token = tokens.current().await;
        match api.post_grants(&token, &host, &grants).await {
            Ok(None) => return,
            Ok(Some(stored)) => {
                // Publish again with a version above the relay's.
                if let Some(connector) = connector.get() {
                    connector.publish_grants_above(stored);
                }
                return;
            }
            Err(SyncError::Unauthorized) => {
                if tokens.refresh().await.is_err() {
                    eprintln!("hame connect: could not publish the device list: signed out");
                    return;
                }
            }
            Err(error) => {
                if attempt == 3 {
                    eprintln!(
                        "hame connect: could not publish the device list: {}",
                        printable(&error.to_string())
                    );
                    return;
                }
                tokio::time::sleep(Duration::from_secs(1 << attempt)).await;
            }
        }
    }
}

async fn run(
    home: &Path,
    store: HostStore,
    config: RelayConfig,
    args: &ConnectArgs,
) -> Result<ExitCode, String> {
    let cwd = std::env::current_dir()
        .and_then(|cwd| hame_kernel::real_path(&cwd))
        .map_err(display)?;
    // Approvals are never answered automatically for a remote host.
    let options = hame_app::Options {
        max_steps: 0,
        yes: false,
        ..hame_app::Options::default()
    };
    let (router, web_token) = hame_web::router(home.to_path_buf(), cwd.clone(), options);
    let tokens = Tokens::new(store.clone(), config.clone());
    let (relay, events) = hame_sync::relay::connect(RelayOptions::new(
        &config.url,
        &config.device_id,
        tokens.source(),
    ));
    let api = RelayApi::new(&config.url).map_err(display)?;
    let host_id = config.device_id.clone();
    let cell: Arc<OnceLock<Connector>> = Arc::new(OnceLock::new());
    let publisher = {
        let tokens = tokens.clone();
        let cell = Arc::clone(&cell);
        move |grants| {
            tokio::spawn(publish(
                api.clone(),
                tokens.clone(),
                host_id.clone(),
                grants,
                Arc::clone(&cell),
            ));
        }
    };
    let connector = Connector::new(
        ConnectorOptions {
            device_id: config.device_id.clone(),
            router,
            token: web_token,
            store: store.clone(),
            confirm: terminal_confirm(),
            notice: Arc::new(print_notice),
            grants_changed: Some(Arc::new(publisher)),
            publish_labels: args.publish_labels,
        },
        relay.clone(),
    )
    .map_err(display)?;
    let _ = cell.set(connector.clone());

    eprintln!(
        "hame connect: host {} ({}) serving sessions in {}",
        config.device_id,
        connector.public().display_fingerprint(),
        cwd.display()
    );
    if args.pair || connector.devices().is_empty() {
        let offer = connector.offer_pairing(&config.url);
        let uri = offer.to_uri();
        eprintln!("\nScan this code with the hame app to add a device (valid 10 minutes, once):\n");
        eprintln!("{}", render_qr(&uri));
        if args.show_uri {
            eprintln!(
                "Pairing code (secret: anyone who reads it in the next ten minutes can ask to pair):"
            );
            eprintln!("{uri}\n");
        }
    }
    eprintln!("hame connect: Ctrl-C stops");
    tokio::select! {
        _ = connector.run(events) => {}
        _ = tokio::signal::ctrl_c() => {}
    }
    relay.stop();
    Ok(ExitCode::SUCCESS)
}

/// A QR code drawn with half blocks, two modules per character cell.
pub fn render_qr(text: &str) -> String {
    match qrcode::QrCode::with_error_correction_level(text, qrcode::EcLevel::L) {
        Ok(code) => code
            .render::<qrcode::render::unicode::Dense1x2>()
            .dark_color(qrcode::render::unicode::Dense1x2::Light)
            .light_color(qrcode::render::unicode::Dense1x2::Dark)
            .quiet_zone(true)
            .build(),
        Err(e) => format!("(cannot draw a QR code: {e})"),
    }
}

/// Prints a notice; every string in it came from the relay or a device,
/// so control and bidirectional formatting characters are removed.
fn print_notice(notice: Notice) {
    let p = |s: &str| printable(s);
    match notice {
        Notice::Rejected { from, reason } => eprintln!(
            "hame connect: dropped an envelope from {}: {}",
            p(&from),
            p(&reason)
        ),
        Notice::Paired { device, label } => {
            eprintln!("hame connect: paired {} ({})", p(&label), p(&device))
        }
        Notice::PairingRefused { device, reason } => eprintln!(
            "hame connect: pairing with {} refused: {}",
            p(&device),
            p(&reason)
        ),
        Notice::KeyChanged { device } => eprintln!(
            "hame connect: WARNING: device {} presented a different key than the one it was \
             paired with. Refused. If you reset that device, revoke it and pair again.",
            p(&device)
        ),
        Notice::Revoked { device, by } => {
            eprintln!(
                "hame connect: device {} revoked (by {})",
                p(&device),
                p(&by)
            )
        }
        Notice::Relay(message) => eprintln!("hame connect: relay {}", p(&message)),
    }
}

/// Groups eight digits as "1234 5678".
fn grouped(sas: &str) -> String {
    if sas.len() == 8 {
        format!("{} {}", &sas[..4], &sas[4..])
    } else {
        sas.to_string()
    }
}

/// Asks on the terminal whether the digits match the device's screen.
fn terminal_confirm() -> ConfirmFn {
    Arc::new(|prompt: PairingPrompt| {
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let mut out = std::io::stderr();
                let _ = write!(
                    out,
                    "\nDevice \"{}\" ({}) asks to control this host.\nKey fingerprint: {}\n\
                     Does the device show the code {}?\nAllow it? [y/N] ",
                    printable(&prompt.label),
                    printable(&prompt.device),
                    prompt.fingerprint,
                    grouped(&prompt.sas)
                );
                let _ = out.flush();
                let mut answer = String::new();
                if std::io::stdin().lock().read_line(&mut answer).is_err() {
                    return false;
                }
                matches!(answer.trim(), "y" | "Y" | "yes" | "Yes")
            })
            .await
            .unwrap_or(false)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pairing_code_draws_as_a_qr_code() {
        let drawn = render_qr("hame-pair:v1?relay=https%3A%2F%2Fr&host=h&exp=1");
        assert!(drawn.lines().count() > 10);
        assert!(drawn.contains('█') || drawn.contains('▀') || drawn.contains('▄'));
    }

    #[test]
    fn digits_are_grouped() {
        assert_eq!(grouped("12345678"), "1234 5678");
    }

    #[test]
    fn auto_approving_or_full_access_configurations_are_refused() {
        let home = tempfile::tempdir().unwrap();
        assert!(check_remote_policy(home.path()).is_ok());
        for config in [
            "[approval]\nreviewer = true\n",
            "[approval]\ndefault = \"allow\"\n",
            "[sandbox]\nmode = \"danger-full-access\"\n",
        ] {
            std::fs::write(home.path().join("config.toml"), config).unwrap();
            assert!(check_remote_policy(home.path()).is_err(), "{config}");
        }
    }
}
