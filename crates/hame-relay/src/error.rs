//! The API error type: a status, a stable machine-readable code and a
//! human-readable message, rendered as `{"error": code, "message": ...}`.

use axum::Json;
use axum::extract::rejection::{FormRejection, JsonRejection, QueryRejection};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};

/// An error returned by a handler.
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
    pub retry_after: Option<u64>,
}

pub type ApiResult<T> = Result<T, ApiError>;

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            retry_after: None,
        }
    }

    pub fn bad_request(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, code, message)
    }

    pub fn unauthorized() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "missing, invalid or expired token",
        )
    }

    pub fn invalid_credentials() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "invalid_credentials",
            "email or password is incorrect",
        )
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, "forbidden", message)
    }

    pub fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", "not found")
    }

    pub fn conflict(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, code, message)
    }

    pub fn rate_limited(retry_after: u64) -> Self {
        Self {
            retry_after: Some(retry_after.max(1)),
            ..Self::new(
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
                "too many requests, try again later",
            )
        }
    }

    /// The server is saturated (password hashing queue full).
    pub fn busy() -> Self {
        Self {
            retry_after: Some(1),
            ..Self::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "busy",
                "the server is busy, try again shortly",
            )
        }
    }

    /// The session was not signed in with a full sign-in (device code).
    pub fn host_scope() -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            "insufficient_scope",
            "this session may only register and connect a host; sign in fully first",
        )
    }

    /// The action needs a credential proved in the last minutes.
    pub fn reauth_required() -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            "reauth_required",
            "confirm your password or sign in again first",
        )
    }

    pub fn unavailable(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_configured", message)
    }

    /// An internal failure. The cause is logged; the client sees only a
    /// generic message.
    pub fn internal(cause: impl std::fmt::Display) -> Self {
        tracing::error!(error = %cause, "internal error");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "internal server error",
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = serde_json::json!({ "error": self.code, "message": self.message });
        let mut res = (self.status, Json(body)).into_response();
        if let Some(secs) = self.retry_after
            && let Ok(v) = HeaderValue::from_str(&secs.to_string())
        {
            res.headers_mut().insert(header::RETRY_AFTER, v);
        }
        res
    }
}

impl From<crate::repo::RepoError> for ApiError {
    fn from(e: crate::repo::RepoError) -> Self {
        match e {
            crate::repo::RepoError::Conflict => Self::conflict("conflict", "already exists"),
            crate::repo::RepoError::Db(e) => Self::internal(e),
        }
    }
}

impl From<JsonRejection> for ApiError {
    fn from(e: JsonRejection) -> Self {
        if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
            return Self::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "too_large",
                "request body too large",
            );
        }
        Self::bad_request("bad_request", e.body_text())
    }
}

impl From<FormRejection> for ApiError {
    fn from(e: FormRejection) -> Self {
        Self::bad_request("bad_request", e.body_text())
    }
}

impl From<QueryRejection> for ApiError {
    fn from(e: QueryRejection) -> Self {
        Self::bad_request("bad_request", e.body_text())
    }
}

/// `Json<T>` whose rejection is an [`ApiError`].
#[derive(axum::extract::FromRequest)]
#[from_request(via(axum::Json), rejection(ApiError))]
pub struct ApiJson<T>(pub T);
