//! Email and password accounts: sign-up, verification, login, refresh,
//! logout, re-authentication, password reset, sessions and the profile.

use crate::auth::{Authed, ClientIp, FullAuthed, TokenResponse, issue_access_token, issue_tokens};
use crate::error::{ApiError, ApiJson, ApiResult};
use crate::mailer::Email;
use crate::password::check_strength;
use crate::ratelimit::{Limit, ip_key};
use crate::repo::{Account, RefreshOutcome};
use crate::state::AppState;
use crate::util;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;
use std::net::IpAddr;

const VERIFY_TTL: i64 = 24 * 3600;
const RESET_TTL: i64 = 3600;
pub const PURPOSE_VERIFY: &str = "verify";
pub const PURPOSE_RESET: &str = "reset";
/// Sign-ins from a network seen within this period are not notified.
const KNOWN_NETWORK_PERIOD: i64 = 90 * 24 * 3600;
const GLOBAL_FAILED_LOGINS: &str = "global:login_failed";

/// The same answer whether or not the address has an account.
fn accepted() -> Response {
    (
        StatusCode::ACCEPTED,
        Json(json!({
            "status": "accepted",
            "message": "If the address can receive mail, a message is on its way."
        })),
    )
        .into_response()
}

pub fn validate_name(raw: &str) -> ApiResult<String> {
    let name = raw.trim();
    let len = name.chars().count();
    if len == 0 || len > 100 || name.chars().any(char::is_control) {
        return Err(ApiError::bad_request(
            "invalid_name",
            "name must have 1 to 100 characters",
        ));
    }
    Ok(name.to_string())
}

fn validate_email(raw: &str) -> ApiResult<String> {
    util::normalize_email(raw)
        .ok_or_else(|| ApiError::bad_request("invalid_email", "not a valid email address"))
}

async fn new_email_token(
    state: &AppState,
    account_id: &str,
    purpose: &str,
    ttl: i64,
) -> ApiResult<String> {
    let token = util::random_token("");
    let now = util::now();
    state
        .repo
        .insert_email_token(
            &util::token_hash(&token),
            account_id,
            purpose,
            now + ttl,
            now,
        )
        .await?;
    Ok(token)
}

async fn send_verification(state: &AppState, account: &Account) -> ApiResult<()> {
    let token = new_email_token(state, &account.id, PURPOSE_VERIFY, VERIFY_TTL).await?;
    let link = state.app_link("/verify", &[("token", &token)]);
    state.send_email(Email {
        to: account.email.clone(),
        subject: "Verify your email for hame".into(),
        text: format!(
            "Hello {},\n\nconfirm your email address to finish creating your hame account:\n\n{link}\n\n\
             The link is valid for 24 hours. If you did not sign up, ignore this message and do \
             not open the link.\n",
            account.name
        ),
    });
    Ok(())
}

async fn send_reset(state: &AppState, account: &Account, intro: &str) -> ApiResult<()> {
    let token = new_email_token(state, &account.id, PURPOSE_RESET, RESET_TTL).await?;
    let link = state.app_link("/reset-password", &[("token", &token)]);
    state.send_email(Email {
        to: account.email.clone(),
        subject: "Your hame account".into(),
        text: format!(
            "Hello {},\n\n{intro}\n\nTo choose a new password, open:\n\n{link}\n\n\
             The link is valid for one hour. If this was not you, ignore this message; \
             your password stays unchanged.\n",
            account.name
        ),
    });
    Ok(())
}

/// Deletes outstanding verification and reset links of an account.
pub async fn delete_email_links(state: &AppState, account_id: &str) -> ApiResult<()> {
    state
        .repo
        .delete_email_tokens(account_id, PURPOSE_RESET)
        .await?;
    state
        .repo
        .delete_email_tokens(account_id, PURPOSE_VERIFY)
        .await?;
    Ok(())
}

/// Kicks the WebSocket connections of devices whose sessions were revoked.
pub fn kick_all(state: &AppState, devices: &[String]) {
    for d in devices {
        state.hub.kick(d);
    }
}

/// The network an address belongs to for "new sign-in" notices: IPv4 /24,
/// IPv6 /64.
fn network(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            format!("{}.{}.{}.0/24", o[0], o[1], o[2])
        }
        IpAddr::V6(_) => ip_key(ip),
    }
}

