//! Passkeys (WebAuthn) with webauthn-rs.
//!
//! Login starts either with an email (the challenge lists that account's
//! credentials, or stable fake ones for an unknown address, so the answer
//! does not reveal whether it is registered) or without one (a
//! discoverable-credential challenge for conditional UI).

use crate::auth::{Authed, ClientIp, TokenResponse, issue_tokens};
use crate::error::{ApiError, ApiJson, ApiResult};
use crate::repo::StoredPasskey;
use crate::state::AppState;
use crate::util;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use webauthn_rs::Webauthn;
use webauthn_rs::prelude::{
    DiscoverableAuthentication, Passkey, PasskeyRegistration, PublicKeyCredential,
    RegisterPublicKeyCredential, Uuid,
};

const CHALLENGE_TTL: i64 = 300;
const KIND_REG: &str = "passkey_reg";
const KIND_AUTH: &str = "passkey_auth";

fn webauthn(state: &AppState) -> ApiResult<&Webauthn> {
    state
        .webauthn
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("passkeys are not configured"))
}

pub async fn register_start(
    State(state): State<AppState>,
    Authed(ctx): Authed,
) -> ApiResult<Json<Value>> {
    let wa = webauthn(&state)?;
    let account = state
        .repo
        .account(&ctx.account_id)
        .await?
        .ok_or_else(ApiError::unauthorized)?;
    let user_id = Uuid::parse_str(&account.id).map_err(ApiError::internal)?;
    let mut exclude = Vec::new();
    for p in state.repo.passkeys(&account.id).await? {
        if let Ok(pk) = serde_json::from_str::<Passkey>(&p.data) {
            exclude.push(pk.cred_id().clone());
        }
    }
    let (options, reg) = wa
        .start_passkey_registration(
            user_id,
            &account.email,
            &account.name,
            (!exclude.is_empty()).then_some(exclude),
        )
        .map_err(ApiError::internal)?;
    let challenge_id = util::random_token("");
    state
        .repo
        .put_ephemeral(
            KIND_REG,
            &util::token_hash(&challenge_id),
            Some(&account.id),
            &serde_json::to_string(&reg).map_err(ApiError::internal)?,
            util::now() + CHALLENGE_TTL,
        )
        .await?;
    Ok(Json(
        json!({ "challenge_id": challenge_id, "options": options }),
    ))
}

#[derive(Deserialize)]
pub struct RegisterFinish {
    challenge_id: String,
    credential: RegisterPublicKeyCredential,
    name: Option<String>,
}

