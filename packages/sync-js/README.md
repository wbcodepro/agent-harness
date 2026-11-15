# @hame/sync

The device side of hame's end-to-end encrypted remote transport: envelopes,
pairing, key storage, a relay WebSocket client, and `RemoteTransport`, the
UI's `Transport` over a paired host. Byte-compatible with
`crates/hame-sync` (shared test vectors in
`crates/hame-sync/tests/vectors/`); the constructions are described in
`crates/hame-sync/README.md`.

## Using it in the UI

```ts
import { IndexedDbKeyStorage, connectRemote, loadOrCreateIdentity, parseOffer } from "@hame/sync";

const storage = new IndexedDbKeyStorage(); // or an adapter over the OS keychain
const identity = await loadOrCreateIdentity(storage); // non-extractable WebCrypto keys where possible

// deviceId and token come from the relay: register once with
// POST /v1/devices {name, kind, ed25519: b64url(identity.publicKeys.ed), x25519: b64url(identity.publicKeys.x)}
const { transport, host, relay } = connectRemote({
  relayUrl, deviceId, token: async () => accessToken, identity,
  hostId, hostKeys, // hostKeys: pinned at pairing; omit before pairing
});

// Pairing: scan the QR code shown by `hame connect` or the desktop app.
const offer = parseOffer(scannedText);
const { sas, grant } = await host.pair(offer, "My phone");
showToUser(sas);           // the host shows the same six digits
const granted = await grant; // the host user confirmed; persist offer.hostKeys

// Then the UI talks to `transport` exactly as to the local one:
const res = await transport.request("POST", "/api/sessions", {});
const stop = transport.subscribe(`/api/sessions/${id}/events`, -1, onEvent, onEnd);
const blob = await transport.fetchBlob(sha256); // session learned from the stream, or pass it
```

Notes:

- `afterSeq` is the last record seq seen; `-1` means from the start
  (records are numbered from 0). Events that are not records carry the last
  record's seq.
- Approval answers must echo the question's `tool_call_id` and
  `args_sha256`: `POST /api/sessions/{id}/approval {id, allow, tool_call_id, args_sha256}`.
- The relay token is sent as the WebSocket subprotocol `bearer.<token>` next
  to `hame-relay.v1`.
- `CounterStore` (optional) persists the last counter accepted from each host,
  so a restarted app still refuses replayed envelopes from before.
- When the host comes back online (relay presence), live subscriptions are
  sent again from their last seq.

## Tests

`npm test` (type check + vitest): vectors from Rust, envelope and key tests
(including WebCrypto ↔ raw interop), pairing, and the remote transport
against a mock relay and a host written with this library.
`npm run vectors` regenerates `ts.json`.
