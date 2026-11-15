//! Sign-in with Google and Apple (OIDC authorization-code flow with PKCE,
//! state and nonce).
//!
//! `GET /v1/auth/oauth/{provider}/start` redirects to the provider and sets
//! a cookie that binds the flow to this browser. The provider sends the
//! browser back to `/callback` (Apple with a form POST). Without a
//! `return_to`, the callback answers with tokens as JSON; with one, it
//! redirects to `return_to#code=<one-time code>` and the app exchanges the
//! code at `POST /v1/auth/oauth/exchange` (with its own PKCE verifier when
//! it sent a `code_challenge` to `start`).

use crate::auth::{ClientIp, issue_tokens};
use crate::error::{ApiError, ApiJson, ApiResult};
use crate::oidc::{IdClaims, OidcError, ProviderKind};
use crate::repo::Account;
use crate::state::AppState;
use crate::util;
use axum::Json;
use axum::extract::{Form, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

const STATE_TTL: i64 = 600;
const LOGIN_CODE_TTL: i64 = 120;
const COOKIE: &str = "hame_oauth";
const KIND_STATE: &str = "oauth_state";
const KIND_CODE: &str = "oauth_code";

#[derive(Serialize, Deserialize)]
struct FlowState {
    provider: String,
    nonce: String,
    verifier: String,
    binding_hash: String,
    return_to: Option<String>,
    app_challenge: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct LoginCode {
    method: String,
    app_challenge: Option<String>,
}

fn provider_kind(state: &AppState, name: &str) -> ApiResult<ProviderKind> {
    let kind = ProviderKind::parse(name).ok_or_else(ApiError::not_found)?;
    if state.provider(kind).is_none() {
        return Err(ApiError::unavailable(format!(
            "{} sign-in is not configured",
            kind.as_str()
        )));
    }
    Ok(kind)
}

fn redirect_uri(state: &AppState, kind: ProviderKind) -> String {
    state.public_link(&format!("/v1/auth/oauth/{}/callback", kind.as_str()))
}

fn pkce_challenge(verifier: &str) -> String {
    util::b64url(&util::sha256(verifier.as_bytes()))
}

fn cookie(state: &AppState, value: &str, max_age: i64) -> String {
    // Apple returns with a cross-site form POST, which only carries
    // SameSite=None cookies; those must be Secure.
    let attrs = if state.config.public_url.scheme() == "https" {
        "SameSite=None; Secure"
    } else {
        "SameSite=Lax"
    };
    format!("{COOKIE}={value}; Path=/v1/auth/oauth/; Max-Age={max_age}; HttpOnly; {attrs}")
}

fn read_cookie(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == COOKIE)
        .map(|(_, v)| v.to_string())
}

#[derive(Deserialize)]
pub struct StartQuery {
    return_to: Option<String>,
    /// The app's own PKCE challenge (S256) for the login-code exchange.
    code_challenge: Option<String>,
}

pub async fn start(
    State(state): State<AppState>,
    Path(provider): Path<String>,
    Query(q): Query<StartQuery>,
) -> ApiResult<Response> {
    let kind = provider_kind(&state, &provider)?;
    if let Some(rt) = &q.return_to
        && !state.config.oauth_return_urls.iter().any(|u| u == rt)
    {
        return Err(ApiError::bad_request(
            "invalid_return_to",
            "return_to is not an allowed URL",
        ));
    }
    if let Some(c) = &q.code_challenge
        && (c.len() != 43 || util::b64url_decode(c).is_none())
    {
        return Err(ApiError::bad_request(
            "invalid_code_challenge",
            "code_challenge must be a base64url SHA-256",
        ));
    }
    // A custom scheme (hame://) can be claimed by any app on the device:
    // the one-time code must then be useless without the app's verifier.
    if let Some(rt) = &q.return_to
        && !rt.starts_with("https://")
        && q.code_challenge.is_none()
    {
        return Err(ApiError::bad_request(
            "code_challenge_required",
            "a return_to that is not https needs the app's code_challenge",
        ));
    }
    let flow_state = util::random_token("");
    let nonce = util::random_token("");
    let verifier = util::random_token("");
    let binding = util::random_token("");
    let data = FlowState {
        provider: kind.as_str().into(),
        nonce: nonce.clone(),
        verifier: verifier.clone(),
        binding_hash: util::token_hash(&binding),
        return_to: q.return_to,
        app_challenge: q.code_challenge,
    };
    let provider = state.provider(kind).ok_or_else(ApiError::not_found)?;
    let url = provider
        .authorization_url(
            &redirect_uri(&state, kind),
            &flow_state,
            &nonce,
            &pkce_challenge(&verifier),
        )
        .await
        .map_err(provider_error)?;
    state
        .repo
        .put_ephemeral(
            KIND_STATE,
            &util::token_hash(&flow_state),
            None,
            &serde_json::to_string(&data).map_err(ApiError::internal)?,
            util::now() + STATE_TTL,
        )
        .await?;
    let mut res = StatusCode::SEE_OTHER.into_response();
    let h = res.headers_mut();
    h.insert(
        header::LOCATION,
        HeaderValue::from_str(&url).map_err(ApiError::internal)?,
    );
    h.insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie(&state, &binding, STATE_TTL)).map_err(ApiError::internal)?,
    );
    Ok(res)
}

