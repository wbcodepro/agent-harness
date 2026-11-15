//! The storage layer. Handlers talk to [`Repository`]; [`sqlite`] is the
//! one implementation today, and a PostgreSQL backend can implement the
//! same trait.

pub mod sqlite;

use async_trait::async_trait;
use serde::Serialize;

/// A storage error.
#[derive(Debug, thiserror::Error)]
pub enum RepoError {
    /// A unique constraint was violated.
    #[error("conflict")]
    Conflict,
    #[error("database: {0}")]
    Db(String),
}

pub type RepoResult<T> = Result<T, RepoError>;

#[derive(Clone, Debug)]
pub struct Account {
    pub id: String,
    pub name: String,
    /// Normalized (trimmed, lower case).
    pub email: String,
    pub email_verified: bool,
    /// Argon2id PHC string; `None` for accounts that only sign in with a
    /// provider or a passkey.
    pub password_hash: Option<String>,
    pub created_at: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Identity {
    pub provider: String,
    pub email: Option<String>,
    pub created_at: i64,
}

#[derive(Clone, Debug)]
pub struct Device {
    pub id: String,
    pub account_id: String,
    pub name: String,
    /// web, desktop, ios, android or host.
    pub kind: String,
    /// base64url public keys.
    pub ed25519: String,
    pub x25519: String,
    pub created_at: i64,
    pub last_seen_at: Option<i64>,
    pub revoked_at: Option<i64>,
    pub last_seq: i64,
    pub acked_seq: i64,
    pub push_provider: Option<String>,
    pub push_token: Option<String>,
}

/// Who a valid access token belongs to.
#[derive(Clone, Debug)]
pub struct AuthContext {
    pub session_id: String,
    pub account_id: String,
    pub device_id: Option<String>,
}

/// The result of presenting a refresh token.
#[derive(Debug)]
pub enum RefreshOutcome {
    /// The token was valid; it is now used and the new one is stored.
    Rotated(AuthContext),
    /// The token had already been used: the whole session was revoked.
    Reused(AuthContext),
    /// Unknown, expired or revoked.
    Invalid,
}

#[derive(Debug)]
pub enum EnqueueOutcome {
    Queued(i64),
    /// The recipient's queue holds the maximum number of envelopes.
    Full,
}

#[derive(Clone, Debug)]
pub struct MailItem {
    pub seq: i64,
    pub sender_id: String,
    pub envelope: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct Grants {
    pub document: Vec<u8>,
    pub signature: Vec<u8>,
    pub updated_at: i64,
}

#[derive(Clone, Debug)]
pub struct StoredPasskey {
    pub id: String,
    pub account_id: String,
    /// base64url credential id.
    pub credential_id: String,
    pub name: String,
    /// The serialized `webauthn_rs::prelude::Passkey` (public key material
    /// and counter; nothing secret).
    pub data: String,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct DeviceCodeRow {
    pub device_code_hash: String,
    pub user_code_hash: String,
    pub client_name: String,
    /// pending, approved, denied or consumed.
    pub status: String,
    pub account_id: Option<String>,
    pub interval_secs: i64,
    pub last_poll_at: Option<i64>,
    pub expires_at: i64,
    pub created_at: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct AuditEntry {
    pub at: i64,
    pub event: String,
    pub account_id: Option<String>,
    pub device_id: Option<String>,
    pub ip: Option<String>,
    pub detail: Option<String>,
}

/// Everything the relay stores.
#[async_trait]
pub trait Repository: Send + Sync + 'static {
    // Accounts.
    async fn create_account(&self, account: &Account) -> RepoResult<()>;
    async fn account(&self, id: &str) -> RepoResult<Option<Account>>;
    async fn account_by_email(&self, email: &str) -> RepoResult<Option<Account>>;
    async fn set_email_verified(&self, id: &str, now: i64) -> RepoResult<()>;
    async fn set_password(&self, id: &str, hash: Option<&str>, now: i64) -> RepoResult<()>;
    async fn set_name(&self, id: &str, name: &str, now: i64) -> RepoResult<()>;

    // Email verification and reset tokens.
    async fn insert_email_token(
        &self,
        hash: &str,
        account_id: &str,
        purpose: &str,
        expires_at: i64,
        now: i64,
    ) -> RepoResult<()>;
    /// Marks the token used and returns its account, if it is unused,
    /// unexpired and for `purpose`.
    async fn consume_email_token(
        &self,
        hash: &str,
        purpose: &str,
        now: i64,
    ) -> RepoResult<Option<String>>;
    /// The account of an unused, unexpired token, without consuming it.
    async fn peek_email_token(
        &self,
        hash: &str,
        purpose: &str,
        now: i64,
    ) -> RepoResult<Option<String>>;
    async fn delete_email_tokens(&self, account_id: &str, purpose: &str) -> RepoResult<()>;

    // External identities.
    async fn identity_account(&self, provider: &str, subject: &str) -> RepoResult<Option<String>>;
    async fn link_identity(
        &self,
        provider: &str,
        subject: &str,
        account_id: &str,
        email: Option<&str>,
        now: i64,
    ) -> RepoResult<()>;
    async fn identities(&self, account_id: &str) -> RepoResult<Vec<Identity>>;

    // Sessions and tokens.
    async fn create_session(
        &self,
        id: &str,
        account_id: &str,
        device_id: Option<&str>,
        method: &str,
        now: i64,
    ) -> RepoResult<()>;
    async fn insert_access_token(
        &self,
        hash: &str,
        session_id: &str,
        expires_at: i64,
    ) -> RepoResult<()>;
    async fn insert_refresh_token(
        &self,
        hash: &str,
        session_id: &str,
        expires_at: i64,
        now: i64,
    ) -> RepoResult<()>;
    /// The session behind a valid, unexpired access token whose session and
    /// device are not revoked.
    async fn authenticate(&self, access_hash: &str, now: i64) -> RepoResult<Option<AuthContext>>;
    async fn rotate_refresh_token(
        &self,
        old_hash: &str,
        new_hash: &str,
        new_expires_at: i64,
        now: i64,
    ) -> RepoResult<RefreshOutcome>;
    /// Revokes a session; returns its device, if any.
    async fn revoke_session(&self, id: &str, reason: &str, now: i64) -> RepoResult<Option<String>>;
    /// Revokes every session of an account except `except`; returns the
    /// devices that were bound to them.
    async fn revoke_account_sessions(
        &self,
        account_id: &str,
        except: Option<&str>,
        reason: &str,
        now: i64,
    ) -> RepoResult<Vec<String>>;
    async fn bind_session_device(&self, session_id: &str, device_id: &str) -> RepoResult<()>;

    // Devices and hosts.
    async fn create_device(&self, device: &Device) -> RepoResult<()>;
    async fn device(&self, id: &str) -> RepoResult<Option<Device>>;
    /// The account's devices that are not revoked.
    async fn devices(&self, account_id: &str) -> RepoResult<Vec<Device>>;
    /// Marks the device revoked, revokes its sessions, drops its queue and
    /// push token.
    async fn revoke_device(&self, id: &str, now: i64) -> RepoResult<()>;
    async fn set_push_token(
        &self,
        id: &str,
        provider: Option<&str>,
        token: Option<&str>,
    ) -> RepoResult<()>;
    async fn touch_device(&self, id: &str, now: i64) -> RepoResult<()>;
    async fn put_grants(
        &self,
        host_id: &str,
        document: &[u8],
        signature: &[u8],
        now: i64,
    ) -> RepoResult<()>;
    async fn grants(&self, host_id: &str) -> RepoResult<Option<Grants>>;

    // Mailbox.
    async fn enqueue(
        &self,
        recipient_id: &str,
        sender_id: &str,
        envelope: &[u8],
        now: i64,
        max_queued: i64,
    ) -> RepoResult<EnqueueOutcome>;
    async fn fetch_after(
        &self,
        recipient_id: &str,
        after: i64,
        limit: i64,
    ) -> RepoResult<Vec<MailItem>>;
    /// Drops queued envelopes with `seq <= upto` and records the ack.
    async fn ack(&self, recipient_id: &str, upto: i64) -> RepoResult<()>;

    // Passkeys.
    async fn insert_passkey(&self, passkey: &StoredPasskey) -> RepoResult<()>;
    async fn passkeys(&self, account_id: &str) -> RepoResult<Vec<StoredPasskey>>;
    async fn passkey_by_credential(&self, credential_id: &str)
    -> RepoResult<Option<StoredPasskey>>;
    async fn update_passkey_data(&self, id: &str, data: &str, now: i64) -> RepoResult<()>;
    async fn delete_passkey(&self, account_id: &str, id: &str) -> RepoResult<bool>;

    // Ceremony state.
    async fn put_ephemeral(
        &self,
        kind: &str,
        key: &str,
        account_id: Option<&str>,
        data: &str,
        expires_at: i64,
    ) -> RepoResult<()>;
    /// Removes and returns an unexpired entry: `(account_id, data)`.
    async fn take_ephemeral(
        &self,
        kind: &str,
        key: &str,
        now: i64,
    ) -> RepoResult<Option<(Option<String>, String)>>;

    // Device-code flow.
    async fn insert_device_code(&self, row: &DeviceCodeRow) -> RepoResult<()>;
    async fn device_code(&self, device_code_hash: &str) -> RepoResult<Option<DeviceCodeRow>>;
    async fn device_code_by_user_code(
        &self,
        user_code_hash: &str,
    ) -> RepoResult<Option<DeviceCodeRow>>;
    /// Moves a code from `from` to `to` status (atomically); returns
    /// whether it did.
    async fn transition_device_code(
        &self,
        device_code_hash: &str,
        from: &str,
        to: &str,
        account_id: Option<&str>,
    ) -> RepoResult<bool>;
    async fn record_device_code_poll(
        &self,
        device_code_hash: &str,
        now: i64,
        interval_secs: i64,
    ) -> RepoResult<()>;

    // Audit log.
    async fn audit(&self, entry: &AuditEntry) -> RepoResult<()>;
    async fn audit_for_account(&self, account_id: &str, limit: i64) -> RepoResult<Vec<AuditEntry>>;

    /// Deletes expired tokens, codes, ceremony state and envelopes older
    /// than `mailbox_cutoff`. Returns the number of rows removed.
    async fn purge(&self, now: i64, mailbox_cutoff: i64) -> RepoResult<u64>;

    /// A cheap query to check the database is reachable.
    async fn ping(&self) -> RepoResult<()>;
}
