//! Device-code sign-in for hosts and CLIs (after RFC 8628).
//!
//! The host calls `start`, shows the user code and polls `token`. The user
//! enters the code in a signed-in app, which calls `lookup` (to show what
//! is asking) and `approve`.

use crate::auth::{Authed, ClientIp, issue_tokens};
use crate::error::{ApiError, ApiJson, ApiResult};
use crate::repo::DeviceCodeRow;
use crate::state::AppState;
use crate::util;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};

/// Letters that are hard to confuse and spell no words (no vowels).
const ALPHABET: &[u8] = b"BCDFGHJKLMNPQRSTVWXZ";

fn new_user_code() -> String {
    let bytes = util::random_bytes::<16>();
    // 256 is not a multiple of 20; reject the biased tail.
    let mut out = String::new();
    let mut i = 0;
    let mut pool = bytes.to_vec();
    while out.len() < 9 {
        if i == pool.len() {
            pool = util::random_bytes::<16>().to_vec();
            i = 0;
        }
        let b = pool[i];
        i += 1;
        if b >= 240 {
            continue;
        }
        if out.len() == 4 {
            out.push('-');
        }
        out.push(ALPHABET[(b % 20) as usize] as char);
    }
    out
}

/// Upper case, without separators.
fn normalize_user_code(raw: &str) -> String {
    raw.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

fn user_code_hash(raw: &str) -> String {
    util::token_hash(&normalize_user_code(raw))
}

fn grant_error(code: &'static str, message: &str) -> ApiError {
    ApiError::bad_request(code, message)
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct StartBody {
    client_name: Option<String>,
}

pub async fn start(
    State(state): State<AppState>,
    ApiJson(body): ApiJson<StartBody>,
) -> ApiResult<Json<Value>> {
    let client_name = match body.client_name.as_deref() {
        Some(n) => super::account::validate_name(n)?,
        None => "hame".to_string(),
    };
    let now = util::now();
    let ttl = state.config.device_code_ttl.as_secs() as i64;
    let interval = state.config.device_code_interval.as_secs().max(1) as i64;
    let device_code = util::random_token("hdc_");
    for _ in 0..5 {
        let user_code = new_user_code();
        let row = DeviceCodeRow {
            device_code_hash: util::token_hash(&device_code),
            user_code_hash: user_code_hash(&user_code),
            client_name: client_name.clone(),
            status: "pending".into(),
            account_id: None,
            interval_secs: interval,
            last_poll_at: None,
            expires_at: now + ttl,
            created_at: now,
        };
        match state.repo.insert_device_code(&row).await {
            Ok(()) => {
                return Ok(Json(json!({
                    "device_code": device_code,
                    "user_code": user_code,
                    "verification_url": state.app_link("/device", &[]),
                    "verification_url_complete": state.app_link("/device", &[("code", &user_code)]),
                    "expires_in": ttl,
                    "interval": interval,
                })));
            }
            Err(crate::repo::RepoError::Conflict) => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(ApiError::internal("could not allocate a unique user code"))
}

#[derive(Deserialize)]
pub struct UserCodeBody {
    user_code: String,
    #[serde(default = "yes")]
    approve: bool,
}

fn yes() -> bool {
    true
}

async fn pending_code(
    state: &AppState,
    account_id: &str,
    user_code: &str,
) -> ApiResult<DeviceCodeRow> {
    // Limits guessing of user codes from a signed-in account.
    state.check_account_limit("device_code", account_id)?;
    state
        .repo
        .device_code_by_user_code(&user_code_hash(user_code))
        .await?
        .filter(|r| r.status == "pending" && r.expires_at > util::now())
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "invalid_code",
                "the code is unknown, used or expired",
            )
        })
}

pub async fn lookup(
    State(state): State<AppState>,
    Authed(ctx): Authed,
    ApiJson(body): ApiJson<UserCodeBody>,
) -> ApiResult<Json<Value>> {
    let row = pending_code(&state, &ctx.account_id, &body.user_code).await?;
    Ok(Json(json!({
        "client_name": row.client_name,
        "created_at": row.created_at,
        "expires_in": row.expires_at - util::now(),
    })))
}

pub async fn approve(
    State(state): State<AppState>,
    ip: ClientIp,
    Authed(ctx): Authed,
    ApiJson(body): ApiJson<UserCodeBody>,
) -> ApiResult<StatusCode> {
    let row = pending_code(&state, &ctx.account_id, &body.user_code).await?;
    let to = if body.approve { "approved" } else { "denied" };
    if !state
        .repo
        .transition_device_code(&row.device_code_hash, "pending", to, Some(&ctx.account_id))
        .await?
    {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "invalid_code",
            "the code is unknown, used or expired",
        ));
    }
    state
        .audit(
            if body.approve {
                "device_code_approved"
            } else {
                "device_code_denied"
            },
            Some(&ctx.account_id),
            ctx.device_id.as_deref(),
            Some(ip.0),
            Some(row.client_name),
        )
        .await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct TokenBody {
    device_code: String,
}

pub async fn token(
    State(state): State<AppState>,
    ip: ClientIp,
    ApiJson(body): ApiJson<TokenBody>,
) -> ApiResult<Response> {
    let hash = util::token_hash(body.device_code.trim());
    let row = state
        .repo
        .device_code(&hash)
        .await?
        .ok_or_else(|| grant_error("invalid_grant", "unknown device code"))?;
    let now = util::now();
    if now >= row.expires_at {
        return Err(grant_error("expired_token", "the device code has expired"));
    }
    if let Some(last) = row.last_poll_at
        && now - last < row.interval_secs
    {
        let interval = row.interval_secs + 5;
        state
            .repo
            .record_device_code_poll(&hash, now, interval)
            .await?;
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "slow_down",
                "message": format!("polling too fast; wait {interval} seconds between requests"),
                "interval": interval,
            })),
        )
            .into_response());
    }
    state
        .repo
        .record_device_code_poll(&hash, now, row.interval_secs)
        .await?;
    match row.status.as_str() {
        "pending" => Err(grant_error("authorization_pending", "waiting for the user")),
        "denied" => Err(grant_error("access_denied", "the user denied the request")),
        "approved" => {
            let account_id = row
                .account_id
                .ok_or_else(|| ApiError::internal("approved device code without account"))?;
            if !state
                .repo
                .transition_device_code(&hash, "approved", "consumed", None)
                .await?
            {
                return Err(grant_error(
                    "invalid_grant",
                    "the device code was already used",
                ));
            }
            let tokens = issue_tokens(&state, &account_id, None, "device_code").await?;
            state
                .audit(
                    "login",
                    Some(&account_id),
                    None,
                    Some(ip.0),
                    Some("device_code".into()),
                )
                .await;
            Ok(Json(tokens).into_response())
        }
        _ => Err(grant_error(
            "invalid_grant",
            "the device code was already used",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_codes_have_the_expected_shape() {
        for _ in 0..50 {
            let c = new_user_code();
            assert_eq!(c.len(), 9);
            assert_eq!(&c[4..5], "-");
            assert!(
                c.chars()
                    .filter(|ch| *ch != '-')
                    .all(|ch| ALPHABET.contains(&(ch as u8)))
            );
        }
        assert_eq!(normalize_user_code("bcdf-ghjk"), "BCDFGHJK");
    }
}
