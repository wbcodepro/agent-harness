//! hame relay: accounts and authentication, devices and hosts, and routing
//! of end-to-end encrypted envelopes between the devices of one account.
//!
//! The relay is an untrusted mailbox: it stores public keys and opaque
//! ciphertext, never prompts, answers or files in readable form. See
//! `docs/design/APPS-AND-SYNC.md` (sections 3 and 4) for the design,
//! `docs/relay/` for deployment and the threat model.

pub mod auth;
pub mod config;
pub mod error;
pub mod mailer;
pub mod oidc;
pub mod password;
pub mod push;
pub mod ratelimit;
pub mod repo;
mod routes;
pub mod state;
pub mod util;
pub mod ws;

pub use config::Config;
pub use state::{AppState, Services};

use oidc::{OidcProvider, ProviderKind};
use repo::Repository;
use repo::sqlite::SqliteRepo;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;

/// A startup failure.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct StartError(pub String);

/// A built relay: its state and router.
pub struct Relay {
    pub state: AppState,
    router: axum::Router,
}

impl Relay {
    /// Opens the SQLite database and builds the relay.
    pub async fn new(config: Config, services: Services) -> Result<Self, StartError> {
        let repo = SqliteRepo::open(&config.database_path)
            .await
            .map_err(|e| StartError(format!("database: {e}")))?;
        Self::with_repo(config, services, Arc::new(repo)).await
    }

    /// Builds the relay on any repository.
    pub async fn with_repo(
        config: Config,
        services: Services,
        repo: Arc<dyn Repository>,
    ) -> Result<Self, StartError> {
        let hasher = password::PasswordHasher::new(config.argon2, config.argon2_concurrency)
            .await
            .map_err(|e| StartError(format!("password hasher: {e}")))?;
        let webauthn = match &config.webauthn {
            Some(w) => {
                let first = w
                    .origins
                    .first()
                    .ok_or_else(|| StartError("webauthn: no origin".into()))?;
                let mut b = webauthn_rs::WebauthnBuilder::new(&w.rp_id, first)
                    .map_err(|e| StartError(format!("webauthn: {e}")))?
                    .rp_name(&w.rp_name);
                for o in w.origins.iter().skip(1) {
                    b = b.append_allowed_origin(o);
                }
                Some(
                    b.build()
                        .map_err(|e| StartError(format!("webauthn: {e}")))?,
                )
            }
            None => None,
        };
        let provider = |kind, cfg: &Option<config::OidcConfig>| {
            cfg.as_ref()
                .map(|c| OidcProvider::new(kind, c.clone(), services.http.clone()))
                .transpose()
                .map_err(StartError)
        };
        let google = provider(ProviderKind::Google, &config.google)?;
        let apple = provider(ProviderKind::Apple, &config.apple)?;
        let enumeration_key = config
            .enumeration_key
            .unwrap_or_else(util::random_bytes::<32>);
        let state = AppState::new(state::Inner {
            repo,
            mailer: services.mailer,
            push: services.push,
            hub: ws::Hub::default(),
            limiter: ratelimit::RateLimiter::default(),
            hasher,
            webauthn,
            google,
            apple,
            enumeration_key,
            config,
        });
        let router = routes::router(state.clone());
        Ok(Self { state, router })
    }

    /// The router (for in-process tests).
    pub fn router(&self) -> axum::Router {
        self.router.clone()
    }

    /// Starts the maintenance task (expired rows, old envelopes, idle rate
    /// limit buckets).
    pub fn spawn_maintenance(&self) -> tokio::task::JoinHandle<()> {
        let state = self.state.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(300));
            loop {
                tick.tick().await;
                let now = util::now();
                let cutoff = now - state.config.mailbox_ttl.as_secs() as i64;
                match state.repo.purge(now, cutoff).await {
                    Ok(n) if n > 0 => tracing::info!(rows = n, "purged expired rows"),
                    Ok(_) => {}
                    Err(e) => tracing::error!(error = %e, "purge failed"),
                }
                state.limiter.sweep(Duration::from_secs(3600));
            }
        })
    }

    /// Serves on `listener` until `shutdown` resolves.
    pub async fn serve(
        self,
        listener: TcpListener,
        shutdown: impl std::future::Future<Output = ()> + Send + 'static,
    ) -> std::io::Result<()> {
        let maintenance = self.spawn_maintenance();
        let result = axum::serve(
            listener,
            self.router
                .into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown)
        .await;
        maintenance.abort();
        result
    }
}

/// The default services: the configured mailer and push sender and an
/// HTTP client with timeouts.
pub fn default_services(config: &Config) -> Result<Services, StartError> {
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .connect_timeout(Duration::from_secs(5))
        .user_agent(concat!("hame-relay/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| StartError(format!("http client: {e}")))?;
    Ok(Services {
        mailer: mailer::from_config(&config.mailer).map_err(StartError)?,
        push: push::from_config(&config.push, http.clone()).map_err(StartError)?,
        http,
    })
}
