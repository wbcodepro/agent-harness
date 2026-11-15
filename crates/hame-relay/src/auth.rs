//! Tokens, the bearer-token extractor and the client-IP extractor.

use crate::error::{ApiError, ApiResult};
use crate::repo::{AuthContext, SCOPE_FULL, SCOPE_HOST};
use crate::state::AppState;
use crate::util;
use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::HeaderMap;
use axum::http::request::Parts;
use serde::Serialize;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

pub const ACCESS_PREFIX: &str = "hra_";
pub const REFRESH_PREFIX: &str = "hrr_";

/// Returned by every sign-in.
#[derive(Debug, Serialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: &'static str,
    /// Seconds until the access token expires.
    pub expires_in: u64,
    pub account_id: String,
    pub session_id: String,
    pub device_id: Option<String>,
}

/// Starts a new session (token family) and returns its first tokens.
pub async fn issue_tokens(
    state: &AppState,
    account_id: &str,
    device_id: Option<&str>,
    method: &str,
) -> ApiResult<TokenResponse> {
    let now = util::now();
    let session_id = uuid::Uuid::new_v4().to_string();
    // Device-code sign-ins never proved a credential on this device: they
    // may only register and connect a host.
    let scope = if method == "device_code" {
        SCOPE_HOST
    } else {
        SCOPE_FULL
    };
    state
        .repo
        .create_session(&session_id, account_id, device_id, method, scope, now)
        .await?;
    let access = util::random_token(ACCESS_PREFIX);
    let refresh = util::random_token(REFRESH_PREFIX);
    let access_ttl = state.config.access_token_ttl.as_secs();
    state
        .repo
        .insert_access_token(
            &util::token_hash(&access),
            &session_id,
            now + access_ttl as i64,
        )
        .await?;
    state
        .repo
        .insert_refresh_token(
            &util::token_hash(&refresh),
            &session_id,
            now + state.config.refresh_token_ttl.as_secs() as i64,
            now,
        )
        .await?;
    Ok(TokenResponse {
        access_token: access,
        refresh_token: refresh,
        token_type: "Bearer",
        expires_in: access_ttl,
        account_id: account_id.to_string(),
        session_id,
        device_id: device_id.map(str::to_string),
    })
}

/// A new access token for an existing session (after a refresh).
pub async fn issue_access_token(state: &AppState, session_id: &str) -> ApiResult<String> {
    let access = util::random_token(ACCESS_PREFIX);
    let ttl = state.config.access_token_ttl.as_secs() as i64;
    state
        .repo
        .insert_access_token(&util::token_hash(&access), session_id, util::now() + ttl)
        .await?;
    Ok(access)
}

/// The bearer token in `Authorization`, if any.
pub fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let value = headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    (!token.is_empty()).then_some(token)
}

/// A request authenticated with a valid access token.
#[derive(Clone, Debug)]
pub struct Authed(pub AuthContext);

impl FromRequestParts<AppState> for Authed {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> ApiResult<Self> {
        let token = bearer_token(&parts.headers).ok_or_else(ApiError::unauthorized)?;
        authenticate(state, token).await.map(Authed)
    }
}

/// A request with a full (not device-code) session.
#[derive(Clone, Debug)]
pub struct FullAuthed(pub AuthContext);

impl FromRequestParts<AppState> for FullAuthed {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> ApiResult<Self> {
        let Authed(ctx) = Authed::from_request_parts(parts, state).await?;
        if !ctx.is_full() {
            return Err(ApiError::host_scope());
        }
        Ok(FullAuthed(ctx))
    }
}

/// A request with a full session that proved a credential within
/// `recent_auth_window` (sign-in or `POST /v1/auth/reauth`).
#[derive(Clone, Debug)]
pub struct RecentAuthed(pub AuthContext);

impl FromRequestParts<AppState> for RecentAuthed {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> ApiResult<Self> {
        let FullAuthed(ctx) = FullAuthed::from_request_parts(parts, state).await?;
        let window = state.config.recent_auth_window.as_secs() as i64;
        if util::now() - ctx.auth_at > window {
            return Err(ApiError::reauth_required());
        }
        Ok(RecentAuthed(ctx))
    }
}

/// Resolves an access token.
pub async fn authenticate(state: &AppState, token: &str) -> ApiResult<AuthContext> {
    if !token.starts_with(ACCESS_PREFIX) {
        return Err(ApiError::unauthorized());
    }
    state
        .repo
        .authenticate(&util::token_hash(token), util::now())
        .await?
        .ok_or_else(ApiError::unauthorized)
}

/// The client's IP address: the socket peer, or with `trust_proxy` the
/// last `X-Forwarded-For` entry (the address the reverse proxy saw).
#[derive(Clone, Copy, Debug)]
pub struct ClientIp(pub IpAddr);

impl ClientIp {
    pub fn from_parts(headers: &HeaderMap, peer: Option<SocketAddr>, trust_proxy: bool) -> Self {
        if trust_proxy
            && let Some(ip) = headers
                .get("x-forwarded-for")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.rsplit(',').next())
                .and_then(|v| v.trim().parse::<IpAddr>().ok())
        {
            return Self(ip);
        }
        Self(
            peer.map(|p| p.ip())
                .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
        )
    }
}

impl FromRequestParts<AppState> for ClientIp {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        if let Some(ip) = parts.extensions.get::<ClientIp>() {
            return Ok(*ip);
        }
        let peer = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|c| c.0);
        Ok(Self::from_parts(
            &parts.headers,
            peer,
            state.config.trust_proxy,
        ))
    }
}
