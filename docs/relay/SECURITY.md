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
- Refresh tokens rotate on every use. If a stolen copy and the real one
  are both used, the second use is detected (`token_reused`): the whole
  session is revoked, its device's WebSocket is closed and the event is
  audited.
- Tokens are per session and per device; any device can revoke any
  other (`DELETE /v1/devices/{id}`), a password reset or change revokes
  all (other) sessions, logout revokes its own.
- A thief with a device's access token can connect as that device:
  receive its queued envelopes (ciphertext only) and acknowledge them
  (dropping them, a denial of service), and send envelopes in its name,
  which hosts reject without the device's signing key. Apps keep tokens
  and keys in the OS keychain.
- A WebSocket is authenticated when it opens and stays open after the
  access token expires; revoking the device or its session (logout,
  refresh-token reuse, password reset or change) closes it at once.
- Rebinding a new session to an existing device needs a fresh Ed25519
  signature by that device's key over the account, device, session and
  time.

### Online attacks on sign-in

- Rate limits per IP (all `/v1/auth/*` routes) and per account or email
  (login, sign-up, reset, passkey login, device-code approval).
  Behind the proxy, the client IP is the last `X-Forwarded-For` entry,
  and only when `RELAY_TRUST_PROXY` is set.
- No account enumeration: sign-up, forgot-password and resend answer
  the same for every address; login failures are identical and run
  Argon2id against a dummy hash for unknown addresses; sign-up hashes the
  password before looking the address up; email is sent in the
  background; passkey login for an unknown address returns a stable fake
  credential id. The one difference left is `email_not_verified`, which
  needs the correct password.
- The per-account limit lets an attacker slow a known user's password
  logins (one-minute windows); passkeys and providers are limited
  separately per IP.

### OAuth and OIDC

- Login CSRF and state injection: random `state`, single use, bound to
  the browser with an HttpOnly cookie (`SameSite=None; Secure` for
  Apple's form POST).
- Code interception and injection: PKCE (S256) with the provider; for
  native apps an optional second PKCE on the one-time-code exchange.
- ID tokens: signature against the provider's JWKS (RS256/ES256 only;
  `none` and HMAC refused), `iss`, `aud`, `exp` (60 s leeway), `iat`,
  `nonce` checked. JWKS cached for an hour, refetched on an unknown key
  id at most every 30 s.
- Open redirects: `return_to` must match an allowlist exactly; the
  one-time code is in the URL fragment.
- Account pre-hijacking: linking by email needs `email_verified` from the
  provider; if the existing account never verified its address, its
  password is removed and its sessions revoked before linking. Accounts
  never verified are deleted after 7 days. Clicking a verification link
  for a sign-up you did not make would still verify an attacker-chosen
  password; the mail says to ignore it, and a later sign-up attempt for
  the address sends a reset link, not a verification link.

### Device-code phishing

An attacker can start a device code and persuade a user to approve it.
The approval needs a signed-in session, shows the requesting client's
name (chosen by the requester), expires in 10 minutes and is audited.
The attacker would get a host in the victim's account, but no client
pairs with that host without scanning its QR code, so it receives
nothing. Apps should present the approval as "a computer is asking to
sign in" with a clear warning.

### Denial of service

Request bodies 64 KiB, envelopes 1 MiB, WebSocket messages 2 MiB, frame
rate per connection, 10 000 queued envelopes per device, Argon2id
concurrency bounded (memory), pushes at most two a minute per device,
mails at most three at once and six an hour per address (further ones
are dropped silently; the answer stays the same),
idle WebSockets closed after 75 s. Volumetric attacks need upstream
protection.

### Browser and transport

TLS at the reverse proxy, HSTS; API authentication is by bearer token
(no session cookies, so no CSRF on the API); CORS allows only configured
origins; responses carry `Content-Security-Policy: default-src 'none'`,
`X-Frame-Options: DENY`, `X-Content-Type-Options: nosniff`,
`Referrer-Policy: no-referrer`, `Cache-Control: no-store`. Outgoing calls
to providers use rustls.

### Logs

Structured logs record the route template, status, latency and audit
events with account and device ids. They never contain tokens,
passwords, codes, query strings or envelope contents. The development
`log` mailer is the exception (it prints mails with their links) and is
for local use only.

## Checklist

| Control | Where | Tested |
|---|---|---|
| Argon2id m=64 MiB t=3 p=1 | `password.rs` | unit test |
| Tokens and codes stored hashed | `auth.rs`, `repo/sqlite.rs` | `accounts.rs`: raw values absent from the database file |
| Refresh rotation, reuse revokes the family | `repo/sqlite.rs` `rotate_refresh_token` | `accounts.rs` |
| No enumeration (sign-up, login, forgot, passkey) | `routes/account.rs`, `routes/passkey.rs` | `accounts.rs`, `passkeys.rs` |
| Rate limits per IP and per account | `routes/mod.rs`, `state.rs` | `security.rs` |
| OIDC: signature, iss, aud, exp, nonce, PKCE, state cookie | `oidc.rs`, `routes/oauth.rs` | `oidc.rs` |
| Pre-hijacking protection | `routes/oauth.rs` `resolve_account` | `oidc.rs` |
| Passkeys: single-use challenges, owner check | `routes/passkey.rs` | `passkeys.rs` |
| Device code: approval needs a session, slow_down, expiry | `routes/device_code.rs` | `device_code.rs` |
| Same-account routing only, indistinguishable refusal | `ws.rs` `on_send` | `ws.rs` |
| Envelope limit 1 MiB | `ws.rs` | `ws.rs` |
| Revocation closes WebSockets | `ws.rs` `Hub::kick` | `accounts.rs` |
| Rebind needs a device-key signature | `routes/devices.rs` | `accounts.rs` |
| Host grants signed by the host key | `routes/devices.rs` | `accounts.rs` |
| Security headers, CORS, body limit | `routes/mod.rs` | `security.rs` |
| Audit log | `state.rs` `audit` | `accounts.rs`, `security.rs` |
| Mail flood limit per address | `state.rs` `send_email` | `accounts.rs` |

## Open questions

- Audit-log and IP retention period (privacy law); currently kept.
- Account deletion and email change endpoints are not implemented yet.
- Rate limits and presence live in memory: one relay process; a restart
  resets the buckets.
- Should device names be optional or encrypted?
- A CAPTCHA or proof-of-work on sign-up if abuse appears.
- Key transparency for host keys (a log clients can audit) would make
  key substitution at first pairing detectable even without the SAS.
