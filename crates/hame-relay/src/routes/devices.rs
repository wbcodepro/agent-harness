//! Devices, hosts, host grants and push tokens.

use crate::auth::{Authed, ClientIp, FullAuthed};
use crate::error::{ApiError, ApiJson, ApiResult};
use crate::push::PushProvider;
use crate::repo::{AuthContext, Device, SCOPE_HOST};
use crate::state::AppState;
use crate::util;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};

const KINDS: &[&str] = &["web", "desktop", "ios", "android", "host"];
/// How far a rebind proof's timestamp may be from the relay's clock.
const REBIND_SKEW: i64 = 300;

fn decode_key(field: &str, value: &str) -> ApiResult<Vec<u8>> {
    util::b64url_decode(value)
        .filter(|k| k.len() == 32)
        .ok_or_else(|| {
            ApiError::bad_request(
                "invalid_key",
                format!("{field} must be a 32-byte public key, base64url"),
            )
        })
}

/// Whether `key` is a canonical encoding of an Ed25519 point that is not of
/// small order.
pub fn valid_ed25519_key(key: &[u8]) -> bool {
    use curve25519_dalek::edwards::CompressedEdwardsY;
    let Ok(bytes) = <[u8; 32]>::try_from(key) else {
        return false;
    };
    match CompressedEdwardsY(bytes).decompress() {
        Some(p) => !p.is_small_order() && p.compress().to_bytes() == bytes,
        None => false,
    }
}

/// Whether `key` is an X25519 public key of large order (all-zero and
/// the other low-order points give a predictable shared secret).
pub fn valid_x25519_key(key: &[u8]) -> bool {
    use curve25519_dalek::montgomery::MontgomeryPoint;
    use curve25519_dalek::scalar::Scalar;
    let Ok(bytes) = <[u8; 32]>::try_from(key) else {
        return false;
    };
    // A point of order dividing the cofactor maps to the identity (u = 0).
    let p = MontgomeryPoint(bytes) * Scalar::from(8u8);
    p.to_bytes() != [0u8; 32]
}

/// Verifies an Ed25519 signature with a base64url public key.
pub fn verify_ed25519(public_key_b64: &str, message: &[u8], signature: &[u8]) -> bool {
    use aws_lc_rs::signature::{ED25519, UnparsedPublicKey};
    let Some(pk) = util::b64url_decode(public_key_b64) else {
        return false;
    };
    UnparsedPublicKey::new(&ED25519, pk)
        .verify(message, signature)
        .is_ok()
}

fn fingerprint(d: &Device) -> String {
    let ed = util::b64url_decode(&d.ed25519).unwrap_or_default();
    let x = util::b64url_decode(&d.x25519).unwrap_or_default();
    util::key_fingerprint(&ed, &x)
}

fn device_json(state: &AppState, d: &Device) -> Value {
    json!({
        "id": d.id,
        "name": d.name,
        "kind": d.kind,
        "ed25519": d.ed25519,
        "x25519": d.x25519,
        "fingerprint": fingerprint(d),
        "online": state.hub.is_online(&d.id),
        "created_at": d.created_at,
        "last_seen_at": d.last_seen_at,
        "push": d.push_provider,
    })
}

/// A device of the caller's account that is not revoked.
async fn own_device(state: &AppState, ctx: &AuthContext, id: &str) -> ApiResult<Device> {
    state
        .repo
        .device(id)
        .await?
        .filter(|d| d.account_id == ctx.account_id && d.revoked_at.is_none())
        .ok_or_else(ApiError::not_found)
}

#[derive(Deserialize)]
pub struct RegisterBody {
    name: String,
    kind: String,
    ed25519: String,
    x25519: String,
}

