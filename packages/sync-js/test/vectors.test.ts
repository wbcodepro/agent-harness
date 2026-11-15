// Cross-language vectors: rust.json (from crates/hame-sync/tests/vectors.rs)
// is checked here; ts.json is generated here and checked by the Rust tests.
// Regenerate ts.json with `npm run vectors`.

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { sha256 } from "@noble/hashes/sha2.js";
import { describe, expect, it } from "vitest";
import {
  Sealed,
  type Header,
  RawIdentity,
  b64url,
  fingerprint,
  fromB64url,
  fromB64url32,
  offerToUri,
  pairingProof,
  parseOffer,
  sas,
  sealWith,
  utf8,
  verifyGrant,
  verifyGrantList,
  verifySignature,
} from "../src/index.js";
import { GRANT_CONTEXT } from "../src/pairing.js";
import { concat } from "../src/encoding.js";

const here = dirname(fileURLToPath(import.meta.url));
const vectorsDir = join(here, "../../../crates/hame-sync/tests/vectors");

type Json = Record<string, any>;

function seed(label: string): Uint8Array {
  return sha256(utf8(label));
}

function identity(label: string): { id: RawIdentity; json: Json } {
  const ed = seed(`${label}/ed`);
  const x = seed(`${label}/x`);
  const id = RawIdentity.fromSeeds(ed, x);
  return {
    id,
    json: { ed_seed: b64url(ed), x_seed: b64url(x), ed: b64url(id.publicKeys.ed), x: b64url(id.publicKeys.x) },
  };
}

function keysOf(value: Json): RawIdentity {
  const id = RawIdentity.fromSeeds(fromB64url32(value.ed_seed), fromB64url32(value.x_seed));
  expect(b64url(id.publicKeys.ed)).toBe(value.ed);
  expect(b64url(id.publicKeys.x)).toBe(value.x);
  return id;
}

async function check(vectors: Json): Promise<void> {
  for (const vector of vectors.envelopes as Json[]) {
    const sender = keysOf(vector.sender);
    const recipient = keysOf(vector.recipient);
    const sealed = Sealed.parse(vector.envelope);
    const h = vector.header as Header;
    expect(sealed.header).toEqual(h);
    const plain = await sealed.open(recipient, h.to, sender.publicKeys, h.counter - 1);
    expect(b64url(plain)).toBe(vector.plaintext);
    const again = await sealWith(
      sender,
      recipient.publicKeys.x,
      h,
      plain,
      fromB64url32(vector.ephemeral_seed),
      fromB64url(vector.nonce),
    );
    expect(b64url(again)).toBe(vector.envelope);
    const bytes = fromB64url(vector.envelope);
    bytes[bytes.length >> 1]! ^= 0x80;
    await expect(Sealed.fromBytes(bytes).open(recipient, h.to, sender.publicKeys, undefined)).rejects.toThrow();
  }
  for (const vector of vectors.pairing as Json[]) {
    const host = keysOf(vector.host);
    const client = keysOf(vector.client);
    const secret = fromB64url32(vector.secret);
    const offer = parseOffer(vector.uri);
    expect(offer.relay).toBe(vector.relay);
    expect(offer.host).toBe(vector.host_id);
    expect(offer.expires).toBe(vector.exp);
    expect(offer.hostKeys.ed).toEqual(host.publicKeys.ed);
    expect(offer.secret).toEqual(secret);
    expect(offerToUri(offer)).toBe(vector.uri);
    expect(b64url(pairingProof(secret, client.publicKeys))).toBe(vector.mac);
    expect(sas(secret, host.publicKeys.ed, client.publicKeys.ed)).toBe(vector.sas);
  }
  for (const vector of vectors.fingerprints as Json[]) {
    expect(fingerprint(keysOf(vector.keys).publicKeys)).toBe(vector.fingerprint);
  }
  for (const vector of vectors.grants as Json[]) {
    const host = keysOf(vector.host);
    const grant = verifyGrant(vector.signed, host.publicKeys.ed);
    expect(grant).toEqual(vector.grant);
    // Signing the same bytes gives the same signature (Ed25519 is deterministic).
    const signature = await host.sign(concat(GRANT_CONTEXT, fromB64url(vector.signed.grant)));
    expect(b64url(signature)).toBe(vector.signed.sig);
  }
  for (const vector of (vectors.grant_lists ?? []) as Json[]) {
    const host = keysOf(vector.host);
    const document = fromB64url(vector.signed.document);
    // What the relay checks: a plain signature over the document bytes.
    expect(verifySignature(host.publicKeys.ed, document, fromB64url(vector.signed.signature))).toBe(true);
    expect(verifyGrantList(vector.signed, host.publicKeys.ed)).toEqual(vector.list);
    expect(b64url(await host.sign(document))).toBe(vector.signed.signature);
  }
}

