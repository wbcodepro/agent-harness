# hame-relay security model

The relay is an **untrusted mailbox** (design: `docs/design/APPS-AND-SYNC.md`
section 4). It authenticates accounts and routes envelopes between the
devices of one account; the envelopes are end-to-end encrypted and signed
by the devices, and each host keeps its own allowlist of paired devices.
The goal: a stolen relay database, a relay administrator or a fully
malicious relay can deny service and observe metadata, but cannot read
what users and agents say and **cannot make a host run anything**.

## What the relay can and cannot see

| The relay sees | The relay does not see |
|---|---|
| Account id, name, email, password hash, linked provider and subject | Prompts, answers, files, file names, paths |
| Devices: id, kind, user-chosen name, Ed25519 and X25519 public keys, push token | Tool calls, approvals, screenshots |
| Which device sends to which, when, how many bytes; online status | Session titles, host labels inside envelopes |
| Client IP addresses (audit log, rate limits) | Private keys (generated on devices, never sent) |
| Passwords in clear **while a login request is processed** (TLS ends at the relay) | Anything inside an envelope |
| Host grant documents (signed; content chosen by the host) | |

Apps should keep device names generic (or put the real label inside the
encrypted channel); the name is metadata the relay stores.

## Stored secrets

| Item | Stored as |
|---|---|
| Passwords | Argon2id PHC string, m = 64 MiB, t = 3, p = 1, random salt |
| Access and refresh tokens (256-bit random) | SHA-256 |
| Email verification and reset tokens (256-bit random) | SHA-256, single use, 24 h / 1 h |
| Device codes (256-bit) and user codes | SHA-256 |
| OAuth one-time login codes | SHA-256, 2 min |
| OAuth flow state, WebAuthn challenges | keyed by SHA-256 of the client's value; the record (nonce, PKCE verifier, challenge) is plain but expires in 5–10 min and is deleted on use |
| Passkeys | public key and counter only |
| Envelopes | the ciphertext as received, at most 24 h or until acknowledged |
| Push tokens | plain (needed to send; useless without the APNs/FCM credentials, which are not in the database) |
| Provider secrets, APNs/FCM keys, SMTP password | not in the database: environment or secret files |


## Threats

### Stolen database (read access)

- Password hashes need an offline Argon2id attack per account; weak and
  common passwords are refused at sign-up (length 10+, common list,
  no email or name inside), which raises the cost.
- Tokens and codes are hashes of 256-bit random values: not reversible,
  not usable.
- Envelopes are ciphertext for the recipient's X25519 key.
- **Exposed**: names, emails, device names and kinds, public keys, IP
  addresses in the audit log, the social graph of devices and its timing.
  Mitigation: disk encryption on the VPS, encrypted backups, restricted
  access; audit-log retention (open question below).

### Database tampering, malicious administrator, malicious relay

The relay can:

- deny service: drop, delay or refuse envelopes, revoke devices, lock
  accounts;
- replay or reorder envelopes: hosts reject them by the per-sender
  counter inside the signed envelope header;
- sign in as any user (set a password hash, issue tokens), list and
  revoke their devices;
- add a device to an account or swap the public keys it serves through
  `GET /v1/hosts`: hosts and clients do not trust relay-served keys for
  enrolled peers; they pin keys at pairing, which uses the QR secret and a
  short authentication string confirmed on the host;
- withhold the newest host grant list: clients see the list's signed
  `version` and can notice a stale one; the relay itself refuses to
  replace a list with an older version, so a stolen host session cannot
  roll it back either;
- read passwords as users log in (TLS ends at the relay process), and
  link sign-ins to IP addresses. Passkeys and Google/Apple avoid giving
  the relay a reusable secret.

The relay cannot:

- read envelope contents or forge an envelope from an enrolled device
  (Ed25519 signatures with pinned keys);
- add a device to a host's allowlist (the host decides, on the host);
- make a host run a command: the host connector exposes only the hame
  web API, behind the host's permission engine and sandbox.

