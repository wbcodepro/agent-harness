// Byte helpers shared by the protocol: base64url (keys, envelopes),
// standard base64 (blob chunks), UTF-8 and hex.

const encoder = new TextEncoder();
const decoder = new TextDecoder("utf-8", { fatal: true });

export function utf8(text: string): Uint8Array {
  return encoder.encode(text);
}

export function fromUtf8(bytes: Uint8Array): string {
  return decoder.decode(bytes);
}

export function concat(...parts: Uint8Array[]): Uint8Array {
  let length = 0;
  for (const part of parts) length += part.length;
  const out = new Uint8Array(length);
  let at = 0;
  for (const part of parts) {
    out.set(part, at);
    at += part.length;
  }
  return out;
}

export function equalBytes(a: Uint8Array, b: Uint8Array): boolean {
  if (a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i++) diff |= a[i]! ^ b[i]!;
  return diff === 0;
}

export function hex(bytes: Uint8Array): string {
  let out = "";
  for (const b of bytes) out += b.toString(16).padStart(2, "0");
  return out;
}

const B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const B64URL = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

function encode(bytes: Uint8Array, alphabet: string, pad: boolean): string {
  let out = "";
  let i = 0;
  for (; i + 2 < bytes.length; i += 3) {
    const n = (bytes[i]! << 16) | (bytes[i + 1]! << 8) | bytes[i + 2]!;
    out += alphabet[(n >> 18) & 63]! + alphabet[(n >> 12) & 63]! + alphabet[(n >> 6) & 63]! + alphabet[n & 63]!;
  }
  const rest = bytes.length - i;
  if (rest === 1) {
    const n = bytes[i]! << 16;
    out += alphabet[(n >> 18) & 63]! + alphabet[(n >> 12) & 63]! + (pad ? "==" : "");
  } else if (rest === 2) {
    const n = (bytes[i]! << 16) | (bytes[i + 1]! << 8);
    out += alphabet[(n >> 18) & 63]! + alphabet[(n >> 12) & 63]! + alphabet[(n >> 6) & 63]! + (pad ? "=" : "");
  }
  return out;
}

function decode(text: string, alphabet: string, padded: boolean): Uint8Array {
  let body = text;
  if (padded) {
    if (body.length % 4 !== 0) throw new Error("bad base64");
    body = body.replace(/={1,2}$/, "");
  }
  if (body.length % 4 === 1) throw new Error("bad base64");
  const lookup = new Int16Array(128).fill(-1);
  for (let i = 0; i < 64; i++) lookup[alphabet.charCodeAt(i)] = i;
  const out = new Uint8Array(Math.floor((body.length * 3) / 4));
  let bits = 0;
  let value = 0;
  let at = 0;
  for (let i = 0; i < body.length; i++) {
    const code = body.charCodeAt(i);
    const digit = code < 128 ? lookup[code]! : -1;
    if (digit < 0) throw new Error("bad base64");
    value = (value << 6) | digit;
    bits += 6;
    if (bits >= 8) {
      bits -= 8;
      out[at++] = (value >> bits) & 0xff;
    }
  }
  // Leftover bits must be zero (canonical encoding), as in Rust's decoder.
  if (bits > 0 && (value & ((1 << bits) - 1)) !== 0) throw new Error("bad base64");
  return out;
}

/** Base64url without padding. */
export function b64url(bytes: Uint8Array): string {
  return encode(bytes, B64URL, false);
}

export function fromB64url(text: string): Uint8Array {
  return decode(text, B64URL, false);
}

/** Decodes a base64url field that must hold exactly 32 bytes. */
export function fromB64url32(text: string): Uint8Array {
  const bytes = fromB64url(text);
  if (bytes.length !== 32) throw new Error("a key must be 32 bytes");
  return bytes;
}

/** Standard base64 with padding (blob chunks). */
export function b64(bytes: Uint8Array): string {
  return encode(bytes, B64, true);
}

export function fromB64(text: string): Uint8Array {
  return decode(text, B64, true);
}

export function randomBytes(length: number): Uint8Array {
  const out = new Uint8Array(length);
  globalThis.crypto.getRandomValues(out);
  return out;
}
