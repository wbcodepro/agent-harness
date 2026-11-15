# hame-sync

End-to-end encrypted sync between a hame host and the hame apps through an
untrusted relay (`docs/design/APPS-AND-SYNC.md`, §2 to §4). The relay
routes envelopes; it cannot read them, and nothing it sends makes the host
run anything unless it comes from a device on the host's own allowlist.

| Module | What |
|---|---|
| `keys` | Ed25519 + X25519 identities, fingerprints |
| `envelope` | the sealed, signed envelope |
| `pairing` | QR offer, HMAC proof, SAS, signed grants and grant lists |
| `store` | host identity, allowlist and relay registration under `<home>/sync/`, mode 0600 |
| `frames` | relay WebSocket frames (§3) and remote API frames (§2) |
| `relay` | a relay WebSocket connection: resume, ack, ping, jittered reconnection |
| `relay_api` | the relay HTTP routes a host uses (device code, refresh, registration, grants) |
| `approval` | binding of remote approval answers to the tool call and its arguments |
| `connector` | the host connector: runs remote frames against the hame web router in process |
| `client` | a device side in Rust (tests, tools) |
| `mock_relay` (feature `mock-relay`) | an in-process relay following the real relay's contract |

`hame connect` (crates/hame-cli) is the command that runs a host. The
TypeScript device side is `packages/sync-js`.

## Constructions

All byte strings travel as base64url without padding, except blob chunk
data (standard base64 with padding).

**Fingerprint**: lowercase hex of `SHA256(ed25519_public ‖ x25519_public)`,
the relay's `fingerprint` field; shown to people in groups of four digits.

**Envelope** (every request, response, event, blob chunk and pairing
message):

```
0x01 | u16 BE header length (≤ 1024) | header | 32 ephemeral X25519 public
     | 24 nonce | ciphertext + 16-byte tag | 64-byte Ed25519 signature
```

- header: UTF-8 JSON `{"v":1,"from","to","stream","counter"}`, carried as
  the exact bytes that were authenticated (never re-serialised); ids are 1
  to 128 of `[A-Za-z0-9_-]`; `counter` at most 2^53 − 1.
- `key = HKDF-SHA256(ikm = X25519(ephemeral, recipient_x), salt =
  ephemeral_public ‖ recipient_x_public, info = "hame-sync v1 envelope key")`,
  32 bytes. A fresh ephemeral key per envelope; an all-zero shared secret
  (low-order point) is refused.
- `ciphertext = XChaCha20-Poly1305(key, nonce, plaintext, aad = header bytes)`;
  the 24-byte nonce is random (the key is single-use anyway).
- `signature = Ed25519(sender, "hame-sync v1 signature" ‖ every byte before
  the signature)`, i.e. header ‖ ciphertext with their framing; verified
  strictly (`verify_strict`; noble with `zip215: false`).
- Limits: plaintext ≤ 512 KiB, envelope text ≤ 1 MiB (checked before
  decoding).