fn provider_error(e: OidcError) -> ApiError {
    match e {
        OidcError::InvalidToken(m) => {
            ApiError::new(StatusCode::UNAUTHORIZED, "invalid_id_token", m)
        }
        OidcError::Provider(m) => {
            tracing::warn!(error = %m, "identity provider error");
            ApiError::new(
                StatusCode::BAD_GATEWAY,
                "provider_error",
                "the identity provider could not be reached",
            )
        }
    }
}

#[derive(Deserialize)]
pub struct CallbackParams {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    /// Apple, first sign-in only: `{"name":{"firstName","lastName"},"email"}`.
    user: Option<String>,
}

pub async fn callback_get(
    State(state): State<AppState>,
    Path(provider): Path<String>,
    ip: ClientIp,
    headers: HeaderMap,
    Query(p): Query<CallbackParams>,
) -> ApiResult<Response> {
    callback(state, provider, ip, headers, p).await
}

pub async fn callback_post(
    State(state): State<AppState>,
    Path(provider): Path<String>,
    ip: ClientIp,
    headers: HeaderMap,
    Form(p): Form<CallbackParams>,
) -> ApiResult<Response> {
    callback(state, provider, ip, headers, p).await
}

async fn callback(
    state: AppState,
    provider: String,
    ip: ClientIp,
    headers: HeaderMap,
    p: CallbackParams,
) -> ApiResult<Response> {
    let kind = provider_kind(&state, &provider)?;
    if let Some(err) = p.error {
        return Err(ApiError::bad_request(
            "oauth_denied",
            format!(
                "the provider returned an error: {}",
                err.chars().take(64).collect::<String>()
            ),
        ));
    }
    let invalid_state =
        || ApiError::bad_request("invalid_state", "sign-in state is invalid or expired");
    let (Some(code), Some(flow_state)) = (p.code, p.state) else {
        return Err(ApiError::bad_request(
            "bad_request",
            "code and state are required",
        ));
    };
    let (_, data) = state
        .repo
        .take_ephemeral(KIND_STATE, &util::token_hash(&flow_state), util::now())
        .await?
        .ok_or_else(invalid_state)?;
    let flow: FlowState = serde_json::from_str(&data).map_err(ApiError::internal)?;
    if flow.provider != kind.as_str() {
        return Err(invalid_state());
    }
    let binding = read_cookie(&headers).ok_or_else(invalid_state)?;
    if !util::ct_eq(
        util::token_hash(&binding).as_bytes(),
        flow.binding_hash.as_bytes(),
    ) {
        return Err(invalid_state());
    }
    let provider = state.provider(kind).ok_or_else(ApiError::not_found)?;
    let claims = match provider
        .exchange_and_verify(
            &code,
            &redirect_uri(&state, kind),
            &flow.verifier,
            &flow.nonce,
        )
        .await
    {
        Ok(c) => c,
        Err(e) => {
            state
                .audit(
                    "login_failed",
                    None,
                    None,
                    Some(ip.0),
                    Some(format!("{}: {e}", kind.as_str())),
                )
                .await;
            return Err(provider_error(e));
        }
    };
    let apple_name = p.user.as_deref().and_then(apple_user_name);
    let account_id = resolve_account(&state, kind, &claims, apple_name, ip).await?;

    let clear = cookie(&state, "", 0);
    let mut res = if let Some(return_to) = flow.return_to {
        let code = util::random_token("");
        let data = LoginCode {
            method: kind.as_str().into(),
            app_challenge: flow.app_challenge,
        };
        state
            .repo
            .put_ephemeral(
                KIND_CODE,
                &util::token_hash(&code),
                Some(&account_id),
                &serde_json::to_string(&data).map_err(ApiError::internal)?,
                util::now() + LOGIN_CODE_TTL,
            )
            .await?;
        let mut url = url::Url::parse(&return_to).map_err(ApiError::internal)?;
        url.set_fragment(Some(&format!("code={code}")));
        let mut res = StatusCode::SEE_OTHER.into_response();
        res.headers_mut().insert(
            header::LOCATION,
            HeaderValue::from_str(url.as_str()).map_err(ApiError::internal)?,
        );
        res
    } else {
        let tokens = issue_tokens(&state, &account_id, None, kind.as_str()).await?;
        Json(tokens).into_response()
    };
    res.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&clear).map_err(ApiError::internal)?,
    );
    super::account::notify_new_sign_in(&state, &account_id, ip.0, kind.as_str()).await;
    state
        .audit(
            "login",
            Some(&account_id),
            None,
            Some(ip.0),
            Some(kind.as_str().into()),
        )
        .await;
    Ok(res)
}