async function generate(): Promise<Json> {
  const plaintexts: [string, Uint8Array][] = [
    ["event frame", utf8('{"t":"evt","id":8,"seq":121,"data":{"kind":"record","record":{"seq":121}}}')],
    ["empty", new Uint8Array()],
    ["unicode", utf8("xin chào — 世界 🚀")],
    ["binary", Uint8Array.from({ length: 777 }, (_, n) => (n * 13) % 256)],
  ];
  const envelopes: Json[] = [];
  for (const [n, [name, plaintext]] of plaintexts.entries()) {
    const sender = identity(`ts/sender/${n}`);
    const recipient = identity(`ts/recipient/${n}`);
    const ephemeral = seed(`ts/ephemeral/${n}`);
    const nonce = seed(`ts/nonce/${n}`).subarray(0, 24);
    const h: Header = { v: 1, from: `host-${n}`, to: `phone-${n}`, stream: String(100 + n), counter: 1_763_100_000_000_000 + n };
    const bytes = await sealWith(sender.id, recipient.id.publicKeys.x, h, plaintext, ephemeral, nonce);
    envelopes.push({
      name,
      sender: sender.json,
      recipient: recipient.json,
      ephemeral_seed: b64url(ephemeral),
      nonce: b64url(nonce),
      header: h,
      plaintext: b64url(plaintext),
      envelope: b64url(bytes),
    });
  }
  const pairing: Json[] = [];
  for (let n = 0; n < 3; n++) {
    const host = identity(`ts/host/${n}`);
    const client = identity(`ts/client/${n}`);
    const secret = seed(`ts/secret/${n}`);
    const offer = {
      relay: `https://relay.example.org/r ${n}?x=1&y`,
      host: `host-${n}`,
      hostKeys: host.id.publicKeys,
      secret,
      expires: 1_763_100_600 + n,
    };
    pairing.push({
      host: host.json,
      client: client.json,
      secret: b64url(secret),
      relay: offer.relay,
      host_id: offer.host,
      exp: offer.expires,
      uri: offerToUri(offer),
      mac: b64url(pairingProof(secret, client.id.publicKeys)),
      sas: sas(secret, host.id.publicKeys.ed, client.id.publicKeys.ed),
    });
  }
  const fingerprints = [0, 1, 2].map((n) => {
    const { id, json } = identity(`ts/fingerprint/${n}`);
    return { keys: json, fingerprint: fingerprint(id.publicKeys) };
  });
  const host = identity("ts/grant/host");
  const device = identity("ts/grant/device");
  // Field order as Rust's Grant serialises it.
  const grant = {
    v: 1,
    host: "host-ts",
    device: "device-ts",
    ed: b64url(device.id.publicKeys.ed),
    x: b64url(device.id.publicKeys.x),
    label: "Máy tính ✓",
    issued: 1_763_100_000,
  };
  const grantBytes = utf8(JSON.stringify(grant));
  const sig = await host.id.sign(concat(GRANT_CONTEXT, grantBytes));
  // Field order as Rust's GrantList serialises it.
  const list = {
    kind: "hame-sync/grants",
    v: 1,
    host: "host-ts",
    devices: [{ device: "device-ts", ed: grant.ed, x: grant.x, label: grant.label }],
    issued: 1_763_100_000,
  };
  const listBytes = utf8(JSON.stringify(list));
  const listSig = await host.id.sign(listBytes);
  return {
    comment: "Generated by packages/sync-js/test/vectors.test.ts. Base64url without padding throughout.",
    envelopes,
    pairing,
    fingerprints,
    grants: [{ host: host.json, grant, signed: { grant: b64url(grantBytes), sig: b64url(sig) } }],
    grant_lists: [{ host: host.json, list, signed: { document: b64url(listBytes), signature: b64url(listSig) } }],
  };
}

describe("cross-language vectors", () => {
  it("verifies and reproduces the Rust vectors", async () => {
    const vectors = JSON.parse(readFileSync(join(vectorsDir, "rust.json"), "utf8")) as Json;
    expect(vectors.envelopes.length).toBeGreaterThan(0);
    await check(vectors);
  });

  it("keeps the TypeScript vectors current and valid", async () => {
    const generated = await generate();
    const path = join(vectorsDir, "ts.json");
    if (process.env.HAME_SYNC_WRITE_VECTORS) {
      writeFileSync(path, `${JSON.stringify(generated, null, 2)}\n`);
    }
    const stored = JSON.parse(readFileSync(path, "utf8")) as Json;
    expect(stored).toEqual(JSON.parse(JSON.stringify(generated)));
    await check(stored);
  });
});