- A recipient checks, in order: `to` is itself; `from` is on its allowlist
  (the relay's `from` must match the signed one); the signature verifies
  with the pinned key; the counter is new to the sender's replay window;
  the ciphertext decrypts; the frame's id matches the header's `stream`.
  The counter is recorded (on the host: persisted under the store's lock)
  only after all of this.
- Counters are sequences (1, 2, 3…), persisted by the sender: the host
  reserves blocks of 256 on disk before use, so even after a crash it never
  repeats one; devices keep theirs next to their identity (sync-js
  `PersistentSequence`, incremented under a Web Lock so tabs sharing an
  identity do not collide). The receiver keeps a 64-counter replay window
  (highest seen plus a bitmap), so envelopes reordered between tabs or
  resent after a reconnection are accepted once. A device that lost its
  sequence pairs again: the host keeps the key's window (lowering it would
  let a relay replay everything the device sent before) and returns the
  highest counter it accepted in `paired` (`last_counter`); the device
  continues above it.

**Pairing**:

- QR: `hame-pair:v1?relay=…&host=<device id>&ed=…&x=…&secret=<32 bytes>&exp=<unix>`
  (form-encoded query; one use; ten minutes).
- The device sends `{"t":"pair","ed","x","label","mac"}` sealed to the
  host's X25519 key and signed with its own new Ed25519 key, `mac =
  HMAC-SHA256(secret, client_ed ‖ client_x)`. The host verifies the
  signature with the presented key and the MAC in constant time, spends
  the offer, and refuses revoked keys and changed keys for known devices
  (with a warning).
- After a valid request the host answers `pair_challenge {nonce}` (32
  random bytes, chosen after the device committed to its keys).
- SAS: the first eight bytes of `SHA256("hame-sas-v2" ‖ secret ‖ host_ed ‖
  client_ed ‖ host_nonce)` as a big-endian number, modulo 10^8, eight digits
  (shown as `1234 5678`). Nobody can search for keys that give chosen
  digits, since the nonce is unknown until the keys are fixed.
- `hame connect` shows the offer as a QR code only; `--show-uri` also prints
  the text (it holds the secret).
- The host user has two minutes to answer; no answer, or a yes after the
  code expired, is a refusal. A second device answering a code that was
  already used gets `pair_refused`, and the host user is told.
- After the user confirms on the host: a grant `{v, host, device, ed, x,
  label, issued}`, signed as `Ed25519(host, "hame-sync v1 grant" ‖ JSON
  bytes)`, travels back in a `paired` frame.
- Published grant list (`POST /v1/hosts/{id}/grants {version, document,
  signature}`): `document` is the JSON
  `{"kind":"hame-sync/grants","v":1,"host","version","devices":[…],"issued"}`
  and `signature` is Ed25519 over `"hame-grants:v1\n{host_id}\n{version}\n"
  ‖ document`, the relay's format, which it verifies with the host's
  registered key; `version` increases with every list (persisted in the
  allowlist; on `409 stale_version` the host publishes again above the
  relay's). Labels are left out unless `hame connect --publish-labels`.

**Approvals**: hame web's approval event carries `tool_call_id`, `tool`,
the exact `arguments` and `args_sha256`, the sha256 of the arguments'
canonical JSON (RFC 8785). The canonical form has one implementation,
`hame_web::jcs` (re-exported as `hame_sync::jcs`; sync-js
`canonicalJson`/`argsSha256`; both vector suites run against it). The
device shows the arguments and recomputes the hash (`approvalAnswer`).
hame web's approval route requires `{id, allow, tool_call_id,
args_sha256}` naming the question and refuses anything else, including a
session-wide `always` from a remote device; the connector relays the
event and the answer unchanged.

**Remote policy**: hame web applies its `[remote]` policy to every request
carrying `RemoteCaller` (session folders within `[remote] roots`, and with
roots set no sight of sessions outside them, which answer 404; no "Full
access" or "Accept edits" unless `[remote] allow_full_access = true`; no
session-wide grants; bound approval answers; browser and desktop actions
that act are asked about in remote-driven turns, looking still runs). On top of it,
`hame connect` builds every session of its router with
`hame_app::Options::ask_before_changes`, so each call that is not
read-only asks the user, whatever the configuration or the session's mode
says when the session is built (`yes`, the reviewer model, `[approval]
default` and allow rules are ignored; ask and deny rules, and a mode that
denies, apply). The sandbox mode is pinned at startup and full access is
refused unless `[remote] allow_full_access = true`. The connector limits
each device to 32 running requests, 8 subscriptions, 2 artifact downloads
(read from hame web's streamed artifact route chunk by chunk, never
buffered whole) and 10 new sessions a minute; loaded sessions are released
by hame web's idle eviction (`[web] idle_seconds`). Because a host under
`hame connect` asks whatever the mode says, hame web reports it: `info`
has `"enforced": "ask"`, each session `"enforced": "ask"` (or
`"ask-without-sandbox"` for Full access, which there still asks), the
interface shows "Asks first" instead of the mode, and a remote device's
new session is recorded as Ask.

The event loop never waits on the relay: pairing answers, refusals and
the "changed key" refusal are sent from tasks of their own, and the
revocation watcher runs in its own task, so a stalled relay (sends
waiting for room) cannot delay a revocation. A "changed key" warning can
be produced by anyone on the relay path, the relay included; it changes
nothing (pairing again needs a new QR code and the code shown on the
host), is worded so, and is shown at most once per device every ten
minutes.

## What the host connector exposes

Only `/api/...` paths of the hame web router, called in process with the
local token (`tower::ServiceExt::oneshot`): no `..`, `//`, `\`, `#` or
non-printable bytes. Responses go back as `res`; event streams as `evt`
(records whose `seq` ≤ `after` skipped; non-record events carry the last
record's seq, −1 before any); artifacts (`/api/artifacts/…`) as a `res`
with `{"blob":{"sha256","mime","size","chunks"}}` followed by 256 KiB `blob`
chunks streamed as they are read (never buffered whole by the connector),
hash-checked by the device against the content-addressed id. Plus two control frames for enrolled
devices: `devices` and `revoke`. No shell, file or generic RPC.

Every request the connector passes to the router carries hame web's
`RemoteCaller { device_id, label }` extension (`hame_web::mark_remote`,
re-exported by `connector`), which HTTP clients cannot set.

## Files

`<home>/sync/` (0700) holds `identity.json` (private keys), `allowlist.json`
(devices, pinned keys, replay windows, revoked keys, host counter
reservation, grant list version), `revocations.log` (append-only: every
revoked device id and key; a device listed there is never enrolled,
whatever `allowlist.json` says), `relay.json` (relay URL, device id,
tokens) and `lock`, each 0600, written atomically. Every read-modify-write,
by the running connector and by `hame connect revoke` alike, holds an
advisory lock on `lock` and re-reads under it, so a revocation is never
lost. A damaged line in `revocations.log` stops the host with an error
(never skipped: that would un-revoke a device), and an append after a torn
line starts on a new line. A running connector takes a revocation in
before the next envelope
and within half a second otherwise: the device's envelopes are refused, its
requests and streams stop, and the turns it started are cancelled through
the web API. Revocation has one path (the connector's copy changes, then
the device's tasks stop), and new tasks are admitted only for devices
still in the copy, under the same lock, so nothing of a revoked device
starts after the cleanup; a turn whose prompt was accepted just as the
device was revoked is cancelled when its answer arrives. `connect.lock`
allows one `hame connect` per hame home. Loading refuses a file or the
directory if the group or others have any access. On Windows the permission check is skipped (the profile
directory's ACL applies).

## Relay connection and HTTP

- `wss`/`https` only, except `localhost`, `127.0.0.1` and `[::1]` (dev).
- Incoming WebSocket messages and frames are limited to 1.1 MiB; HTTP
  answers are read up to 64 KiB, without following redirects.
- The access token is renewed on the open socket with `{"op":"auth","token"}`
  every 10 minutes (the relay's tokens live 15); an `unauthorized` error
  closes the connection and the next one starts with a refreshed token.
  `hame connect` refreshes one at a time, saves the rotated pair before
  using it, and never falls back to an old token.
- Envelopes queued for the socket are bounded to 16 MiB; a sender waits
  for room (backpressure on the streams producing them).
- A send stays unconfirmed until the relay answers `sent` or an error with
  the same recipient (`to`); the relay has no per-send id, so the oldest
  unconfirmed send to that recipient is matched. Unconfirmed sends are
  bounded to 1024 and 16 MiB.
- Everything the relay or another device supplies is printed without
  control or bidirectional formatting characters (`text::printable`),
  including errors `hame connect` returns (relay answers, JSON bodies).

## Dependencies added

| Crate | License |
|---|---|
| ed25519-dalek 2.2, x25519-dalek 2.0, curve25519-dalek 4.1 (+ derive) | BSD-3-Clause |
| chacha20poly1305 0.10, chacha20, poly1305, aead, cipher, universal-hash, inout, opaque-debug | MIT OR Apache-2.0 |
| hkdf 0.12, hmac 0.12, signature, ed25519, pkcs8, spki, der, const-oid, base64ct, sha1 | MIT OR Apache-2.0 |
| fiat-crypto | MIT OR Apache-2.0 OR BSD-1-Clause |
| rand 0.8/0.9, rand_core, rand_chacha, ppv-lite86 | MIT OR Apache-2.0 |
| zeroize, zeroize_derive | Apache-2.0 OR MIT |
| zerocopy (+ derive) | BSD-2-Clause OR Apache-2.0 OR MIT |
| tokio-tungstenite 0.29 | MIT |
| tungstenite 0.29, utf-8 | MIT OR Apache-2.0 |
| tower 0.5 (already in the tree) | MIT |
| qrcode 0.14 (hame-cli, no default features) | MIT OR Apache-2.0 |

TLS for `wss://` uses the rustls (aws-lc-rs provider) and native roots
already in the tree through reqwest.

npm (packages/sync-js): `@noble/curves`, `@noble/ciphers`, `@noble/hashes`
2.4.0 (MIT); dev: `vitest`, `ws`, `@types/*` (MIT), `typescript`
(Apache-2.0).
