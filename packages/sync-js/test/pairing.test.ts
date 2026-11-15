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
  b64url,
  utf8,
  concat,
} from "../src/index.js";
import { GRANT_CONTEXT } from "../src/pairing.js";

describe("pairing", () => {
  it("round-trips an offer through its URI and refuses bad ones", () => {
    const host = RawIdentity.generate();
    const offer = {
      relay: "https://relay.example/x?y=1&z",
      host: "host 1",
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
    const code = sas(secret, new Uint8Array(32).fill(1), client.ed);
    expect(code).toMatch(/^[0-9]{6}$/);
    expect(code).not.toBe(sas(secret, new Uint8Array(32).fill(1), other.ed));
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