/// Registers the calling session's device. A session has at most one; an
/// account at most `max_devices_per_account` that are not revoked.
/// Device-code sessions may only register a host.
pub async fn register(
    State(state): State<AppState>,
    ip: ClientIp,
    Authed(ctx): Authed,
    ApiJson(body): ApiJson<RegisterBody>,
) -> ApiResult<Response> {
    if ctx.device_id.is_some() {
        return Err(ApiError::conflict(
            "session_has_device",
            "this session already belongs to a device",
        ));
    }
    let name = super::account::validate_name(&body.name)?;
    if !KINDS.contains(&body.kind.as_str()) {
        return Err(ApiError::bad_request(
            "invalid_kind",
            "kind must be web, desktop, ios, android or host",
        ));
    }
    if ctx.scope == SCOPE_HOST && body.kind != "host" {
        return Err(ApiError::host_scope());
    }
    if !valid_ed25519_key(&decode_key("ed25519", &body.ed25519)?) {
        return Err(ApiError::bad_request(
            "invalid_key",
            "ed25519 is not a valid public key",
        ));
    }
    if !valid_x25519_key(&decode_key("x25519", &body.x25519)?) {
        return Err(ApiError::bad_request(
            "invalid_key",
            "x25519 is a low-order point",
        ));
    }
    let device = Device {
        id: uuid::Uuid::new_v4().to_string(),
        account_id: ctx.account_id.clone(),
        name,
        kind: body.kind,
        ed25519: body.ed25519,
        x25519: body.x25519,
        created_at: util::now(),
        last_seen_at: None,
        revoked_at: None,
        last_seq: 0,
        acked_seq: 0,
        push_provider: None,
        push_token: None,
    };
    if !state
        .repo
        .create_device(&device, state.config.max_devices_per_account)
        .await?
    {
        return Err(ApiError::conflict(
            "too_many_devices",
            format!(
                "an account may have at most {} devices; revoke one first",
                state.config.max_devices_per_account
            ),
        ));
    }
    state
        .repo
        .bind_session_device(&ctx.session_id, &device.id)
        .await?;
    state
        .audit(
            "device_added",
            Some(&ctx.account_id),
            Some(&device.id),
            Some(ip.0),
            Some(format!("{} {}", device.kind, fingerprint(&device))),
        )
        .await;
    state
        .notify_account(
            &ctx.account_id,
            "A device was added to your hame account",
            &format!(
                "A {} device named \"{}\" was added to your account (key fingerprint {}).",
                device.kind,
                device.name,
                &fingerprint(&device)[..16]
            ),
        )
        .await;
    Ok((StatusCode::CREATED, Json(device_json(&state, &device))).into_response())
}

pub async fn list(State(state): State<AppState>, Authed(ctx): Authed) -> ApiResult<Json<Value>> {
    let devices = state.repo.devices(&ctx.account_id).await?;
    let items: Vec<Value> = devices.iter().map(|d| device_json(&state, d)).collect();
    Ok(Json(json!({ "devices": items })))
}

pub async fn hosts(State(state): State<AppState>, Authed(ctx): Authed) -> ApiResult<Json<Value>> {
    let devices = state.repo.devices(&ctx.account_id).await?;
    let items: Vec<Value> = devices
        .iter()
        .filter(|d| d.kind == "host")
        .map(|d| device_json(&state, d))
        .collect();
    Ok(Json(json!({ "hosts": items })))
}

