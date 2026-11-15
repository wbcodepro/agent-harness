# hame-relay

The relay server of hame's apps (design: `docs/design/APPS-AND-SYNC.md`,
sections 3 and 4). It holds accounts and sign-in, the devices and hosts of
each account, and routes end-to-end encrypted envelopes between the
devices of one account over a WebSocket. It never sees prompts, answers,
files or session titles: envelopes are opaque bytes to it.

- API reference: `docs/relay/API.md`
- Deployment and configuration: `docs/relay/DEPLOY.md`
- Threat model: `docs/relay/SECURITY.md`

## Layout

| Path | What |
|---|---|
| `src/config.rs` | configuration from `RELAY_*` environment variables |
| `src/repo/` | `Repository` trait and the SQLite (WAL) implementation; `migrations/` |
| `src/routes/account.rs` | sign-up, verification, login, refresh, logout, reset, profile |
| `src/routes/oauth.rs`, `src/oidc.rs` | Google and Apple sign-in (OIDC, PKCE, JWKS) |
| `src/routes/passkey.rs` | passkeys (webauthn-rs) |
| `src/routes/device_code.rs` | device-code sign-in for hosts and CLIs |
| `src/routes/devices.rs` | devices, hosts, grants, push tokens |
| `src/ws.rs` | WebSocket routing, queues, presence |
| `src/mailer.rs`, `src/push.rs` | SMTP / log / test mailers; APNs, FCM, no-op and test push |
| `src/ratelimit.rs` | per-IP (/64 for IPv6), per-account and global token buckets |
| `src/disk.rs` | free-space floor for new envelopes |

## Run locally

```sh
RELAY_PUBLIC_URL=http://localhost:8080 RELAY_MAILER=log RELAY_LOG_FORMAT=text \
  cargo run -p hame-relay
```

With `RELAY_MAILER=log` (development only; refused with an https public
URL unless `RELAY_DEV=1`), verification and reset emails are
written to the log instead of being sent.

## Tests

```sh
cargo test -p hame-relay
cargo clippy -p hame-relay --all-targets -- -D warnings
```

The integration tests (`tests/`) start a relay on a random port with a
recording mailer and push sender. Google and Apple run against a local
mock identity provider (`tests/common/idp.rs`); passkeys use the
`webauthn-authenticator-rs` software passkey. No test touches the network
beyond 127.0.0.1.

## Dependencies and licenses

Direct dependencies added for this crate (all permissive or MPL-2.0):

| Crate | License | Use |
|---|---|---|
| argon2 | MIT OR Apache-2.0 | password hashing |
| async-trait | MIT OR Apache-2.0 | object-safe async repository trait |
| aws-lc-rs | ISC AND (Apache-2.0 OR ISC) | Ed25519 verification (rebind proofs, host grants) |
| axum (ws) | MIT | HTTP and WebSocket server (already in the workspace) |
| curve25519-dalek | BSD-3-Clause | validating device public keys (Ed25519 points, X25519 order) |
| jsonwebtoken | MIT | ID-token verification, Apple/APNs ES256 and FCM RS256 JWTs |
| lettre | MIT | SMTP |
| rand | MIT OR Apache-2.0 | token randomness |
| reqwest | MIT OR Apache-2.0 | identity providers, APNs, FCM (already in the workspace) |
| sqlx (sqlite) | MIT OR Apache-2.0 | storage, migrations (bundles SQLite, public domain) |
| tower-http | MIT | CORS, headers, limits, tracing |
| tracing, tracing-subscriber | MIT | structured logs |
| webauthn-rs | MPL-2.0 | passkeys |
| nix (Unix only, `fs`) | MIT | free disk space (`statvfs`) |

Test-only: `webauthn-authenticator-rs` (MPL-2.0, software passkey),
`openssl` (Apache-2.0, test keys for the mock identity provider),
`tokio-tungstenite` (MIT), `futures-util` (MIT OR Apache-2.0), `tempfile`
(MIT OR Apache-2.0). The release binary does not link OpenSSL.

MPL-2.0 is file-level copyleft: using the crates unmodified places no
obligation on hame's own code; changes to their files would have to be
published under MPL-2.0.
