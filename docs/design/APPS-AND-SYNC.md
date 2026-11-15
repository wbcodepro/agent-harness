# hame apps and sync: architecture and plan

Status: plan, 2026-11-16. Owner decisions (from the product owner):

- A new web UI, a desktop app and native iOS/Android apps, all one product.
- A user logs in on any of them and can message the agent from any of
  them; **the agent always works on a computer where hame is running**
  (the "host"), never on the server.
- A relay server runs on the owner's VPS (commercial service). Built and
  tested locally first; deploying it needs the owner's go-ahead.
- Sign-up with name, email and password; sign-in with Google and with
  Apple; passkeys; new devices paired by QR code.

## 1. Components

```
 phone app ─┐                      ┌─ host: desktop app or `hame connect`
 web app  ──┼── TLS ── relay ── TLS┤   (runs the agent, the tools, the
 desktop  ──┘   (accounts, routing, │    sandbox, on the user's computer)
                 push; sees only    └─ more hosts per account
                 ciphertext)
```

| Component | Where | What |
|---|---|---|
| `apps/ui` | TypeScript, React, Vite | One UI for web, desktop and mobile. Two transports behind one interface: **local** (the hame web server on this machine) and **remote** (a host through the relay, end-to-end encrypted). |
| `crates/hame-web` | Rust | Serves the built UI and the local JSON/SSE API (extended for the new UI). |
| `apps/desktop` | Tauri v2 (Rust) | Desktop app: starts the hame web server **in process**, shows the UI, tray, notifications, deep links (`hame://`), start at login; is a host for the relay when signed in. Same project builds the **iOS and Android** apps (remote mode only). |
| `crates/hame-sync` | Rust | Protocol types and end-to-end crypto shared by relay, host and tests; `hame connect` (host connector) and the pairing flow. |
| `crates/hame-relay` | Rust, separate binary | Accounts and auth, devices and hosts, message routing over WebSockets, push notifications. Stores no prompt, answer or file in readable form. |

Why Tauri: Rust like the rest of hame, small binaries (system web
view instead of a bundled Chromium), one project for desktop and mobile.

## 2. Transport: the same API locally and remotely

The UI speaks one API, the hame web API (`/api/...`, JSON requests, an
event stream per session). Locally it goes over HTTP + SSE to
`127.0.0.1`. Remotely, each request and each stream event is wrapped in an
encrypted envelope and carried over the relay's WebSocket; the host
connector unwraps it and calls the **same axum router in process**
(`tower::Service::oneshot`), so a remote session is exactly a local one.

Remote frames (inside the encrypted envelope, JSON):

```json
{"t":"req","id":7,"method":"POST","path":"/api/sessions/abc/prompt","body":{...}}
{"t":"res","id":7,"status":200,"body":{...}}
{"t":"sub","id":8,"path":"/api/sessions/abc/events","after":120}
{"t":"evt","id":8,"seq":121,"data":{...record...}}
{"t":"end","id":8}
{"t":"cancel","id":8}
{"t":"blob","id":9,"sha256":"...","chunk":0,"of":3,"data":"base64"}
```

Artifacts (screenshots, files) travel as encrypted `blob` chunks
(256 KiB). Reconnects resume event streams with `after` (session records
are sequence-numbered already).

## 3. Relay API

HTTPS JSON under `/v1`, WebSocket at `/v1/ws`.

Accounts and sessions:
- `POST /v1/auth/signup {name,email,password}` → sends a verification
  email (SMTP); `POST /v1/auth/verify {token}`.