/// Emails the owner when a sign-in comes from a network not seen in the
/// last 90 days (not for the very first sign-in). Call before the sign-in
/// is written to the audit log.
pub async fn notify_new_sign_in(state: &AppState, account_id: &str, ip: IpAddr, method: &str) {
    let since = util::now() - KNOWN_NETWORK_PERIOD;
    let Ok(ips) = state.repo.recent_login_ips(account_id, since, 500).await else {
        return;
    };
    if ips.is_empty() {
        return;
    }
    let net = network(ip);
    if ips
        .iter()
        .filter_map(|s| s.parse::<IpAddr>().ok())
        .any(|p| network(p) == net)
    {
        return;
    }
    state
        .notify_account(
            account_id,
            "New sign-in to your hame account",
            &format!(
                "Someone signed in to your account ({method}) from {ip}, a network not seen before."
            ),
        )
        .await;
}

#[derive(Deserialize)]
pub struct SignupBody {
    name: String,
    email: String,
    password: String,
}

pub async fn signup(
    State(state): State<AppState>,
    ip: ClientIp,
    ApiJson(body): ApiJson<SignupBody>,
) -> ApiResult<Response> {
    let name = validate_name(&body.name)?;
    let email = validate_email(&body.email)?;
    state.check_account_limit("signup", &email)?;
    check_strength(&body.password, &email, &name)
        .map_err(|m| ApiError::bad_request("weak_password", m))?;
    // Hash before looking the address up, so both paths cost the same.
    let hash = state.hasher.hash(&body.password).await?;
    if let Some(existing) = state.repo.account_by_email(&email).await? {
        send_reset(
            &state,
            &existing,
            "someone (hopefully you) tried to create a hame account with this address, \
             but it already has one. You can sign in as usual.",
        )
        .await?;
        state
            .audit(
                "signup_existing_email",
                Some(&existing.id),
                None,
                Some(ip.0),
                None,
            )
            .await;
        return Ok(accepted());
    }
    let account = Account {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        email,
        email_verified: false,
        password_hash: Some(hash),
        password_proven: false,
        created_at: util::now(),
    };
    match state.repo.create_account(&account).await {
        Ok(()) => {}
        // A concurrent sign-up with the same address: same answer.
        Err(crate::repo::RepoError::Conflict) => return Ok(accepted()),
        Err(e) => return Err(e.into()),
    }
    send_verification(&state, &account).await?;
    state
        .audit("signup", Some(&account.id), None, Some(ip.0), None)
        .await;
    Ok(accepted())
}

#[derive(Deserialize)]
pub struct TokenBody {
    token: String,
}

pub async fn verify(
    State(state): State<AppState>,
    ip: ClientIp,
    ApiJson(body): ApiJson<TokenBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let now = util::now();
    let account_id = state
        .repo
        .consume_email_token(&util::token_hash(body.token.trim()), PURPOSE_VERIFY, now)
        .await?
        .ok_or_else(|| ApiError::bad_request("invalid_token", "the link is invalid or expired"))?;
    state.repo.set_email_verified(&account_id, now).await?;
    state
        .repo
        .delete_email_tokens(&account_id, PURPOSE_VERIFY)
        .await?;
    state
        .audit("email_verified", Some(&account_id), None, Some(ip.0), None)
        .await;
    Ok(Json(json!({ "status": "verified" })))
}

#[derive(Deserialize)]
pub struct EmailBody {
    email: String,
}

/// Runs `work` after the response, so that its database work cannot be
/// timed by the caller.
fn off_response_path<F>(work: F)
where
    F: std::future::Future<Output = ApiResult<()>> + Send + 'static,
{
    tokio::spawn(async move {
        if let Err(e) = work.await {
            tracing::error!(error = %e.message, "background mail work failed");
        }
    });
}

pub async fn resend_verification(
    State(state): State<AppState>,
    ApiJson(body): ApiJson<EmailBody>,
) -> ApiResult<Response> {
    let Some(email) = util::normalize_email(&body.email) else {
        return Ok(accepted());
    };
    state.check_account_limit("resend", &email)?;
    off_response_path(async move {
        if let Some(account) = state.repo.account_by_email(&email).await?
            && !account.email_verified
        {
            send_verification(&state, &account).await?;
        }
        Ok(())
    });
    Ok(accepted())
}

#[derive(Deserialize)]
pub struct LoginBody {
    email: String,
    password: String,
}