pub async fn register_finish(
    State(state): State<AppState>,
    ip: ClientIp,
    Authed(ctx): Authed,
    ApiJson(body): ApiJson<RegisterFinish>,
) -> ApiResult<Json<Value>> {
    let wa = webauthn(&state)?;
    let invalid = || ApiError::bad_request("invalid_challenge", "challenge is invalid or expired");
    let (owner, data) = state
        .repo
        .take_ephemeral(KIND_REG, &util::token_hash(&body.challenge_id), util::now())
        .await?
        .ok_or_else(invalid)?;
    if owner.as_deref() != Some(ctx.account_id.as_str()) {
        return Err(invalid());
    }
    let reg: PasskeyRegistration = serde_json::from_str(&data).map_err(ApiError::internal)?;
    let passkey = wa
        .finish_passkey_registration(&body.credential, &reg)
        .map_err(|e| {
            ApiError::bad_request("invalid_credential", format!("registration failed: {e}"))
        })?;
    let name = body
        .name
        .as_deref()
        .map(super::account::validate_name)
        .transpose()?
        .unwrap_or_else(|| "Passkey".into());
    let stored = StoredPasskey {
        id: uuid::Uuid::new_v4().to_string(),
        account_id: ctx.account_id.clone(),
        credential_id: util::b64url(passkey.cred_id()),
        name: name.clone(),
        data: serde_json::to_string(&passkey).map_err(ApiError::internal)?,
        created_at: util::now(),
        last_used_at: None,
    };
    state
        .repo
        .insert_passkey(&stored)
        .await
        .map_err(|e| match e {
            crate::repo::RepoError::Conflict => {
                ApiError::conflict("passkey_exists", "this passkey is already registered")
            }
            e => e.into(),
        })?;
    state
        .audit(
            "passkey_added",
            Some(&ctx.account_id),
            ctx.device_id.as_deref(),
            Some(ip.0),
            None,
        )
        .await;
    Ok(Json(json!({ "id": stored.id, "name": name })))
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct LoginStart {
    email: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct AuthState {
    state: DiscoverableAuthentication,
}

pub async fn login_start(
    State(state): State<AppState>,
    ApiJson(body): ApiJson<LoginStart>,
) -> ApiResult<Json<Value>> {
    let wa = webauthn(&state)?;
    let (rcr, auth) = wa
        .start_discoverable_authentication()
        .map_err(ApiError::internal)?;
    let mut options = serde_json::to_value(&rcr).map_err(ApiError::internal)?;
    if let Some(raw) = body.email.as_deref() {
        let email = util::normalize_email(raw)
            .ok_or_else(|| ApiError::bad_request("invalid_email", "not a valid email address"))?;
        state.check_account_limit("passkey", &email)?;
        let mut ids: Vec<String> = Vec::new();
        if let Some(account) = state.repo.account_by_email(&email).await? {
            ids = state
                .repo
                .passkeys(&account.id)
                .await?
                .into_iter()
                .map(|p| p.credential_id)
                .collect();
        }
        if ids.is_empty() {
            // A stable fake credential: the same answer every time for this
            // address, like a real account with one passkey.
            let mut seed = state.enumeration_key.to_vec();
            seed.extend_from_slice(b"passkey:");
            seed.extend_from_slice(email.as_bytes());
            ids.push(util::b64url(&util::sha256(&seed)));
        }
        let allow: Vec<Value> = ids
            .iter()
            .map(|id| json!({ "type": "public-key", "id": id }))
            .collect();
        if let Some(obj) = options.as_object_mut() {
            obj.remove("mediation");
            if let Some(pk) = obj.get_mut("publicKey").and_then(Value::as_object_mut) {
                pk.insert("allowCredentials".into(), Value::Array(allow));
            }
        }
    }
    let challenge_id = util::random_token("");
    state
        .repo
        .put_ephemeral(
            KIND_AUTH,
            &util::token_hash(&challenge_id),
            None,
            &serde_json::to_string(&AuthState { state: auth }).map_err(ApiError::internal)?,
            util::now() + CHALLENGE_TTL,
        )
        .await?;
    Ok(Json(
        json!({ "challenge_id": challenge_id, "options": options }),
    ))
}

#[derive(Deserialize)]
pub struct LoginFinish {
    challenge_id: String,
    credential: PublicKeyCredential,
}

pub async fn login_finish(
    State(state): State<AppState>,
    ip: ClientIp,
    ApiJson(body): ApiJson<LoginFinish>,
) -> ApiResult<Json<TokenResponse>> {
    let wa = webauthn(&state)?;
    let fail = || {
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "invalid_credentials",
            "passkey sign-in failed",
        )
    };
    let (_, data) = state
        .repo
        .take_ephemeral(
            KIND_AUTH,
            &util::token_hash(&body.challenge_id),
            util::now(),
        )
        .await?
        .ok_or_else(|| {
            ApiError::bad_request("invalid_challenge", "challenge is invalid or expired")
        })?;
    let auth: AuthState = serde_json::from_str(&data).map_err(ApiError::internal)?;
    let cred_id = util::b64url(&body.credential.raw_id);
    let Some(stored) = state.repo.passkey_by_credential(&cred_id).await? else {
        state
            .audit(
                "login_failed",
                None,
                None,
                Some(ip.0),
                Some("passkey: unknown credential".into()),
            )
            .await;
        return Err(fail());
    };
    if let Some(handle) = body.credential.get_user_unique_id() {
        let expected = Uuid::parse_str(&stored.account_id).map_err(ApiError::internal)?;
        if handle != expected.as_bytes() {
            return Err(fail());
        }
    }
    let mut passkey: Passkey = serde_json::from_str(&stored.data).map_err(ApiError::internal)?;
    let result =
        match wa.finish_discoverable_authentication_v2(&body.credential, auth.state, &passkey) {
            Ok(r) => r,
            Err(e) => {
                state
                    .audit(
                        "login_failed",
                        Some(&stored.account_id),
                        None,
                        Some(ip.0),
                        Some(format!("passkey: {e}")),
                    )
                    .await;
                return Err(fail());
            }
        };
    passkey.update_credential(&result);
    state
        .repo
        .update_passkey_data(
            &stored.id,
            &serde_json::to_string(&passkey).map_err(ApiError::internal)?,
            util::now(),
        )
        .await?;
    let tokens = issue_tokens(&state, &stored.account_id, None, "passkey").await?;
    state
        .audit(
            "login",
            Some(&stored.account_id),
            None,
            Some(ip.0),
            Some("passkey".into()),
        )
        .await;
    Ok(Json(tokens))
}

pub async fn list(State(state): State<AppState>, Authed(ctx): Authed) -> ApiResult<Json<Value>> {
    let items: Vec<Value> = state
        .repo
        .passkeys(&ctx.account_id)
        .await?
        .into_iter()
        .map(|p| {
            json!({
                "id": p.id,
                "name": p.name,
                "created_at": p.created_at,
                "last_used_at": p.last_used_at,
            })
        })
        .collect();
    Ok(Json(json!({ "passkeys": items })))
}

pub async fn remove(
    State(state): State<AppState>,
    ip: ClientIp,
    Authed(ctx): Authed,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    if !state.repo.delete_passkey(&ctx.account_id, &id).await? {
        return Err(ApiError::not_found());
    }
    state
        .audit(
            "passkey_removed",
            Some(&ctx.account_id),
            ctx.device_id.as_deref(),
            Some(ip.0),
            None,
        )
        .await;
    Ok(StatusCode::NO_CONTENT)
}