- `POST /v1/auth/login {email,password}` → tokens.
- `POST /v1/auth/password/forgot`, `/v1/auth/password/reset`.
- `GET /v1/auth/oauth/{google|apple}/start` → redirect;
  `/v1/auth/oauth/{provider}/callback` → tokens (OIDC: ID-token signature
  checked against the provider's JWKS, `aud`, `iss`, `nonce`, `exp`).
- Passkeys (WebAuthn): `/v1/auth/passkey/register/{start,finish}`,
  `/v1/auth/passkey/login/{start,finish}`.
- Tokens: opaque, random 256-bit, stored **hashed** (SHA-256) server-side;
  access token 15 min, refresh token 30 days, rotated on use, revocable;
  per-device.
- Device-code login for hosts and CLIs:
  `POST /v1/auth/device/start` → `{user_code, verification_url, device_code}`;
  the user approves in a signed-in app; the host polls
  `POST /v1/auth/device/token`.

Devices and hosts:
- `POST /v1/devices {name, kind: web|desktop|ios|android|host, public_key}`
  registers this device's X25519 public key.
- `GET /v1/hosts` lists the account's hosts with online status and their
  public keys and fingerprints.
- `POST /v1/hosts/{id}/grants` (sent by the host) lists which client
  devices the host accepts, signed by the host (see §4).
- Push: `POST /v1/devices/{id}/push {apns|fcm token}`.

WebSocket `/v1/ws?device=<id>` (bearer token of that device). Text
frames, JSON:

```
client → relay  {"op":"send","to":"<device id>","envelope":"<b64url>"}
relay → client  {"op":"sent","to":"<device id>","seq":41}
relay → client  {"op":"deliver","seq":42,"from":"<device id>","envelope":"<b64url>"}
client → relay  {"op":"ack","seq":42}             (relay may drop ≤ 42)
client → relay  {"op":"resume","after":40}        (first frame after connect)
relay → client  {"op":"presence","device":"<id>","online":true}
relay → client  {"op":"error","code":"not_found|too_large|rate_limited|forbidden","message":"..."}
both            {"op":"ping"} / {"op":"pong"}     (every 25 s)
```

Routing rules: only between devices of the same account; envelopes up to
1 MiB (blobs are chunked); per-recipient queue kept 24 h or until acked;
`seq` is per recipient and monotonic. The relay does not decide who may
control a host: it delivers to the host any envelope from a device of the
same account, and the host enforces its allowlist (§4). Push: when an
envelope addressed to an offline mobile device is queued and the sender
flagged it `"push": true` in the send frame, the relay sends an opaque
push.

Security baseline: Argon2id (m=64 MiB, t=3, p=1) for passwords; rate
limits per IP and per account on every auth route; email enumeration
avoided in responses; CSRF protection for cookie flows; security headers;
audit log of sign-ins, devices added and grants changed; SQLite (WAL) with
a repository layer that a PostgreSQL backend can replace; migrations;
configuration from environment; `/healthz`. Docker image and a deploy
guide for a VPS (TLS by a reverse proxy). No deployment without the
owner.

## 4. End-to-end encryption and pairing

Goal: **the relay can route but cannot read, and a stolen relay database,
a relay administrator or a malicious relay cannot make a host run
anything.** Research on existing products (Claude Code Remote Control and
Codex remote: TLS only, transcripts on the vendor's servers; Happy Coder:
E2E but metadata leaks, raw `bash`/`readFile` RPCs routed by the relay,
keys on disk with mode 644) sets the bar: the relay is an untrusted
mailbox; **the host's own list of enrolled devices is the only
authority.**

Keys (generated on the device, never sent):
- Each device and each host: an Ed25519 signing key and an X25519 key.
  Desktop/mobile keep them in the OS keychain; a host's files are mode
  0600; the browser keeps non-extractable WebCrypto keys in IndexedDB.
- The relay stores public keys only.

Envelope (every request, response, event and blob chunk):
- header: `{v:1, from, to, stream, counter}` (device ids, a stream id per
  session subscription or request, a per-sender persisted sequence);
- body: ephemeral X25519 to the recipient → HKDF-SHA256 → XChaCha20-
  Poly1305, header as associated data;
- signature: the sender's Ed25519 over header ‖ ciphertext.
- The host accepts an envelope only if `from` is on its local allowlist,
  the signature verifies with the pinned key, and the counter has not
  been seen from that device (a 64-counter replay window above the
  highest seen; see crates/hame-sync/README.md). Metadata such as session
  titles, paths and host names is inside the ciphertext, never in the
  header.
- Libraries: Rust `ed25519-dalek`, `x25519-dalek`, `chacha20poly1305`,
  `hkdf`, `sha2`; TypeScript `@noble/curves`, `@noble/ciphers`,
  `@noble/hashes` (audited, pure TS). Shared test vectors are checked in
  both languages.

Pairing (the host decides who may control it):
1. The host (desktop app or `hame connect`) shows a QR code:
   `hame-pair:v1?relay=…&host=<id>&ed=<pk>&x=<pk>&secret=<32 bytes b64url>&exp=<unix>`
   (one use, 10 minutes).
2. The client scans it and sends, through the relay, its public keys
   plus `HMAC-SHA256(secret, client_ed ‖ client_x)`, encrypted to the
   host.
3. The host answers a valid request with a fresh random nonce
   (`pair_challenge`), chosen after the client committed to its keys.
   Both screens show a short authentication string: 8 digits from
   `SHA256("hame-sas-v2" ‖ secret ‖ host_ed ‖ client_ed ‖ host_nonce)`.
   The user confirms **on the host**; only then is the device added to
   the host's allowlist (with a label), and the host returns a signed
   grant with the highest counter it accepted from that key (a device
   that lost its counter and pairs again continues above it; the host
   never lowers a known key's replay window).
4. Later devices: approved the same way from the host, or from an
   enrolled device that relays the request to the host, which still asks
   for confirmation on the host or shows the request to the enrolled
   device with the SAS.
5. Revocation from any enrolled device or the host; a revoked key is
   refused at once. A changed key for a known device is refused with a
   warning, never silently accepted.

Approvals: an approval request from the host carries the tool call id and
`sha256` of its exact arguments; the approving envelope must name the same
pair. Tool-call approvals do not expire; other prompts expire to the safe
choice.

Relay mailbox: per recipient device, an ordered queue of envelopes with a
monotonic `seq`, acknowledged by the recipient, kept at most 24 h; a
client that reconnects resumes from its last acknowledged `seq`. Push
notifications (APNs/FCM) carry only "a session on <host label> needs you"
with opaque ids; the app fetches and decrypts.

The host connector exposes **only** the hame web API (sessions, prompts,
cancel, approvals, artifacts). There is no shell, file or generic RPC
route: every remote action goes through the agent and its permission
engine and sandbox, as a local one does.

What the relay sees: account ids, device and host ids, online status,
timing and sizes. Not: prompts, answers, file names, paths, tool calls,
screenshots, session titles.

## 5. UI (filled in from the UI research)

See `docs/design/UI.md` (written after studying DeepSeek Harness, Claude
and Codex UIs): layout, components, design tokens, behaviours. No
product's name, logo, colours or assets are copied.

## 6. Work packages

| WP | Owner | Depends on | Delivers | Accepted when |
|---|---|---|---|---|
| A | agent | UI.md | `apps/ui` + hame-web API extensions; local transport; built assets embedded in `hame web` | full chat flow against a real `hame web` (scripted model in tests); Playwright UI tests; responsive at 390 px; `cargo test`, `npm test`, lint clean |
| B | agent | this doc | `crates/hame-relay`: accounts, email+password, Google/Apple OIDC, passkeys, device-code, devices/hosts/grants, WS routing, push stubs, Docker | integration tests for every auth flow (OIDC against a local mock IdP; WebAuthn with a software authenticator), rate limits, token rotation; threat checklist |
| C | agent | this doc, B's API | `crates/hame-sync`: envelope crypto, `hame connect` host connector tunnelling the hame-web router, QR pairing, approvals; TS client transport in `apps/ui/src/sync` | end-to-end test: relay + host + scripted client run a turn remotely; tampered/replayed/unknown-key envelopes rejected; Rust↔TS crypto test vectors |
| D | agent | A | `apps/desktop` Tauri v2: in-process hame web server, tray, notifications, deep links, sign-in, host mode | builds and runs on macOS; Windows build checked by the Windows machine |
| E | agent | A, C, D | iOS and Android apps from the Tauri project (remote mode, QR pairing, push) | iOS simulator and Android emulator run the UI and complete a remote turn against a local relay |
| R | reviewers | each WP | independent review per WP (security for B and C) | findings fixed or documented |

Order: A, B and C in parallel (C codes against B's API as specified
here); D after A has a working UI; E after D and C. Every package works in
its own git worktree and branch; each is reviewed before it is merged.