fn apple_user_name(user: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(user).ok()?;
    let first = v
        .pointer("/name/firstName")
        .and_then(|x| x.as_str())
        .unwrap_or("");
    let last = v
        .pointer("/name/lastName")
        .and_then(|x| x.as_str())
        .unwrap_or("");
    let name = format!("{first} {last}").trim().to_string();
    (!name.is_empty()).then_some(name)
}

/// Finds or creates the account for a verified identity.
///
/// Linking by email happens only when the provider says the address is
/// verified. If the matching account never verified its address (someone
/// may have registered it in advance to take it over), its password is
/// removed and its sessions revoked before the identity is linked.
async fn resolve_account(
    state: &AppState,
    kind: ProviderKind,
    claims: &IdClaims,
    apple_name: Option<String>,
    ip: ClientIp,
) -> ApiResult<String> {
    let provider = kind.as_str();
    if let Some(id) = state.repo.identity_account(provider, &claims.sub).await? {
        return Ok(id);
    }
    let email = claims
        .email
        .as_deref()
        .and_then(util::normalize_email)
        .filter(|_| claims.email_is_verified())
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::FORBIDDEN,
                "email_not_verified",
                "the provider did not return a verified email address",
            )
        })?;
    let now = util::now();
    let account_id = match state.repo.account_by_email(&email).await? {
        Some(existing) => {
            // Keep the password only if the verified owner has signed in
            // with it: otherwise whoever registered the address may know
            // it (pre-account hijacking).
            let password_unproven = existing.password_hash.is_some() && !existing.password_proven;
            if !existing.email_verified || password_unproven {
                state
                    .repo
                    .set_password(&existing.id, None, false, now)
                    .await?;
                state.repo.set_email_verified(&existing.id, now).await?;
                let devices = state
                    .repo
                    .revoke_account_sessions(&existing.id, None, "account_claimed_by_provider", now)
                    .await?;
                super::account::kick_all(state, &devices);
                state
                    .audit(
                        "password_removed_on_claim",
                        Some(&existing.id),
                        None,
                        Some(ip.0),
                        Some(provider.into()),
                    )
                    .await;
            }
            super::account::delete_email_links(state, &existing.id).await?;
            existing.id
        }
        None => {
            let name = claims
                .name
                .clone()
                .or(apple_name)
                .and_then(|n| super::account::validate_name(&n).ok())
                .unwrap_or_else(|| email.split('@').next().unwrap_or("user").to_string());
            let account = Account {
                id: uuid::Uuid::new_v4().to_string(),
                name,
                email: email.clone(),
                email_verified: true,
                password_hash: None,
                password_proven: false,
                created_at: now,
            };
            state.repo.create_account(&account).await?;
            state
                .audit(
                    "signup",
                    Some(&account.id),
                    None,
                    Some(ip.0),
                    Some(provider.into()),
                )
                .await;
            account.id
        }
    };
    state
        .repo
        .link_identity(provider, &claims.sub, &account_id, Some(&email), now)
        .await?;
    state
        .audit(
            "identity_linked",
            Some(&account_id),
            None,
            Some(ip.0),
            Some(provider.into()),
        )
        .await;
    let name = match kind {
        ProviderKind::Google => "Google",
        ProviderKind::Apple => "Apple",
    };
    state
        .notify_account(
            &account_id,
            &format!("{name} sign-in was linked to your hame account"),
            &format!(
                "Sign-in with {name} was linked to your account. If your account had a password \
                 you never used, it was removed; set one with \"forgot password\" if you want one. \
                 If your account has a password you did not set yourself, reset it now."
            ),
        )
        .await;
    Ok(account_id)
}

#[derive(Deserialize)]
pub struct ExchangeBody {
    code: String,
    code_verifier: Option<String>,
}

pub async fn exchange(
    State(state): State<AppState>,
    ApiJson(body): ApiJson<ExchangeBody>,
) -> ApiResult<Json<crate::auth::TokenResponse>> {
    let invalid = || ApiError::bad_request("invalid_code", "the code is invalid or expired");
    let (account_id, data) = state
        .repo
        .take_ephemeral(KIND_CODE, &util::token_hash(body.code.trim()), util::now())
        .await?
        .ok_or_else(invalid)?;
    let account_id = account_id.ok_or_else(invalid)?;
    let code: LoginCode = serde_json::from_str(&data).map_err(ApiError::internal)?;
    if let Some(challenge) = &code.app_challenge {
        let verifier = body.code_verifier.as_deref().ok_or_else(invalid)?;
        if !util::ct_eq(pkce_challenge(verifier).as_bytes(), challenge.as_bytes()) {
            return Err(invalid());
        }
    }
    Ok(Json(
        issue_tokens(&state, &account_id, None, &code.method).await?,
    ))
}