pub async fn login(
    State(state): State<AppState>,
    ip: ClientIp,
    ApiJson(body): ApiJson<LoginBody>,
) -> ApiResult<Json<TokenResponse>> {
    let email = util::normalize_email(&body.email).unwrap_or_default();
    state.check_account_limit("login", &email)?;
    // Across all addresses: a credential-stuffing wave stops password
    // logins for a while instead of costing unlimited Argon2id work.
    let global = Limit::per_minute(state.config.rate_limits.global_failed_logins_per_min);
    state
        .limiter
        .peek(GLOBAL_FAILED_LOGINS, global)
        .map_err(ApiError::rate_limited)?;
    let account = if email.is_empty() {
        None
    } else {
        state.repo.account_by_email(&email).await?
    };
    let phc = account.as_ref().and_then(|a| a.password_hash.as_deref());
    let ok = body.password.chars().count() <= crate::password::MAX_LEN
        && state.hasher.verify(&body.password, phc).await?;
    let Some(account) = account.clone().filter(|_| ok) else {
        let _ = state.limiter.check(GLOBAL_FAILED_LOGINS, global);
        state
            .audit(
                "login_failed",
                account.as_ref().map(|a| a.id.as_str()),
                None,
                Some(ip.0),
                Some("password".into()),
            )
            .await;
        return Err(ApiError::invalid_credentials());
    };
    if !account.email_verified {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "email_not_verified",
            "confirm your email address first",
        ));
    }
    // A verified owner signed in with this password: it is theirs.
    state.repo.mark_password_proven(&account.id).await?;
    let tokens = issue_tokens(&state, &account.id, None, "password").await?;
    notify_new_sign_in(&state, &account.id, ip.0, "password").await;
    state
        .audit(
            "login",
            Some(&account.id),
            None,
            Some(ip.0),
            Some("password".into()),
        )
        .await;
    Ok(Json(tokens))
}

#[derive(Deserialize)]
pub struct ReauthBody {
    password: String,
}

/// Proves the password again in this session: makes it recent (for adding
/// passkeys) and upgrades a device-code session to a full one.
pub async fn reauth(
    State(state): State<AppState>,
    ip: ClientIp,
    Authed(ctx): Authed,
    ApiJson(body): ApiJson<ReauthBody>,
) -> ApiResult<StatusCode> {
    state.check_account_limit("reauth", &ctx.account_id)?;
    let account = state
        .repo
        .account(&ctx.account_id)
        .await?
        .ok_or_else(ApiError::unauthorized)?;
    let Some(phc) = account.password_hash.as_deref() else {
        return Err(ApiError::forbidden(
            "this account has no password; sign in again with your provider or passkey",
        ));
    };
    if body.password.chars().count() > crate::password::MAX_LEN
        || !state.hasher.verify(&body.password, Some(phc)).await?
    {
        state
            .audit(
                "reauth_failed",
                Some(&account.id),
                ctx.device_id.as_deref(),
                Some(ip.0),
                None,
            )
            .await;
        return Err(ApiError::invalid_credentials());
    }
    state.repo.mark_password_proven(&account.id).await?;
    state
        .repo
        .reauth_session(&ctx.session_id, util::now())
        .await?;
    state
        .audit(
            "reauth",
            Some(&account.id),
            ctx.device_id.as_deref(),
            Some(ip.0),
            Some(ctx.scope),
        )
        .await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct RefreshBody {
    refresh_token: String,
}

pub async fn refresh(
    State(state): State<AppState>,
    ip: ClientIp,
    ApiJson(body): ApiJson<RefreshBody>,
) -> ApiResult<Json<TokenResponse>> {
    let now = util::now();
    let new_refresh = util::random_token(crate::auth::REFRESH_PREFIX);
    let outcome = state
        .repo
        .rotate_refresh_token(
            &util::token_hash(body.refresh_token.trim()),
            &util::token_hash(&new_refresh),
            now + state.config.refresh_token_ttl.as_secs() as i64,
            now,
        )
        .await?;
    match outcome {
        RefreshOutcome::Rotated(ctx) => {
            let access = issue_access_token(&state, &ctx.session_id).await?;
            Ok(Json(TokenResponse {
                access_token: access,
                refresh_token: new_refresh,
                token_type: "Bearer",
                expires_in: state.config.access_token_ttl.as_secs(),
                account_id: ctx.account_id,
                session_id: ctx.session_id,
                device_id: ctx.device_id,
            }))
        }
        RefreshOutcome::Reused(ctx) => {
            if let Some(d) = &ctx.device_id {
                state.hub.kick(d);
            }
            state
                .audit(
                    "refresh_token_reused",
                    Some(&ctx.account_id),
                    ctx.device_id.as_deref(),
                    Some(ip.0),
                    Some(format!("session {} revoked", ctx.session_id)),
                )
                .await;
            Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "token_reused",
                "this refresh token was already used; the session is revoked",
            ))
        }
        RefreshOutcome::Invalid => Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "invalid_token",
            "refresh token is invalid or expired",
        )),
    }
}