### Token theft

- Access tokens live 15 minutes and are only sent in the
  `Authorization` header or the WebSocket subprotocol, never in URLs.
- Refresh tokens rotate on every use, in a write transaction taken at
  once (`BEGIN IMMEDIATE`), so two concurrent uses cannot both succeed.
  If a stolen copy and the real one are both used, the second use is
  detected (`token_reused`): the whole session is revoked, its device's
  WebSocket is closed and the event is audited.
- Tokens are per session and per device. `GET /v1/account/sessions`
  lists the sessions and `DELETE /v1/account/sessions/{id}` revokes one;
  any device can revoke any other (`DELETE /v1/devices/{id}`); logout
  revokes its own.
- A thief with a device's access token can connect as that device:
  receive its queued envelopes (ciphertext only) and acknowledge them
  (dropping them, a denial of service), and send envelopes in its name,
  which hosts reject without the device's signing key. Apps keep tokens
  and keys in the OS keychain.
- A WebSocket lives no longer than the access token it was opened with
  (or renewed with the `auth` frame): at expiry the relay sends
  `unauthorized` and closes it. Revoking the device or its session
  (logout, session revoke, refresh-token reuse, password reset or change)
  kicks it at once; every ping interval the relay also re-checks the
  token in the database, a backstop for a missed kick. A connection
  re-checks its token right after registering with the hub, so a
  revocation racing with the handshake cannot leave it open.
- Rebinding a new session to an existing device needs a fresh Ed25519
  signature by that device's key over the account, device, session and
  time.

### Persistence after an account compromise

An intruder who briefly controlled an account must not keep access
after the owner recovers it:

- A password reset revokes every session, removes **every passkey** and
  deletes all outstanding reset and verification links; the owner is
  emailed. A password change (with the current password) revokes every
  other session and outstanding link, can remove all passkeys
  (`remove_passkeys`), and is notified by email.
- Adding a passkey needs a credential proved in the last 5 minutes (a
  fresh sign-in or `POST /v1/auth/reauth`): a stolen access token alone
  cannot plant a passkey.
- Sessions from the device-code flow have the `host` scope: they may
  register and connect a host (and publish its grants), nothing else: no
  passkeys, no device-code approvals, no revoking devices, no account or
  session changes, no non-host device. `POST /v1/auth/reauth` with the
  password upgrades such a session.
- The owner is emailed when a passkey or a device is added, when Google
  or Apple is linked, and when a sign-in comes from a network (IPv4 /24,
  IPv6 /64) not seen in the account's sign-ins of the last 90 days.
  Notifications have their own per-address budget (5 at once, 20 an
  hour), separate from verification and reset mails.

### Online attacks on sign-in

- Rate limits per IP (all `/v1/auth/*` routes; IPv6 by /64, since one
  subscriber holds a whole /64) and per account or email (login,
  sign-up, reset, re-authentication, passkey login, device-code
  approval). A global limit on failed password logins (300 a minute by
  default) caps credential stuffing across many addresses; beyond it
  password logins are refused for everyone for a short while.
- The limiter keeps at most 100 000 buckets: full buckets (which carry no
  information) are dropped first, then the least recently used.
- Behind the proxy, the client IP is the last `X-Forwarded-For` entry,
  and only when `RELAY_TRUST_PROXY` is set (see DEPLOY.md for when that
  is unsafe).
- No account enumeration: sign-up, forgot-password and resend answer
  the same for every address; forgot-password and resend do their
  database work after the response; login failures are identical and run
  Argon2id against a dummy hash for unknown addresses; sign-up hashes the
  password before looking the address up; passkey login runs the same
  queries for known and unknown addresses and returns a stable fake
  credential id for unknown ones. The one difference left is
  `email_not_verified`, which needs the correct password.
- Password hashing runs at most `RELAY_ARGON2_CONCURRENCY` at once; a
  request that waits more than 5 s for a slot gets `503 busy` instead of
  queueing without bound.
