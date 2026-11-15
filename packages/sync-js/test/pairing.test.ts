import { describe, expect, it } from "vitest";
import {
  RawIdentity,
  checkPairingProof,
  offerExpired,
  offerToUri,
  pairingProof,
  parseOffer,
  randomBytes,
  sas,
  verifyGrant,
  approvalAnswer,
  argsSha256,
  canonicalJson,
  b64url,
  utf8,
  concat,
} from "../src/index.js";
import { GRANT_CONTEXT } from "../src/pairing.js";

describe("approvals", () => {
  it("answers bind the recomputed hash of the canonical arguments", () => {
    const question = { id: 3, tool_call_id: "c1", arguments: { path: "a.txt", content: "from afar" } };
    const answer = approvalAnswer({ ...question, args_sha256: argsSha256({ content: "from afar", path: "a.txt" }) }, true);
    expect(answer).toEqual({ id: 3, allow: true, tool_call_id: "c1", args_sha256: argsSha256(question.arguments) });
    expect(() => approvalAnswer({ ...question, args_sha256: "0".repeat(64) }, true)).toThrow("does not match");
    expect(canonicalJson({ b: [1, 2.5, 1e21], a: "é" })).toBe('{"a":"é","b":[1,2.5,1e+21]}');
  });
});

describe("pairing", () => {
  it("round-trips an offer through its URI and refuses bad ones", () => {
    const host = RawIdentity.generate();
    const offer = {
      relay: "https://relay.example/x?y=1&z",
      host: "host-1",
      hostKeys: host.publicKeys,
      secret: randomBytes(32),
      expires: Math.floor(Date.now() / 1000) + 600,
    };
    const uri = offerToUri(offer);
    expect(uri.startsWith("hame-pair:v1?relay=https%3A%2F%2Frelay.example")).toBe(true);
    expect(parseOffer(uri)).toEqual(offer);
    expect(() => parseOffer("hame-pair:v2?relay=x")).toThrow();
    expect(() => parseOffer(`${uri}&exp=1`)).toThrow();
    expect(() => parseOffer(uri.replace(/secret=[^&]+/, "secret=AAAA"))).toThrow();
    expect(() => parseOffer(offerToUri({ ...offer, relay: "http://relay.example" }))).toThrow("unencrypted");
    expect(parseOffer(offerToUri({ ...offer, relay: "http://127.0.0.1:8080" })).relay).toBe("http://127.0.0.1:8080");
    expect(() => parseOffer(offerToUri({ ...offer, host: "host 1" }))).toThrow("host id");
    expect(offerExpired(offer)).toBe(false);
    expect(offerExpired(offer, offer.expires)).toBe(true);
  });

  it("binds the proof and the SAS to the keys", () => {
    const secret = randomBytes(32);
    const client = RawIdentity.generate().publicKeys;
    const other = RawIdentity.generate().publicKeys;
    const proof = pairingProof(secret, client);
    expect(checkPairingProof(secret, client, proof)).toBe(true);
    expect(checkPairingProof(secret, other, proof)).toBe(false);
    expect(checkPairingProof(randomBytes(32), client, proof)).toBe(false);
    const nonce = new Uint8Array(32).fill(9);
    const code = sas(secret, new Uint8Array(32).fill(1), client.ed, nonce);
    expect(code).toMatch(/^[0-9]{8}$/);
    expect(code).not.toBe(sas(secret, new Uint8Array(32).fill(1), other.ed, nonce));
    expect(code).not.toBe(sas(secret, new Uint8Array(32).fill(1), client.ed, new Uint8Array(32).fill(8)));
  });

  it("verifies a grant only with the host key", async () => {
    const host = RawIdentity.generate();
    const grant = { v: 1, host: "h", device: "d", ed: "e", x: "x", label: "Phone", issued: 1 };
    const bytes = utf8(JSON.stringify(grant));
    const signed = { grant: b64url(bytes), sig: b64url(await host.sign(concat(GRANT_CONTEXT, bytes))) };
    expect(verifyGrant(signed, host.publicKeys.ed)).toEqual(grant);
    expect(() => verifyGrant(signed, RawIdentity.generate().publicKeys.ed)).toThrow();
  });
});
