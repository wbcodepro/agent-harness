//! End-to-end encrypted sync between hame hosts and client devices through
//! an untrusted relay (docs/design/APPS-AND-SYNC.md, §2 to §4).
//!
//! - [`keys`]: Ed25519 + X25519 identities and fingerprints;
//! - [`envelope`]: the sealed, signed envelope every frame travels in;
//! - [`pairing`]: QR offers, HMAC proofs, the short authentication string
//!   and signed grants;
//! - [`store`]: the host's identity and allowlist on disk (mode 0600);
//! - [`frames`]: relay WebSocket frames and the remote API frames;
//! - [`relay`]: a relay WebSocket connection with resume, ack, ping and
//!   reconnection;
//! - [`relay_api`]: the relay's HTTP API a host needs (device-code
//!   sign-in, registration, grants);
//! - [`connector`]: the host side, which runs remote requests against the
//!   hame web router in process and nothing else;
//! - [`client`]: a device side in Rust, for tests and tools;
//! - `mock_relay` (feature `mock-relay`): a faithful relay for tests.

pub mod approval;
pub mod client;
pub mod connector;
pub mod envelope;
pub mod frames;
pub mod jcs;
pub mod keys;
#[cfg(feature = "mock-relay")]
pub mod mock_relay;
pub mod pairing;
pub mod relay;
pub mod relay_api;
pub mod store;
pub mod text;

use std::path::{Path, PathBuf};

pub use connector::{Connector, ConnectorOptions, PairingPrompt};
pub use envelope::{Header, Sealed};
pub use keys::{Identity, PublicIdentity};
pub use pairing::PairingOffer;
pub use store::HostStore;

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("malformed: {0}")]
    Malformed(&'static str),
    #[error("too large")]
    TooLarge,
    #[error("crypto: {0}")]
    Crypto(&'static str),
    #[error("bad signature")]
    BadSignature,
    #[error("envelope addressed to another device")]
    WrongRecipient,
    #[error("unknown sender {0}")]
    UnknownSender(String),
    #[error("replayed or reordered envelope (counter {counter}, last {last})")]
    Replay { counter: u64, last: u64 },
    #[error(
        "device {0} presented a different key than the one it was paired with; refused. \
         Revoke it and pair again if the device was reset."
    )]
    KeyChanged(String),
    #[error("pairing: {0}")]
    Pairing(String),
    #[error(
        "{path} is readable or writable by other users (mode {mode:o}); run `chmod 600` on \
         files and `chmod 700` on the directory"
    )]
    InsecurePermissions { path: PathBuf, mode: u32 },
    #[error("store: {0}")]
    Store(String),
    #[error("relay: {0}")]
    Relay(String),
    #[error("the relay refused the sign-in; run `hame connect --relay <url>` to sign in again")]
    Unauthorized,
    #[error("closed")]
    Closed,
}

impl SyncError {
    pub(crate) fn io(path: &Path, error: std::io::Error) -> Self {
        SyncError::Store(format!("{}: {error}", path.display()))
    }
}
