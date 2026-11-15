//! `hame-relay`: runs the relay server. Configuration comes from `RELAY_*`
//! environment variables (see docs/relay/DEPLOY.md).

use hame_relay::{Config, Relay, default_services};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    let filter = EnvFilter::try_from_env("RELAY_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let json = std::env::var("RELAY_LOG_FORMAT")
        .map(|v| v == "json")
        .unwrap_or(true);
    if json {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .json()
            .with_current_span(true)
            .init();
    } else {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    }

    if let Err(e) = run().await {
        tracing::error!(error = %e, "relay stopped");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let config = Config::from_env().map_err(|e| e.to_string())?;
    if matches!(config.mailer, hame_relay::config::MailerConfig::Log) {
        tracing::warn!(
            "RELAY_MAILER=log: emails are written to the log, not sent (development only)"
        );
    }
    let bind = config.bind;
    let services = default_services(&config).map_err(|e| e.to_string())?;
    let relay = Relay::new(config, services)
        .await
        .map_err(|e| e.to_string())?;
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .map_err(|e| format!("bind {bind}: {e}"))?;
    tracing::info!(%bind, "hame-relay listening");
    relay
        .serve(listener, shutdown_signal())
        .await
        .map_err(|e| e.to_string())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = term => {},
    }
    tracing::info!("shutting down");
}