- The per-account limit lets an attacker slow a known user's password
  logins (one-minute windows); passkeys and providers are limited
  separately per IP.

### OAuth and OIDC

- Login CSRF and state injection: random `state`, single use, bound to
  the browser with an HttpOnly cookie (`SameSite=None; Secure` for
  Apple's form POST).
- Code interception and injection: PKCE (S256) with the provider. A
  `return_to` that is not https (a custom scheme such as `hame://`, which
  any app may register) requires the app's own `code_challenge`, so an
  intercepted one-time code is useless without the app's verifier.
- ID tokens: signature against the provider's JWKS (RS256/ES256 only;
  `none` and HMAC refused), `iss`, `aud`, `exp` (60 s leeway), `iat`,
  `nonce` checked. JWKS cached for an hour, refetched on an unknown key
  id at most every 30 s.
- Open redirects: `return_to` must match an allowlist exactly; the
  one-time code is in the URL fragment.
- Account pre-hijacking: linking by email needs `email_verified` from the
  provider. The existing account keeps its password only if that
  password was demonstrably chosen by the verified owner: it signed in
  successfully after verification, or was set through a reset link or a
  password change. Otherwise (unverified address, or a password nobody
  has used since verification, which covers an attacker who registered
  the address and a victim who merely clicked the verification mail) the
  password is removed, every session revoked and every outstanding reset
  and verification link deleted before the identity is linked. Every
  link is emailed to the owner, and the mail says that a password the
  owner never set should be reset; the owner also sees every session in
  the session list. (The registrant could still make the password count
  as proven by signing in with it after the victim verified the address
  and before the victim links a provider; that sign-in, its session and
  the link notice are all visible to the owner.) Accounts never verified
  are deleted after 7 days.

### Device-code phishing

An attacker can start a device code and persuade a user to approve it.
The approval needs a full signed-in session, shows the requesting
client's name (chosen by the requester), expires in 10 minutes and is
audited. The attacker would get a `host`-scoped session in the victim's
account: it can register a host (the owner is emailed) but cannot touch
the account, and no client pairs with that host without scanning its QR
code, so it receives nothing. Apps should present the approval as "a
computer is asking to sign in" with a clear warning.

### Denial of service and disk exhaustion

- Request bodies 64 KiB, envelopes 1 MiB, WebSocket messages 2 MiB,
  frame rate per connection, idle WebSockets closed after 75 s.
- Storage: at most 20 devices per account (not revoked); per device at
  most 10 000 queued envelopes and 64 MiB; per account at most 256 MiB
  queued (running byte counters kept in the same transaction as the
  queue); new envelopes are refused (`unavailable`) while the database's
  file system has less than 1 GiB free. All configurable.
- At most 20 open WebSockets per account.
- Delivery reads a queue in batches of at most 4 MiB, never the whole
  queue at once.
- Argon2id concurrency bounded (memory) with a queue timeout; pushes at
  most two a minute per device; mails at most three at once and six an
  hour per address (further ones are dropped silently; the answer stays
  the same).
- Volumetric attacks need upstream protection.

### Device keys

Registration refuses Ed25519 keys that are not a canonical encoding of a
curve point or that have small order, and X25519 keys of low order
(including all-zero), which would make shared secrets predictable.

### Browser and transport

TLS at the reverse proxy, HSTS; API authentication is by bearer token
(no session cookies, so no CSRF on the API); CORS allows only configured
origins; responses carry `Content-Security-Policy: default-src 'none'`,
`X-Frame-Options: DENY`, `X-Content-Type-Options: nosniff`,
`Referrer-Policy: no-referrer`, `Cache-Control: no-store`. Outgoing calls
to providers use rustls.

### Logs and the development mailer

Structured logs record the route template, status, latency and audit
events with account and device ids. They never contain tokens,
passwords, codes, query strings or envelope contents. The `log` mailer
prints mails with their links; there is no default mailer
(`RELAY_MAILER` is required), and the relay refuses to start with the
`log` mailer and an https public URL unless `RELAY_DEV=1`.

