//! The HTTP router and its middleware.

mod account;
mod device_code;
pub(crate) mod devices;
mod oauth;
mod passkey;

use crate::auth::ClientIp;
use crate::error::ApiError;
use crate::ratelimit::Limit;
use crate::state::AppState;
use axum::extract::{ConnectInfo, DefaultBodyLimit, MatchedPath, Request, State};
use axum::http::{HeaderName, HeaderValue, Method, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use std::net::SocketAddr;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::{DefaultOnResponse, TraceLayer};

/// The complete application router.
pub fn router(state: AppState) -> Router {
    let auth = Router::new()
        .route("/signup", post(account::signup))
        .route("/verify", post(account::verify))
        .route("/verify/resend", post(account::resend_verification))
        .route("/login", post(account::login))
        .route("/refresh", post(account::refresh))
        .route("/logout", post(account::logout))
        .route("/reauth", post(account::reauth))
        .route("/password/forgot", post(account::forgot_password))
        .route("/password/reset", post(account::reset_password))
        .route("/oauth/{provider}/start", get(oauth::start))
        .route(
            "/oauth/{provider}/callback",
            get(oauth::callback_get).post(oauth::callback_post),
        )
        .route("/oauth/exchange", post(oauth::exchange))
        .route("/passkey/register/start", post(passkey::register_start))
        .route("/passkey/register/finish", post(passkey::register_finish))
        .route("/passkey/login/start", post(passkey::login_start))
        .route("/passkey/login/finish", post(passkey::login_finish))
        .route("/device/start", post(device_code::start))
        .route("/device/lookup", post(device_code::lookup))
        .route("/device/approve", post(device_code::approve))
        .route("/device/token", post(device_code::token))
        .layer(middleware::from_fn_with_state(state.clone(), ip_rate_limit));

    let v1 = Router::new()
        .nest("/auth", auth)
        .route(
            "/account",
            get(account::profile).patch(account::update_profile),
        )
        .route("/account/password", post(account::change_password))
        .route("/account/audit", get(account::audit_log))
        .route("/account/sessions", get(account::sessions))
        .route("/account/sessions/{id}", delete(account::revoke_session))
        .route("/account/passkeys", get(passkey::list))
        .route("/account/passkeys/{id}", delete(passkey::remove))
        .route("/devices", post(devices::register).get(devices::list))
        .route("/devices/{id}", delete(devices::revoke))
        .route("/devices/{id}/rebind", post(devices::rebind))
        .route("/devices/{id}/push", post(devices::set_push))
        .route("/hosts", get(devices::hosts))
        .route(
            "/hosts/{id}/grants",
            post(devices::put_grants).get(devices::get_grants),
        )
        .route("/ws", get(crate::ws::handler));

    let mut app = Router::new()
        .nest("/v1", v1)
        .route("/healthz", get(healthz))
        .fallback(|| async { ApiError::not_found() })
        .layer(DefaultBodyLimit::max(state.config.max_body_bytes))
        .layer(middleware::from_fn_with_state(state.clone(), client_ip));

    for (name, value) in security_headers(state.config.hsts) {
        app = app.layer(SetResponseHeaderLayer::if_not_present(name, value));
    }
    if let Some(cors) = cors_layer(&state.config.cors_origins) {
        app = app.layer(cors);
    }
    app.layer(
        TraceLayer::new_for_http()
            .make_span_with(|req: &Request| {
                // Method and route template only: query strings carry OAuth
                // codes and state.
                let path = req
                    .extensions()
                    .get::<MatchedPath>()
                    .map(|p| p.as_str().to_string())
                    .unwrap_or_else(|| "unmatched".into());
                tracing::info_span!("http", method = %req.method(), path = %path)
            })
            .on_response(DefaultOnResponse::new().level(tracing::Level::INFO)),
    )
    .with_state(state)
}

fn security_headers(hsts: bool) -> Vec<(HeaderName, HeaderValue)> {
    let mut v = vec![
        (
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ),
        (header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY")),
        (
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        ),
        (
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("default-src 'none'; frame-ancestors 'none'"),
        ),
        (
            HeaderName::from_static("cross-origin-resource-policy"),
            HeaderValue::from_static("same-site"),
        ),
        (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
    ];
    if hsts {
        v.push((
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=63072000; includeSubDomains"),
        ));
    }
    v
}

fn cors_layer(origins: &[String]) -> Option<CorsLayer> {
    let list: Vec<HeaderValue> = origins
        .iter()
        .filter_map(|o| HeaderValue::from_str(o).ok())
        .collect();
    if list.is_empty() {
        return None;
    }
    Some(
        CorsLayer::new()
            .allow_origin(AllowOrigin::list(list))
            .allow_methods([Method::GET, Method::POST, Method::PATCH, Method::DELETE])
            .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
            .max_age(std::time::Duration::from_secs(600)),
    )
}

/// Resolves the client IP once per request.
async fn client_ip(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0);
    let ip = ClientIp::from_parts(req.headers(), peer, state.config.trust_proxy);
    req.extensions_mut().insert(ip);
    next.run(req).await
}

/// Per-IP limit on every authentication route.
async fn ip_rate_limit(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let ip = req
        .extensions()
        .get::<ClientIp>()
        .map(|c| crate::ratelimit::ip_key(c.0))
        .unwrap_or_default();
    let limit = Limit::per_minute(state.config.rate_limits.auth_per_ip_per_min);
    if let Err(wait) = state.limiter.check(&format!("ip:{ip}"), limit) {
        return ApiError::rate_limited(wait).into_response();
    }
    next.run(req).await
}

async fn healthz(State(state): State<AppState>) -> Response {
    match state.repo.ping().await {
        Ok(()) => Json(serde_json::json!({ "status": "ok" })).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "health check: database unavailable");
            (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({ "status": "unavailable" })),
            )
                .into_response()
        }
    }
}
