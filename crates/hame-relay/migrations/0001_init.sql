-- hame-relay schema, version 1.
-- Times are Unix seconds. Secrets (tokens, codes) are stored as SHA-256
-- hex; passwords as Argon2id PHC strings.

CREATE TABLE accounts (
    id             TEXT PRIMARY KEY,
    name           TEXT NOT NULL,
    email          TEXT NOT NULL UNIQUE,
    email_verified INTEGER NOT NULL DEFAULT 0,
    password_hash  TEXT,
    created_at     INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL
);

-- External identities (Google, Apple) linked to an account.
CREATE TABLE identities (
    provider   TEXT NOT NULL,
    subject    TEXT NOT NULL,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    email      TEXT,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (provider, subject)
);
CREATE INDEX identities_account ON identities(account_id);

-- Email verification and password reset tokens.
CREATE TABLE email_tokens (
    token_hash TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    purpose    TEXT NOT NULL,
    expires_at INTEGER NOT NULL,
    used_at    INTEGER,
    created_at INTEGER NOT NULL
);
CREATE INDEX email_tokens_account ON email_tokens(account_id, purpose);

CREATE TABLE devices (
    id            TEXT PRIMARY KEY,
    account_id    TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    name          TEXT NOT NULL,
    kind          TEXT NOT NULL,
    ed25519       TEXT NOT NULL,
    x25519        TEXT NOT NULL,
    created_at    INTEGER NOT NULL,
    last_seen_at  INTEGER,
    revoked_at    INTEGER,
    last_seq      INTEGER NOT NULL DEFAULT 0,
    acked_seq     INTEGER NOT NULL DEFAULT 0,
    push_provider TEXT,
    push_token    TEXT
);
CREATE INDEX devices_account ON devices(account_id);

-- A session is one token family: one sign-in, optionally bound to a device.
CREATE TABLE sessions (
    id            TEXT PRIMARY KEY,
    account_id    TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    device_id     TEXT REFERENCES devices(id) ON DELETE SET NULL,
    method        TEXT NOT NULL,
    created_at    INTEGER NOT NULL,
    last_used_at  INTEGER NOT NULL,
    revoked_at    INTEGER,
    revoke_reason TEXT
);
CREATE INDEX sessions_account ON sessions(account_id);
CREATE INDEX sessions_device ON sessions(device_id);

CREATE TABLE access_tokens (
    token_hash TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    expires_at INTEGER NOT NULL
);
CREATE INDEX access_tokens_session ON access_tokens(session_id);

-- Used refresh tokens are kept until they expire, to detect reuse.
CREATE TABLE refresh_tokens (
    token_hash TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    expires_at INTEGER NOT NULL,
    used_at    INTEGER,
    created_at INTEGER NOT NULL
);
CREATE INDEX refresh_tokens_session ON refresh_tokens(session_id);

-- The latest grant list a host published (opaque, signed by the host).
CREATE TABLE host_grants (
    host_id    TEXT PRIMARY KEY REFERENCES devices(id) ON DELETE CASCADE,
    document   BLOB NOT NULL,
    signature  BLOB NOT NULL,
    updated_at INTEGER NOT NULL
);

-- Per-recipient queue of opaque envelopes.
CREATE TABLE mailbox (
    recipient_id TEXT NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    seq          INTEGER NOT NULL,
    sender_id    TEXT NOT NULL,
    envelope     BLOB NOT NULL,
    created_at   INTEGER NOT NULL,
    PRIMARY KEY (recipient_id, seq)
);
CREATE INDEX mailbox_created ON mailbox(created_at);

CREATE TABLE passkeys (
    id            TEXT PRIMARY KEY,
    account_id    TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    credential_id TEXT NOT NULL UNIQUE,
    name          TEXT NOT NULL,
    data          TEXT NOT NULL,
    created_at    INTEGER NOT NULL,
    last_used_at  INTEGER
);
CREATE INDEX passkeys_account ON passkeys(account_id);

-- Short-lived ceremony state (OAuth state, WebAuthn challenges, one-time
-- login codes), keyed by the SHA-256 of the value the client holds.
CREATE TABLE ephemeral (
    kind       TEXT NOT NULL,
    key        TEXT NOT NULL,
    account_id TEXT,
    data       TEXT NOT NULL,
    expires_at INTEGER NOT NULL,
    PRIMARY KEY (kind, key)
);

CREATE TABLE device_codes (
    device_code_hash TEXT PRIMARY KEY,
    user_code_hash   TEXT NOT NULL UNIQUE,
    client_name      TEXT NOT NULL,
    status           TEXT NOT NULL,
    account_id       TEXT REFERENCES accounts(id) ON DELETE CASCADE,
    interval_secs    INTEGER NOT NULL,
    last_poll_at     INTEGER,
    expires_at       INTEGER NOT NULL,
    created_at       INTEGER NOT NULL
);

-- Kept when an account is deleted (no foreign key).
CREATE TABLE audit_log (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    at         INTEGER NOT NULL,
    event      TEXT NOT NULL,
    account_id TEXT,
    device_id  TEXT,
    ip         TEXT,
    detail     TEXT
);
CREATE INDEX audit_account ON audit_log(account_id, at);
