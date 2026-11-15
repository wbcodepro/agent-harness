// Canonical JSON (RFC 8785, JSON Canonicalization Scheme), the same as
// crates/hame-sync/src/jcs.rs: members sorted by the UTF-16 code units of
// their names, no whitespace, JSON.stringify's escaping and number form.

import { sha256 } from "@noble/hashes/sha2.js";
import { hex, utf8 } from "./encoding.js";
import { SyncError } from "./errors.js";

export function canonicalJson(value: unknown): string {
  if (value === null || typeof value === "boolean" || typeof value === "string") return JSON.stringify(value);
  if (typeof value === "number") {
    if (!Number.isFinite(value)) throw new SyncError("malformed", "non-finite number");
    return JSON.stringify(value);
  }
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
  if (typeof value === "object") {
    // Default sort compares UTF-16 code units, as RFC 8785 asks.
    const keys = Object.keys(value as Record<string, unknown>).sort();
    return `{${keys.map((k) => `${JSON.stringify(k)}:${canonicalJson((value as Record<string, unknown>)[k])}`).join(",")}}`;
  }
  throw new SyncError("malformed", "not JSON");
}

/** sha256 (lowercase hex) of the arguments' canonical JSON: an approval's binding hash. */
export function argsSha256(args: unknown): string {
  return hex(sha256(utf8(canonicalJson(args))));
}
