# hame-relay API

HTTPS JSON under `/v1`, WebSocket at `/v1/ws`. This is the contract the
apps and the host connector (`hame-sync`) code against; it implements
section 3 of `docs/design/APPS-AND-SYNC.md`.

Conventions:

- Request and response bodies are JSON (`Content-Type: application/json`),
  at most 64 KiB by default.
- Authenticated routes take `Authorization: Bearer <access token>`.
- Errors: an HTTP status and `{"error": "<code>", "message": "<text>"}`.
  `429` responses carry `Retry-After`.
- Times are Unix seconds. Binary values (keys, envelopes, signatures) are
  base64url; padding is optional on input and never sent.

## Tokens

Every sign-in answers with:

```json
{
  "access_token": "hra_…",
  "refresh_token": "hrr_…",
  "token_type": "Bearer",
  "expires_in": 900,
  "account_id": "…",
  "session_id": "…",
  "device_id": null
}
```

Tokens are opaque (256 random bits). A sign-in starts a **session** (a
token family). The access token lives 15 minutes, the refresh token 30
days. `POST /v1/auth/refresh {refresh_token}` returns a new pair for the
same session and invalidates the presented refresh token. Presenting a
used refresh token again (`401 token_reused`) revokes the whole session:
a stolen and a legitimate copy cannot both continue. Unknown or expired:
`401 invalid_token`.

A session is bound to at most one device (`POST /v1/devices` or
`/rebind`). Revoking the device revokes its sessions.

## Accounts

| Route | Body | Answer |
|---|---|---|
| `POST /v1/auth/signup` | `{name, email, password}` | `202 {"status":"accepted"}` always (also for registered addresses: the owner gets a "you already have an account" mail with a reset link). `400 weak_password / invalid_email / invalid_name`. |
| `POST /v1/auth/verify` | `{token}` | `200 {"status":"verified"}`; `400 invalid_token` |
| `POST /v1/auth/verify/resend` | `{email}` | `202` always |
| `POST /v1/auth/login` | `{email, password}` | tokens; `401 invalid_credentials` (same for unknown address); `403 email_not_verified` (right password, unverified) |
| `POST /v1/auth/refresh` | `{refresh_token}` | tokens; `401 token_reused / invalid_token` |
| `POST /v1/auth/logout` | – (auth) | `204`, revokes this session |
| `POST /v1/auth/password/forgot` | `{email}` | `202` always |
| `POST /v1/auth/password/reset` | `{token, password}` | `200`; revokes every session and marks the address verified; `400 invalid_token / weak_password` (the link stays usable after `weak_password`) |
| `GET /v1/account` | (auth) | `{id, name, email, email_verified, has_password, created_at, identities[], passkeys[], session_id, device_id}` |
| `PATCH /v1/account` | `{name}` (auth) | `204` |
| `POST /v1/account/password` | `{current_password, new_password}` (auth) | `204`, revokes the other sessions |
| `GET /v1/account/audit` | (auth) | `{events: [{at, event, account_id, device_id, ip, detail}]}`, newest first, 100 |
| `GET /v1/account/passkeys` | (auth) | `{passkeys: [{id, name, created_at, last_used_at}]}` |
| `DELETE /v1/account/passkeys/{id}` | (auth) | `204` |

Email links point at the web app: `{RELAY_APP_URL}/verify?token=…` and
`{RELAY_APP_URL}/reset-password?token=…`; the app posts the token.

Password rules: 10 to 128 characters, at least 5 different characters,
not a common password, not containing the email's local part or the name.

## Google and Apple

1. Open `GET /v1/auth/oauth/{google|apple}/start` in a browser (for
   native apps: the system authentication session). Optional query:
   `return_to` (must be listed in `RELAY_OAUTH_RETURN_URLS`) and
   `code_challenge` (the app's own S256 PKCE challenge, recommended for
   native apps). The relay answers `303` to the provider and sets the
   `hame_oauth` cookie that binds the flow to this browser.
2. The provider returns to `/v1/auth/oauth/{provider}/callback` (Google:
   GET; Apple: form POST). The relay exchanges the code with its PKCE
   verifier and verifies the ID token (signature against the provider's
   JWKS, `iss`, `aud`, `exp`, `nonce`).
3. Without `return_to`: `200` tokens. With it: `303` to
   `{return_to}#code=<one-time code>` (valid 2 minutes); the app calls
   `POST /v1/auth/oauth/exchange {code, code_verifier?}` → tokens.

Errors: `400 invalid_state` (unknown, used, expired or not this browser),
`401 invalid_id_token`, `403 email_not_verified` (the provider did not
vouch for the address), `502 provider_error`, `404 not_configured`.

Accounts are linked by verified email. When the matching account never
verified its address, its password is removed and its sessions revoked
before linking (protection against pre-registered accounts).

## Passkeys

