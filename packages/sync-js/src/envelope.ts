// The end-to-end envelope, byte for byte the same as crates/hame-sync
// (src/envelope.rs). Wire layout, then base64url without padding:
//
//   0x01 | u16 BE header length | header JSON | 32 ephemeral X25519 public |
//   24 nonce | ciphertext + 16 tag | 64 Ed25519 signature
//
// key = HKDF-SHA256(ikm = X25519(ephemeral, recipient X), salt = ephemeral
// public ‖ recipient X public, info = "hame-sync v1 envelope key"), 32 bytes;
// ciphertext = XChaCha20-Poly1305(key, nonce, plaintext, aad = header bytes);
// signature = Ed25519(sender, "hame-sync v1 signature" ‖ every byte before
// the signature).

import { xchacha20poly1305 } from "@noble/ciphers/chacha.js";
import { hkdf } from "@noble/hashes/hkdf.js";
import { sha256 } from "@noble/hashes/sha2.js";
import { b64url, concat, fromB64url, fromUtf8, randomBytes, utf8 } from "./encoding.js";
import { SyncError } from "./errors.js";
import { type DeviceIdentity, type PublicKeys, verifySignature, x25519Agree, x25519Public } from "./keys.js";

export const VERSION = 1;
export const HKDF_INFO = utf8("hame-sync v1 envelope key");
export const SIGNATURE_CONTEXT = utf8("hame-sync v1 signature");
export const MAX_HEADER = 1024;
export const MAX_ID = 128;
export const MAX_PLAINTEXT = 512 * 1024;
export const MAX_ENVELOPE_TEXT = 1024 * 1024;
const OVERHEAD_AFTER_HEADER = 32 + 24 + 16 + 64;

export interface Header {
  v: 1;
  from: string;
  to: string;
  stream: string;
  counter: number;
}

export function header(from: string, to: string, stream: string, counter: number): Header {
  return { v: 1, from, to, stream, counter };
}

/** The header's JSON exactly as Rust's serde writes it. */
export function headerBytes(h: Header): Uint8Array {
  return utf8(JSON.stringify({ v: h.v, from: h.from, to: h.to, stream: h.stream, counter: h.counter }));
}

function checkHeader(h: Header): void {
  if (h.v !== 1) throw new SyncError("malformed", "unknown envelope version");
  for (const id of [h.from, h.to, h.stream]) {
    if (typeof id !== "string" || id.length === 0 || utf8(id).length > MAX_ID) {
      throw new SyncError("malformed", "bad id in header");
    }
  }
  if (!Number.isSafeInteger(h.counter) || h.counter < 0) throw new SyncError("malformed", "counter out of range");
}

function parseHeader(bytes: Uint8Array): Header {
  let value: unknown;
  try {
    value = JSON.parse(fromUtf8(bytes));
  } catch {
    throw new SyncError("malformed", "bad header");
  }
  const h = value as Record<string, unknown>;
  const keys = Object.keys(h ?? {}).sort().join(",");
  if (keys !== "counter,from,stream,to,v") throw new SyncError("malformed", "bad header");
  const parsed = h as unknown as Header;
  checkHeader(parsed);
  return parsed;
}

function deriveKey(shared: Uint8Array, ephemeral: Uint8Array, recipient: Uint8Array): Uint8Array {
  return hkdf(sha256, shared, concat(ephemeral, recipient), HKDF_INFO, 32);
}

/** Deterministic sealing with a given ephemeral scalar and nonce (test vectors). */
export async function sealWith(
  sender: DeviceIdentity,
  recipientX: Uint8Array,
  h: Header,
  plaintext: Uint8Array,
  ephemeralSeed: Uint8Array,
  nonce: Uint8Array,
): Promise<Uint8Array> {
  checkHeader(h);
  if (plaintext.length > MAX_PLAINTEXT) throw new SyncError("too_large", "plaintext too large");
  if (nonce.length !== 24 || ephemeralSeed.length !== 32) throw new SyncError("malformed", "bad nonce or key");
  const hb = headerBytes(h);
  if (hb.length > MAX_HEADER) throw new SyncError("malformed", "header too long");
  const ephemeralPublic = x25519Public(ephemeralSeed);
  const shared = x25519Agree(ephemeralSeed, recipientX);
  const key = deriveKey(shared, ephemeralPublic, recipientX);
  const ciphertext = xchacha20poly1305(key, nonce, hb).encrypt(plaintext);
  const length = new Uint8Array([(hb.length >> 8) & 0xff, hb.length & 0xff]);
  const before = concat(new Uint8Array([VERSION]), length, hb, ephemeralPublic, nonce, ciphertext);
  const signature = await sender.sign(concat(SIGNATURE_CONTEXT, before));
  return concat(before, signature);
}