## Checklist

| Control | Where | Tested |
|---|---|---|
| Argon2id m=64 MiB t=3 p=1, bounded queue (503) | `password.rs` | unit tests, `review.rs` |
| Tokens and codes stored hashed | `auth.rs`, `repo/sqlite.rs` | `accounts.rs`: raw values absent from the database file |
| Refresh rotation, reuse revokes the family, `BEGIN IMMEDIATE` | `repo/sqlite.rs` `rotate_refresh_token` | `accounts.rs`, `review.rs` |
| No enumeration (sign-up, login, forgot, passkey) | `routes/account.rs`, `routes/passkey.rs` | `accounts.rs`, `passkeys.rs` |
| Rate limits per IP (/64 for IPv6), per account, global failed logins; bounded map | `ratelimit.rs`, `routes/mod.rs`, `routes/account.rs` | unit tests, `security.rs`, `review.rs` |
| OIDC: signature, iss, aud, exp, nonce, PKCE, state cookie | `oidc.rs`, `routes/oauth.rs` | `oidc.rs` |
| App PKCE required for non-https `return_to` | `routes/oauth.rs` `start` | `review.rs` |
| Pre-hijacking: unproven passwords removed on provider claim, owner notified | `routes/oauth.rs` `resolve_account` | `oidc.rs`, `review.rs` |
| Reset and verification links deleted on password change, reset and provider claim | `routes/account.rs`, `routes/oauth.rs` | `review.rs` |
| Reset removes passkeys and all sessions; change can remove passkeys | `routes/account.rs` | `review.rs` |
| Recent authentication to add a passkey; `reauth` | `auth.rs` `RecentAuthed` | `review.rs` |
| Device-code sessions limited to hosts | `auth.rs` `FullAuthed`, `routes/devices.rs` | `review.rs` |
| Session list and per-session revoke | `routes/account.rs` | `review.rs` |
| Notifications: passkey, device, provider linked, new network | `state.rs` `notify`, `routes/*` | `review.rs` |
| Passkeys: single-use challenges, owner check | `routes/passkey.rs` | `passkeys.rs` |
| Device code: approval needs a full session, slow_down, expiry | `routes/device_code.rs` | `device_code.rs`, `review.rs` |
| Same-account routing only, indistinguishable refusal | `ws.rs` `on_send` | `ws.rs` |
| Envelope limit 1 MiB | `ws.rs` | `ws.rs` |
| Device cap, byte quotas per device and account, disk floor, WS cap, batched delivery | `repo/sqlite.rs`, `disk.rs`, `ws.rs` | `review.rs`, unit test |
| WebSocket closes at token expiry, on revocation and by periodic check; `auth` renewal | `ws.rs` | `accounts.rs`, `review.rs` |
| Rebind needs a device-key signature | `routes/devices.rs` | `accounts.rs` |
| Host grants signed by the host key, versioned, no rollback | `routes/devices.rs` | `accounts.rs` |
| Device keys validated (Ed25519 point, X25519 order) | `routes/devices.rs` | `review.rs` |
| Security headers, CORS, body limit | `routes/mod.rs` | `security.rs` |
| Audit log | `state.rs` `audit` | `accounts.rs`, `security.rs` |
| Mail flood limit per address | `state.rs` `send_email` | `accounts.rs` |
| No default mailer; `log` refused on https without `RELAY_DEV` | `config.rs` | unit test |

## Open questions

- Audit-log and IP retention period (privacy law); currently kept.
- Account deletion and email change endpoints are not implemented yet.
- Rate limits and presence live in memory: one relay process; a restart
  resets the buckets.
- Should device names be optional or encrypted?
- A CAPTCHA or proof-of-work on sign-up if abuse appears.
- Key transparency for host keys (a log clients can audit) would make
  key substitution at first pairing detectable even without the SAS.
- Free-space checking uses `statvfs` and is skipped on non-Unix systems.
