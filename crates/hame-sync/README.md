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
  the exact bytes that were authenticated (never re-serialised); ids at most
  128 bytes; `counter` at most 2^53 − 1.
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
  with the pinned key; `counter` is above the last accepted from that
  sender; the ciphertext decrypts; the frame's id matches the header's
  `stream`. The counter is recorded (and persisted, on the host) only after
  all of this.
- Counters: `max(last + 1, unix time in µs)`, so a sender that lost its
  state still counts upwards. Senders seal and queue strictly in counter
  order (the relay preserves order per recipient).

**Pairing**:

- QR: `hame-pair:v1?relay=…&host=<device id>&ed=…&x=…&secret=<32 bytes>&exp=<unix>`
  (form-encoded query; one use; ten minutes).
- The device sends `{"t":"pair","ed","x","label","mac"}` sealed to the
  host's X25519 key and signed with its own new Ed25519 key, `mac =
  HMAC-SHA256(secret, client_ed ‖ client_x)`. The host verifies the
  signature with the presented key and the MAC in constant time, spends
  the offer, and refuses revoked keys and changed keys for known devices
  (with a warning).
- SAS: the first four bytes of `SHA256("hame-sas" ‖ secret ‖ host_ed ‖
  client_ed)` as a big-endian number, modulo 1 000 000, six digits.
- After the user confirms on the host: a grant `{v, host, device, ed, x,
  label, issued}`, signed as `Ed25519(host, "hame-sync v1 grant" ‖ JSON
  bytes)`, travels back in a `paired` frame.
- Published grant list (`POST /v1/hosts/{id}/grants {document, signature}`):
  `document` is the JSON `{"kind":"hame-sync/grants","v":1,"host","devices":[…],"issued"}`
  and `signature` a plain Ed25519 signature over those bytes, which the
  relay verifies with the host's registered key. The `kind` field keeps it
  apart from everything else the host key signs (all other messages start
  with a `hame-sync v1 …` or `hame-relay-rebind` prefix).

**Approvals**: the hame web API's approval event carries `tool_call_id` and
`args_sha256` (sha256 of the JSON arguments as serialised by the host). A
remote answer must be `{id, allow, tool_call_id, args_sha256}` naming the
question the connector relayed; anything else is answered 409 and never
reaches the agent. A path that might reach the approval route in another
form (query string variants are normalised; percent-encoded or otherwise
odd forms) is refused.

## What the host connector exposes

Only `/api/...` paths of the hame web router, called in process with the
local token (`tower::ServiceExt::oneshot`): no `..`, `//`, `\`, `#` or
non-printable bytes. Responses go back as `res`; event streams as `evt`
(records whose `seq` ≤ `after` skipped; non-record events carry the last
record's seq, −1 before any); artifacts (`/api/artifacts/…`) as a `res`
with `{"blob":{"sha256","mime","size","chunks"}}` followed by 256 KiB `blob`
chunks, hash-checked by the device. Plus two control frames for enrolled
devices: `devices` and `revoke`. No shell, file or generic RPC.

Every request the connector passes to the router carries a request
extension `RemoteCaller { device_id, label }` (`connector::mark_remote`),
which HTTP clients cannot set, so the web layer can apply a remote policy.
Until hame-web exposes its own type, `RemoteCaller` is defined here; switch
`mark_remote` to the hame-web type when it lands.

## Files

`<home>/sync/` (0700) holds `identity.json` (private keys), `allowlist.json`
(devices, pinned keys, last counters, revoked keys, host counter) and
`relay.json` (relay URL, device id, tokens), each 0600, written atomically.
Loading refuses a file or the directory if the group or others have any
access. `hame connect revoke` edits the allowlist; a running connector picks
the change up within two seconds and stops the device's streams. On Windows
the permission check is skipped (the profile directory's ACL applies).

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
