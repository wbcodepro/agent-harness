//! Who makes a request: this computer's page (which holds the token), or a
//! remote device whose request the relay connector (`hame connect`) passes
//! to the router in process.

use axum::extract::FromRequestParts;
use axum::http::Request;
use axum::http::request::Parts;
use std::convert::Infallible;

/// A remote device a request comes from. The relay connector attaches it
/// to every request it hands to [`crate::router`] as a request extension;
/// it can never come from HTTP (no header sets an extension), so a request
/// that carries it was made in process by code that already checked the
/// device against the host's allowlist. Requests that carry it pass the
/// token check and get the remote policy (`[remote]` in config.toml).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteCaller {
    pub device_id: String,
    pub label: String,
}

impl RemoteCaller {
    pub fn new(device_id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            device_id: device_id.into(),
            label: label.into(),
        }
    }

    /// Marks `request` as coming from this device.
    pub fn attach<B>(self, request: &mut Request<B>) {
        request.extensions_mut().insert(self);
    }
}

/// Marks `request` as coming from the enrolled remote device `device_id`
/// (same as [`RemoteCaller::attach`]).
pub fn mark_remote<B>(request: &mut Request<B>, device_id: &str, label: &str) {
    RemoteCaller::new(device_id, label).attach(request);
}

/// The caller of one request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Caller {
    /// The page on this computer, with the token.
    Local,
    Remote(RemoteCaller),
}

impl Caller {
    pub fn is_remote(&self) -> bool {
        matches!(self, Caller::Remote(_))
    }

    /// Who, as the session log records it: `local`, or the device id and
    /// its label.
    pub fn by(&self) -> (String, Option<String>) {
        match self {
            Caller::Local => ("local".into(), None),
            Caller::Remote(remote) => (remote.device_id.clone(), Some(remote.label.clone())),
        }
    }
}

impl<S: Send + Sync> FromRequestParts<S> for Caller {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(match parts.extensions.get::<RemoteCaller>() {
            Some(remote) => Caller::Remote(remote.clone()),
            None => Caller::Local,
        })
    }
}
