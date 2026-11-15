//! `hame connect`: makes this computer a host that signed-in apps can reach
//! through the relay, end-to-end encrypted (docs/design/APPS-AND-SYNC.md).

use std::io::{BufRead, Write};
use std::path::Path;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use clap::{Args, Subcommand};
use hame_sync::connector::{ConfirmFn, Notice, device_fingerprint};
use hame_sync::relay::{RelayOptions, TokenSource};
use hame_sync::relay_api::RelayApi;
use hame_sync::store::RelayConfig;
use hame_sync::{Connector, ConnectorOptions, HostStore, PairingPrompt};

#[derive(Args)]
#[command(args_conflicts_with_subcommands = true)]
pub struct ConnectArgs {
    #[command(subcommand)]
    command: Option<ConnectCommand>,
    /// The relay to use. Defaults to the one this host is registered with,
    /// then $HAME_RELAY_URL.
    #[arg(long)]
    relay: Option<String>,
    /// Show a pairing code to add a device, even if devices are enrolled.
    #[arg(long)]
    pair: bool,
    /// The name this host shows in the apps. Defaults to the computer's.
    #[arg(long)]
    name: Option<String>,
}

#[derive(Subcommand)]
enum ConnectCommand {
    /// List the devices that may control this host.
    Devices,
    /// Remove a device: its key is refused from now on, also by a running
    /// `hame connect`.
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
                    device.device,
                    device.label,
                    device_fingerprint(device)
                );
            }
            return Ok(ExitCode::SUCCESS);
        }
        Some(ConnectCommand::Revoke { device }) => {
            let mut allowlist = store.allowlist().map_err(display)?;
            if !allowlist.revoke(&device) {
                return Err(format!("no enrolled device {device}"));
            }
            store.save_allowlist(&allowlist).map_err(display)?;
            println!("revoked {device}");
            return Ok(ExitCode::SUCCESS);
        }
        None => {}
    }

    // The keys first: this refuses to go on with readable key files.
    let identity = store.identity().map_err(display)?;
    let name = args.name.unwrap_or_else(host_name);
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
    run(home, store, config, args.pair).await
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
    let api = RelayApi::new(url);
    let code = api.device_start(name).await.map_err(display)?;
    eprintln!(
        "hame connect: to sign in, open {} in a signed-in app or browser and enter the code\n\n    {}\n",
        code.verification_url, code.user_code
    );
    if !code.verification_url_complete.is_empty() {
        eprintln!(
            "hame connect: or open {} directly",
            code.verification_url_complete
        );
    }
    let tokens = api.device_wait(&code).await.map_err(display)?;
    let device_id = api
        .register_host(&tokens.access_token, name, keys)
        .await
        .map_err(display)?;
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

/// Uses the stored access token first, then refreshes (rotating the
/// refresh token) before every reconnection.
fn token_source(store: HostStore, config: Arc<Mutex<RelayConfig>>) -> TokenSource {
    let first = Arc::new(std::sync::atomic::AtomicBool::new(true));
    Arc::new(move || {
        let store = store.clone();
        let config = Arc::clone(&config);
        let first = Arc::clone(&first);
        Box::pin(async move {
            let current = config.lock().expect("config").clone();
            if first.swap(false, std::sync::atomic::Ordering::SeqCst)
                || current.refresh_token.is_empty()
            {
                return Ok(current.access_token);
            }
            match RelayApi::new(&current.url)
                .refresh(&current.refresh_token)
                .await
            {
                Ok(tokens) => {
                    let mut config = config.lock().expect("config");
                    config.access_token = tokens.access_token.clone();
                    if !tokens.refresh_token.is_empty() {
                        config.refresh_token = tokens.refresh_token;
                    }
                    store.save_relay(&config)?;
                    Ok(tokens.access_token)
                }
                Err(_) => Ok(current.access_token),
            }
        })
    })
}

async fn run(
    home: &Path,
    store: HostStore,
    config: RelayConfig,
    pair: bool,
) -> Result<ExitCode, String> {
    let cwd = std::env::current_dir()
        .and_then(|cwd| hame_kernel::real_path(&cwd))
        .map_err(display)?;
    let options = hame_app::Options {
        max_steps: 0,
        ..hame_app::Options::default()
    };
    let (router, web_token) = hame_web::router(home.to_path_buf(), cwd.clone(), options);
    let shared = Arc::new(Mutex::new(config.clone()));
    let tokens = token_source(store.clone(), Arc::clone(&shared));
    let (relay, events) =
        hame_sync::relay::connect(RelayOptions::new(&config.url, &config.device_id, tokens));
    let api = RelayApi::new(&config.url);
    let host_id = config.device_id.clone();
    let grants_config = Arc::clone(&shared);
    let connector = Connector::new(
        ConnectorOptions {
            device_id: config.device_id.clone(),
            router,
            token: web_token,
            store: store.clone(),
            confirm: terminal_confirm(),
            notice: Arc::new(print_notice),
            grants_changed: Some(Arc::new(move |grants| {
                let api = api.clone();
                let host = host_id.clone();
                let token = grants_config.lock().expect("config").access_token.clone();
                tokio::spawn(async move {
                    if let Err(e) = api.post_grants(&token, &host, &grants).await {
                        eprintln!("hame connect: could not publish the device list: {e}");
                    }
                });
            })),
        },
        relay.clone(),
    )
    .map_err(display)?;

    eprintln!(
        "hame connect: host {} ({}) serving sessions in {}",
        config.device_id,
        connector.public().display_fingerprint(),
        cwd.display()
    );
    if pair || connector.devices().is_empty() {
        let offer = connector.offer_pairing(&config.url);
        let uri = offer.to_uri();
        eprintln!("\nScan this code with the hame app to add a device (valid 10 minutes, once):\n");
        eprintln!("{}", render_qr(&uri));
        eprintln!("or enter: {uri}\n");
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

fn print_notice(notice: Notice) {
    match notice {
        Notice::Rejected { from, reason } => {
            eprintln!("hame connect: dropped an envelope from {from}: {reason}")
        }
        Notice::Paired { device, label } => {
            eprintln!("hame connect: paired {label} ({device})")
        }
        Notice::PairingRefused { device, reason } => {
            eprintln!("hame connect: pairing with {device} refused: {reason}")
        }
        Notice::KeyChanged { device } => eprintln!(
            "hame connect: WARNING: device {device} presented a different key than the one it \
             was paired with. Refused. If you reset that device, revoke it and pair again."
        ),
        Notice::Revoked { device, by } => {
            eprintln!("hame connect: device {device} revoked (by {by})")
        }
        Notice::Relay(message) => eprintln!("hame connect: relay {message}"),
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
                    prompt.label, prompt.device, prompt.fingerprint, prompt.sas
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
}
