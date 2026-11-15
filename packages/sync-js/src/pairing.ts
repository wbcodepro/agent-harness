// Pairing: the host's QR offer, the HMAC proof, the six-digit short
// authentication string and the host's signed grant. Same constructions as
// crates/hame-sync/src/pairing.rs.

import { hmac } from "@noble/hashes/hmac.js";
import { sha256 } from "@noble/hashes/sha2.js";
import { b64url, concat, equalBytes, fromB64url, fromB64url32, fromUtf8, utf8 } from "./encoding.js";
import { SyncError } from "./errors.js";
import { type PublicKeys, verifySignature } from "./keys.js";

export const SCHEME = "hame-pair:v1?";
export const GRANT_CONTEXT = utf8("hame-sync v1 grant");
/** The `kind` of a grant list a host publishes to the relay. */
export const GRANT_LIST_KIND = "hame-sync/grants";

export interface PairingOffer {
  relay: string;
  host: string;
  hostKeys: PublicKeys;
  secret: Uint8Array;
  /** Unix seconds. */
  expires: number;
}

export function offerToUri(offer: PairingOffer): string {
  const query = new URLSearchParams();
  query.append("relay", offer.relay);
  query.append("host", offer.host);
  query.append("ed", b64url(offer.hostKeys.ed));
  query.append("x", b64url(offer.hostKeys.x));
  query.append("secret", b64url(offer.secret));
  query.append("exp", String(offer.expires));
  return SCHEME + query.toString();
}

/** Parses `hame-pair:v1?relay=…&host=…&ed=…&x=…&secret=…&exp=…`. */
export function parseOffer(uri: string): PairingOffer {
  if (!uri.startsWith(SCHEME)) throw new SyncError("malformed", "not a hame pairing code");
  const query = new URLSearchParams(uri.slice(SCHEME.length));
  const fields = new Map<string, string>();
  for (const [key, value] of query) {
    if (fields.has(key)) throw new SyncError("malformed", "repeated field in pairing code");
    fields.set(key, value);
  }
  const field = (name: string): string => {
    const value = fields.get(name);
    if (value === undefined) throw new SyncError("malformed", "pairing code misses a field");
    return value;
  };
  const exp = field("exp");
  if (!/^[0-9]+$/.test(exp)) throw new SyncError("malformed", "bad expiry");
  return {
    relay: field("relay"),
    host: field("host"),
    hostKeys: { ed: fromB64url32(field("ed")), x: fromB64url32(field("x")) },
    secret: fromB64url32(field("secret")),
    expires: Number(exp),
  };
}

export function offerExpired(offer: PairingOffer, nowSeconds = Math.floor(Date.now() / 1000)): boolean {
  return nowSeconds >= offer.expires;
}

/** HMAC-SHA256(secret, client_ed ‖ client_x). */
export function pairingProof(secret: Uint8Array, client: PublicKeys): Uint8Array {
  return hmac(sha256, secret, concat(client.ed, client.x));
}

export function checkPairingProof(secret: Uint8Array, client: PublicKeys, proof: Uint8Array): boolean {
  return equalBytes(pairingProof(secret, client), proof);
}

/**
 * Six digits from SHA256("hame-sas" ‖ secret ‖ host_ed ‖ client_ed): the
 * first four bytes big-endian, modulo one million, zero-padded.
 */
export function sas(secret: Uint8Array, hostEd: Uint8Array, clientEd: Uint8Array): string {
  const digest = sha256(concat(utf8("hame-sas"), secret, hostEd, clientEd));
  const n = ((digest[0]! << 24) >>> 0) + (digest[1]! << 16) + (digest[2]! << 8) + digest[3]!;
  return String(n % 1_000_000).padStart(6, "0");
}

export interface Grant {
  v: number;
  host: string;
  device: string;
  ed: string;
  x: string;
  label: string;
  issued: number;
}

export interface SignedGrant {
  grant: string;
  sig: string;
}

function verifyDocument(hostEd: Uint8Array, context: Uint8Array, bytes: Uint8Array, signature: string): void {
  let sig: Uint8Array;
  try {
    sig = fromB64url(signature);
  } catch {
    throw new SyncError("bad_signature", "bad signature");
  }
  if (!verifySignature(hostEd, concat(context, bytes), sig)) throw new SyncError("bad_signature", "bad signature");
}

/** Verifies a grant with the host's pinned signing key. */
export function verifyGrant(signed: SignedGrant, hostEd: Uint8Array): Grant {
  const bytes = fromB64url(signed.grant);
  verifyDocument(hostEd, GRANT_CONTEXT, bytes, signed.sig);
  return JSON.parse(fromUtf8(bytes)) as Grant;
}

export interface GrantList {
  kind: typeof GRANT_LIST_KIND;
  v: number;
  host: string;
  devices: { device: string; ed: string; x: string; label: string }[];
  issued: number;
}

/**
 * Verifies a host's grant list as the relay returns it
 * (GET /v1/hosts/{id}/grants): a plain Ed25519 signature over the document
 * bytes, the document naming itself with kind "hame-sync/grants".
 */
export function verifyGrantList(signed: { document: string; signature: string }, hostEd: Uint8Array): GrantList {
  const bytes = fromB64url(signed.document);
  verifyDocument(hostEd, new Uint8Array(), bytes, signed.signature);
  const list = JSON.parse(fromUtf8(bytes)) as GrantList;
  if (list.kind !== GRANT_LIST_KIND) throw new SyncError("malformed", "not a grant list");
  return list;
}