/** Seals `plaintext` to the holder of `recipientX`; returns base64url text. */
export async function seal(sender: DeviceIdentity, recipientX: Uint8Array, h: Header, plaintext: Uint8Array): Promise<string> {
  return b64url(await sealWith(sender, recipientX, h, plaintext, randomBytes(32), randomBytes(24)));
}

/** A parsed envelope whose signature and contents are not checked yet. */
export class Sealed {
  private constructor(
    readonly header: Header,
    private readonly bytes: Uint8Array,
    private readonly headerEnd: number,
  ) {}

  static parse(text: string): Sealed {
    if (text.length > MAX_ENVELOPE_TEXT) throw new SyncError("too_large", "envelope too large");
    let bytes: Uint8Array;
    try {
      bytes = fromB64url(text);
    } catch {
      throw new SyncError("malformed", "bad base64url");
    }
    return Sealed.fromBytes(bytes);
  }

  static fromBytes(bytes: Uint8Array): Sealed {
    if (bytes.length < 3 || bytes[0] !== VERSION) throw new SyncError("malformed", "not a version 1 envelope");
    const headerLength = (bytes[1]! << 8) | bytes[2]!;
    if (headerLength > MAX_HEADER) throw new SyncError("malformed", "header too long");
    const headerEnd = 3 + headerLength;
    if (bytes.length < headerEnd + OVERHEAD_AFTER_HEADER) throw new SyncError("malformed", "envelope too short");
    if (bytes.length - headerEnd - OVERHEAD_AFTER_HEADER > MAX_PLAINTEXT) throw new SyncError("too_large", "envelope too large");
    return new Sealed(parseHeader(bytes.subarray(3, headerEnd)), bytes, headerEnd);
  }

  /** Checks the sender's signature against its pinned signing key. */
  verify(senderEd: Uint8Array): boolean {
    const end = this.bytes.length - 64;
    return verifySignature(senderEd, concat(SIGNATURE_CONTEXT, this.bytes.subarray(0, end)), this.bytes.subarray(end));
  }

  /** Decrypts; does not check the signature (use open). */
  async decrypt(recipient: DeviceIdentity): Promise<Uint8Array> {
    const body = this.bytes.subarray(this.headerEnd, this.bytes.length - 64);
    const ephemeral = body.subarray(0, 32);
    const nonce = body.subarray(32, 56);
    const ciphertext = body.subarray(56);
    const shared = await recipient.agree(ephemeral);
    const key = deriveKey(shared, ephemeral, recipient.publicKeys.x);
    try {
      return xchacha20poly1305(key, nonce, this.bytes.subarray(3, this.headerEnd)).decrypt(ciphertext);
    } catch {
      throw new SyncError("crypto", "decryption failed");
    }
  }

  /**
   * Everything a recipient checks, in order: addressed to `myId`, signed by
   * the pinned `sender` key, a counter above `lastCounter`, contents that
   * decrypt. Record the counter only after this succeeds.
   */
  async open(me: DeviceIdentity, myId: string, sender: PublicKeys, lastCounter: number | undefined): Promise<Uint8Array> {
    if (this.header.to !== myId) throw new SyncError("wrong_recipient", "envelope addressed to another device");
    if (!this.verify(sender.ed)) throw new SyncError("bad_signature", "bad signature");
    if (lastCounter !== undefined && this.header.counter <= lastCounter) {
      throw new SyncError("replay", `replayed or reordered envelope (counter ${this.header.counter}, last ${lastCounter})`);
    }
    return this.decrypt(me);
  }
}

/**
 * A sender's monotonic counter, from the clock in microseconds so a device
 * that lost its state still counts upwards.
 */
export class Counter {
  constructor(private last = 0) {}
  get value(): number {
    return this.last;
  }
  next(): number {
    const now = Date.now() * 1000;
    this.last = Math.max(now, this.last + 1);
    return this.last;
  }
}