| Route | Body | Answer |
|---|---|---|
| `POST /v1/auth/passkey/register/start` | `{}` (auth) | `{challenge_id, options}`: `options` is the `PublicKeyCredentialCreationOptions` wrapper for `navigator.credentials.create` |
| `POST /v1/auth/passkey/register/finish` | `{challenge_id, credential, name?}` (auth) | `{id, name}` |
| `POST /v1/auth/passkey/login/start` | `{email?}` | `{challenge_id, options}` for `navigator.credentials.get`. With `email`: `allowCredentials` lists the account's passkeys (a stable fake one for unknown addresses). Without: a discoverable-credential challenge (`mediation: "conditional"`). |
| `POST /v1/auth/passkey/login/finish` | `{challenge_id, credential}` | tokens; `401 invalid_credentials`; `400 invalid_challenge` |

Challenges are single use and expire after 5 minutes. User verification
is required.

## Device code (hosts, CLIs)

| Route | Body | Answer |
|---|---|---|
| `POST /v1/auth/device/start` | `{client_name?}` | `{device_code, user_code: "BCDF-GHJK", verification_url, verification_url_complete, expires_in: 600, interval: 5}` |
| `POST /v1/auth/device/lookup` | `{user_code}` (auth) | `{client_name, created_at, expires_in}` to show before approving |
| `POST /v1/auth/device/approve` | `{user_code, approve?: true}` (auth) | `204` (`approve: false` denies) |
| `POST /v1/auth/device/token` | `{device_code}` | tokens, or `400` with `error`: `authorization_pending`, `slow_down` (with the new `interval`, +5 s each time), `access_denied`, `expired_token`, `invalid_grant` |

User codes ignore case and separators.

## Devices and hosts

| Route | Body | Answer |
|---|---|---|
| `POST /v1/devices` | `{name, kind, ed25519, x25519}` (auth) | `201` device; binds this session to it; `409 session_has_device`. `kind`: `web`, `desktop`, `ios`, `android`, `host`. Keys: 32 bytes each. |
| `GET /v1/devices` | (auth) | `{devices: [device]}` |
| `DELETE /v1/devices/{id}` | (auth) | `204`; revokes its sessions, closes its WebSocket, drops its queue |
| `POST /v1/devices/{id}/rebind` | `{timestamp, signature}` (auth) | `204`: attaches this (new) session to an existing device, e.g. after a password reset. `signature` = Ed25519 by the device key over `hame-relay-rebind:v1\n{account_id}\n{device_id}\n{session_id}\n{timestamp}`, timestamp within 5 minutes |
| `POST /v1/devices/{id}/push` | `{provider: "apns"|"fcm"|null, token}` (auth, the device itself) | `204` |
| `GET /v1/hosts` | (auth) | `{hosts: [device]}` (kind `host`) |
| `POST /v1/hosts/{id}/grants` | `{document, signature}` (auth, the host itself) | `204`; `signature` = Ed25519 by the host key over the document bytes; the document is opaque to the relay |
| `GET /v1/hosts/{id}/grants` | (auth) | `{host_id, document, signature, updated_at}` |

A device:

```json
{
  "id": "…", "name": "…", "kind": "host",
  "ed25519": "<b64url>", "x25519": "<b64url>",
  "fingerprint": "<hex SHA-256 of ed25519 ‖ x25519 public keys>",
  "online": true, "created_at": 0, "last_seen_at": 0, "push": "apns"
}
```

## WebSocket

`GET /v1/ws?device=<id>` with the device's own access token, either as
`Authorization: Bearer …` or, from browsers, as the subprotocol list
`Sec-WebSocket-Protocol: hame-relay.v1, bearer.<token>` (the relay selects
`hame-relay.v1`). Text frames, JSON:

```
client → relay  {"op":"send","to":"<device id>","envelope":"<b64url>","push":false}
relay → client  {"op":"sent","to":"<device id>","seq":41}
relay → client  {"op":"deliver","seq":42,"from":"<device id>","envelope":"<b64url>"}
client → relay  {"op":"ack","seq":42}            relay drops ≤ 42
client → relay  {"op":"resume","after":40}       first frame after connect
relay → client  {"op":"presence","device":"<id>","online":true}
relay → client  {"op":"error","code":"…","message":"…","to":"<device id>"?}
both            {"op":"ping"} / {"op":"pong"}
```

- `seq` is per recipient, monotonic, starting at 1.
- `resume {after}` acknowledges everything up to `after` and delivers the
  rest in order. If the first frame is something else, the relay resumes
  from the last acknowledged `seq`. Unacknowledged envelopes are
  delivered again on the next connection; queues are kept 24 hours.
- After `resume`, the relay sends `presence` for each online device of
  the account; later changes arrive as they happen.
- Error codes: `not_found` (unknown device, or a device of another
  account: the two are indistinguishable), `too_large` (envelope over
  1 MiB decoded), `rate_limited` (frames per second, or the recipient's
  queue is full), `forbidden` (sending to oneself; also sent just before
  the relay closes a revoked or replaced connection), `bad_request`
  (malformed frame or envelope), `internal`.
- The relay pings every 25 s and closes a connection silent for 75 s.
- `push: true` on a send to an offline `ios`/`android` device with a push
  token triggers one opaque notification (`{r: recipient, f: sender, s: seq}`,
  at most two a minute per device).
- A newer connection for the same device replaces the older one.

## Other

- `GET /healthz` → `{"status":"ok"}` (`503` when the database is
  unreachable).