pub async fn revoke(
    State(state): State<AppState>,
    ip: ClientIp,
    FullAuthed(ctx): FullAuthed,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let device = own_device(&state, &ctx, &id).await?;
    state.repo.revoke_device(&device.id, util::now()).await?;
    state.hub.kick(&device.id);
    state
        .audit(
            "device_revoked",
            Some(&ctx.account_id),
            Some(&device.id),
            Some(ip.0),
            ctx.device_id.as_ref().map(|d| format!("by {d}")),
        )
        .await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct RebindBody {
    timestamp: i64,
    /// Ed25519 signature, base64url, over
    /// `hame-relay-rebind:v1\n{account_id}\n{device_id}\n{session_id}\n{timestamp}`.
    signature: String,
}

/// The message a device signs to attach a new session to itself.
pub fn rebind_message(
    account_id: &str,
    device_id: &str,
    session_id: &str,
    timestamp: i64,
) -> String {
    format!("hame-relay-rebind:v1\n{account_id}\n{device_id}\n{session_id}\n{timestamp}")
}

/// Attaches the calling session to an existing device after a new sign-in
/// (for example after a password reset revoked its sessions), so that the
/// device keeps its id, its queue and its place on hosts' allowlists. The
/// device proves possession of its Ed25519 key.
pub async fn rebind(
    State(state): State<AppState>,
    ip: ClientIp,
    Authed(ctx): Authed,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<RebindBody>,
) -> ApiResult<StatusCode> {
    if ctx.device_id.is_some() {
        return Err(ApiError::conflict(
            "session_has_device",
            "this session already belongs to a device",
        ));
    }
    let device = own_device(&state, &ctx, &id).await?;
    if ctx.scope == SCOPE_HOST && device.kind != "host" {
        return Err(ApiError::host_scope());
    }
    if (util::now() - body.timestamp).abs() > REBIND_SKEW {
        return Err(ApiError::bad_request(
            "invalid_proof",
            "timestamp out of range",
        ));
    }
    let msg = rebind_message(&ctx.account_id, &device.id, &ctx.session_id, body.timestamp);
    let sig = util::b64url_decode(&body.signature).unwrap_or_default();
    if !verify_ed25519(&device.ed25519, msg.as_bytes(), &sig) {
        return Err(ApiError::forbidden(
            "the signature does not match the device key",
        ));
    }
    state
        .repo
        .bind_session_device(&ctx.session_id, &device.id)
        .await?;
    state
        .audit(
            "device_rebound",
            Some(&ctx.account_id),
            Some(&device.id),
            Some(ip.0),
            None,
        )
        .await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct PushBody {
    /// `apns`, `fcm`, or null to remove the token.
    provider: Option<String>,
    token: Option<String>,
}

/// Registers this device's push token (only the device itself may).
pub async fn set_push(
    State(state): State<AppState>,
    Authed(ctx): Authed,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<PushBody>,
) -> ApiResult<StatusCode> {
    let device = own_device(&state, &ctx, &id).await?;
    if ctx.device_id.as_deref() != Some(device.id.as_str()) {
        return Err(ApiError::forbidden(
            "only the device itself may set its push token",
        ));
    }
    match (body.provider.as_deref(), body.token.as_deref()) {
        (None, _) => state.repo.set_push_token(&device.id, None, None).await?,
        (Some(p), Some(token)) => {
            let provider = PushProvider::parse(p).ok_or_else(|| {
                ApiError::bad_request("invalid_provider", "provider must be apns or fcm")
            })?;
            if token.is_empty()
                || token.len() > 4096
                || !token.bytes().all(|b| b.is_ascii_graphic())
            {
                return Err(ApiError::bad_request(
                    "invalid_token",
                    "malformed push token",
                ));
            }
            state
                .repo
                .set_push_token(&device.id, Some(provider.as_str()), Some(token))
                .await?
        }
        (Some(_), None) => {
            return Err(ApiError::bad_request("invalid_token", "token is required"));
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

/// The bytes a host signs for a grant list.
pub fn grants_message(host_id: &str, version: i64, document: &[u8]) -> Vec<u8> {
    let mut m = format!("hame-grants:v1\n{host_id}\n{version}\n").into_bytes();
    m.extend_from_slice(document);
    m
}

#[derive(Deserialize)]
pub struct GrantsBody {
    /// Increases with every new list; the relay refuses one that is not
    /// higher than the stored version (no rollback to an older list).
    version: i64,
    /// The host's grant list, opaque to the relay, base64url.
    document: String,
    /// The host's Ed25519 signature, base64url, over
    /// `hame-grants:v1\n{host_id}\n{version}\n` followed by the document.
    signature: String,
}

/// Publishes the host's signed list of accepted client devices. Only the
/// host may; the signature must verify with its registered key. Clients
/// verify it again against the key they pinned at pairing.
pub async fn put_grants(
    State(state): State<AppState>,
    ip: ClientIp,
    Authed(ctx): Authed,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<GrantsBody>,
) -> ApiResult<StatusCode> {
    let host = own_device(&state, &ctx, &id).await?;
    if host.kind != "host" || ctx.device_id.as_deref() != Some(host.id.as_str()) {
        return Err(ApiError::forbidden(
            "only the host itself may publish its grants",
        ));
    }
    let document = util::b64url_decode(&body.document)
        .ok_or_else(|| ApiError::bad_request("bad_request", "document must be base64url"))?;
    let signature = util::b64url_decode(&body.signature)
        .ok_or_else(|| ApiError::bad_request("bad_request", "signature must be base64url"))?;
    if body.version < 1 {
        return Err(ApiError::bad_request(
            "bad_request",
            "version must be positive",
        ));
    }
    let message = grants_message(&host.id, body.version, &document);
    if !verify_ed25519(&host.ed25519, &message, &signature) {
        return Err(ApiError::bad_request(
            "invalid_signature",
            "signature does not verify",
        ));
    }
    if !state
        .repo
        .put_grants(&host.id, body.version, &document, &signature, util::now())
        .await?
    {
        return Err(ApiError::conflict(
            "stale_version",
            "a grant list with this or a higher version is already stored",
        ));
    }
    state
        .audit(
            "grants_changed",
            Some(&ctx.account_id),
            Some(&host.id),
            Some(ip.0),
            None,
        )
        .await;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn get_grants(
    State(state): State<AppState>,
    Authed(ctx): Authed,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let host = own_device(&state, &ctx, &id).await?;
    let grants = state
        .repo
        .grants(&host.id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    Ok(Json(json!({
        "host_id": host.id,
        "version": grants.version,
        "document": util::b64url(&grants.document),
        "signature": util::b64url(&grants.signature),
        "updated_at": grants.updated_at,
    })))
}