pub async fn logout(
    State(state): State<AppState>,
    ip: ClientIp,
    Authed(ctx): Authed,
) -> ApiResult<StatusCode> {
    if let Some(d) = state
        .repo
        .revoke_session(&ctx.session_id, "logout", util::now())
        .await?
    {
        state.hub.kick(&d);
    }
    state
        .audit(
            "logout",
            Some(&ctx.account_id),
            ctx.device_id.as_deref(),
            Some(ip.0),
            None,
        )
        .await;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn forgot_password(
    State(state): State<AppState>,
    ip: ClientIp,
    ApiJson(body): ApiJson<EmailBody>,
) -> ApiResult<Response> {
    let Some(email) = util::normalize_email(&body.email) else {
        return Ok(accepted());
    };
    state.check_account_limit("forgot", &email)?;
    off_response_path(async move {
        if let Some(account) = state.repo.account_by_email(&email).await? {
            send_reset(
                &state,
                &account,
                "we received a request to reset the password of your hame account.",
            )
            .await?;
            state
                .audit(
                    "password_reset_requested",
                    Some(&account.id),
                    None,
                    Some(ip.0),
                    None,
                )
                .await;
        }
        Ok(())
    });
    Ok(accepted())
}

#[derive(Deserialize)]
pub struct ResetBody {
    token: String,
    password: String,
}

/// Sets a new password through a reset link. Everything an intruder may
/// have left behind goes: all sessions, all passkeys and every other
/// outstanding link.
pub async fn reset_password(
    State(state): State<AppState>,
    ip: ClientIp,
    ApiJson(body): ApiJson<ResetBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let now = util::now();
    let hash = util::token_hash(body.token.trim());
    let invalid = || ApiError::bad_request("invalid_token", "the link is invalid or expired");
    let account_id = state
        .repo
        .peek_email_token(&hash, PURPOSE_RESET, now)
        .await?
        .ok_or_else(invalid)?;
    let account = state.repo.account(&account_id).await?.ok_or_else(invalid)?;
    check_strength(&body.password, &account.email, &account.name)
        .map_err(|m| ApiError::bad_request("weak_password", m))?;
    let phc = state.hasher.hash(&body.password).await?;
    if state
        .repo
        .consume_email_token(&hash, PURPOSE_RESET, now)
        .await?
        .is_none()
    {
        return Err(invalid());
    }
    // The link proved control of the inbox: the password is the owner's.
    state
        .repo
        .set_password(&account.id, Some(&phc), true, now)
        .await?;
    state.repo.set_email_verified(&account.id, now).await?;
    delete_email_links(&state, &account.id).await?;
    let passkeys = state.repo.delete_passkeys(&account.id).await?;
    let devices = state
        .repo
        .revoke_account_sessions(&account.id, None, "password_reset", now)
        .await?;
    kick_all(&state, &devices);
    state
        .audit(
            "password_reset",
            Some(&account.id),
            None,
            Some(ip.0),
            Some(format!("{passkeys} passkeys removed")),
        )
        .await;
    state
        .notify_account(
            &account.id,
            "Your hame password was reset",
            "Your password was reset. All sessions were signed out and all passkeys removed; \
             add your passkeys again after signing in.",
        )
        .await;
    Ok(Json(
        json!({ "status": "password_changed", "passkeys_removed": passkeys }),
    ))
}

pub async fn profile(
    State(state): State<AppState>,
    Authed(ctx): Authed,
) -> ApiResult<Json<serde_json::Value>> {
    let account = state
        .repo
        .account(&ctx.account_id)
        .await?
        .ok_or_else(ApiError::unauthorized)?;
    let identities = state.repo.identities(&account.id).await?;
    let passkeys = state.repo.passkeys(&account.id).await?;
    Ok(Json(json!({
        "id": account.id,
        "name": account.name,
        "email": account.email,
        "email_verified": account.email_verified,
        "has_password": account.password_hash.is_some(),
        "created_at": account.created_at,
        "identities": identities,
        "passkeys": passkeys.iter().map(|p| json!({
            "id": p.id, "name": p.name, "created_at": p.created_at, "last_used_at": p.last_used_at,
        })).collect::<Vec<_>>(),
        "session_id": ctx.session_id,
        "session_scope": ctx.scope,
        "device_id": ctx.device_id,
    })))
}

#[derive(Deserialize)]
pub struct ProfileBody {
    name: String,
}

pub async fn update_profile(
    State(state): State<AppState>,
    FullAuthed(ctx): FullAuthed,
    ApiJson(body): ApiJson<ProfileBody>,
) -> ApiResult<StatusCode> {
    let name = validate_name(&body.name)?;
    state
        .repo
        .set_name(&ctx.account_id, &name, util::now())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct ChangePasswordBody {
    current_password: String,
    new_password: String,
    /// Also remove every passkey of the account.
    #[serde(default)]
    remove_passkeys: bool,
}

/// Changes the password (the current one is required; accounts without a
/// password use the reset flow, which proves control of the inbox).
/// Revokes every other session and outstanding reset or verification link,
/// and optionally removes all passkeys.
pub async fn change_password(
    State(state): State<AppState>,
    ip: ClientIp,
    FullAuthed(ctx): FullAuthed,
    ApiJson(body): ApiJson<ChangePasswordBody>,
) -> ApiResult<StatusCode> {
    state.check_account_limit("password", &ctx.account_id)?;
    let account = state
        .repo
        .account(&ctx.account_id)
        .await?
        .ok_or_else(ApiError::unauthorized)?;
    let Some(current) = account.password_hash.as_deref() else {
        return Err(ApiError::forbidden(
            "this account has no password; use the password reset email",
        ));
    };
    if !state
        .hasher
        .verify(&body.current_password, Some(current))
        .await?
    {
        state
            .audit(
                "password_change_failed",
                Some(&account.id),
                None,
                Some(ip.0),
                None,
            )
            .await;
        return Err(ApiError::invalid_credentials());
    }
    check_strength(&body.new_password, &account.email, &account.name)
        .map_err(|m| ApiError::bad_request("weak_password", m))?;
    let phc = state.hasher.hash(&body.new_password).await?;
    let now = util::now();
    state
        .repo
        .set_password(&account.id, Some(&phc), true, now)
        .await?;
    delete_email_links(&state, &account.id).await?;
    let passkeys = if body.remove_passkeys {
        state.repo.delete_passkeys(&account.id).await?
    } else {
        0
    };
    let devices = state
        .repo
        .revoke_account_sessions(&account.id, Some(&ctx.session_id), "password_changed", now)
        .await?;
    kick_all(&state, &devices);
    state
        .audit(
            "password_changed",
            Some(&account.id),
            ctx.device_id.as_deref(),
            Some(ip.0),
            Some(format!("{passkeys} passkeys removed")),
        )
        .await;
    state
        .notify_account(
            &account.id,
            "Your hame password was changed",
            "Your password was changed and your other sessions were signed out.",
        )
        .await;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn audit_log(
    State(state): State<AppState>,
    FullAuthed(ctx): FullAuthed,
) -> ApiResult<Json<serde_json::Value>> {
    let entries = state.repo.audit_for_account(&ctx.account_id, 100).await?;
    Ok(Json(json!({ "events": entries })))
}

/// The account's active sessions.
pub async fn sessions(
    State(state): State<AppState>,
    FullAuthed(ctx): FullAuthed,
) -> ApiResult<Json<serde_json::Value>> {
    let list = state.repo.sessions(&ctx.account_id).await?;
    let items: Vec<_> = list
        .into_iter()
        .map(|s| {
            let current = s.id == ctx.session_id;
            let mut v = serde_json::to_value(s).unwrap_or_default();
            v["current"] = json!(current);
            v
        })
        .collect();
    Ok(Json(json!({ "sessions": items })))
}

/// Revokes one session of the account (and closes its device's WebSocket).
pub async fn revoke_session(
    State(state): State<AppState>,
    ip: ClientIp,
    FullAuthed(ctx): FullAuthed,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let owned = state
        .repo
        .sessions(&ctx.account_id)
        .await?
        .into_iter()
        .any(|s| s.id == id);
    if !owned {
        return Err(ApiError::not_found());
    }
    if let Some(d) = state
        .repo
        .revoke_session(&id, "revoked_by_user", util::now())
        .await?
    {
        state.hub.kick(&d);
    }
    state
        .audit(
            "session_revoked",
            Some(&ctx.account_id),
            ctx.device_id.as_deref(),
            Some(ip.0),
            Some(id),
        )
        .await;
    Ok(StatusCode::NO_CONTENT)
}
