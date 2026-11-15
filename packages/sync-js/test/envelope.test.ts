import { describe, expect, it } from "vitest";
import {
  Counter,
  REPLAY_WINDOW,
  ReplayWindow,
  validId,
  MAX_ENVELOPE_TEXT,
  MAX_PLAINTEXT,
  MemoryKeyStorage,
  RawIdentity,
  Sealed,
  SyncError,
  WebCryptoIdentity,
  b64,
  b64url,
  displayFingerprint,
  fingerprint,
  fromB64,
  fromB64url,
  header,
  loadOrCreateIdentity,
  restoreIdentity,
  seal,
  utf8,
  fromUtf8,
} from "../src/index.js";
import type { KeyStorage } from "../src/index.js";

describe("encoding", () => {
  it("round-trips base64 and base64url and refuses junk", () => {
    for (let n = 0; n < 40; n++) {
      const bytes = Uint8Array.from({ length: n }, (_, i) => (i * 37 + n) % 256);
      expect(fromB64url(b64url(bytes))).toEqual(bytes);
      expect(fromB64(b64(bytes))).toEqual(bytes);
      expect(b64(bytes)).toBe(Buffer.from(bytes).toString("base64"));
      expect(b64url(bytes)).toBe(Buffer.from(bytes).toString("base64url"));
    }
    expect(() => fromB64url("a")).toThrow();
    expect(() => fromB64url("ab=")).toThrow();
    expect(() => fromB64url("a+")).toThrow();
    // Non-canonical trailing bits are refused, as in Rust.
    expect(() => fromB64url("AB")).toThrow();
  });
});

describe("envelope", () => {
  it("opens for its recipient and refuses every tampering", async () => {
    const alice = RawIdentity.generate();
    const bob = RawIdentity.generate();
    const mallory = RawIdentity.generate();
    const h = header("alice", "bob", "7", 10);
    const text = await seal(alice, bob.publicKeys.x, h, utf8("secret"));
    const sealed = Sealed.parse(text);
    expect(sealed.header).toEqual(h);
    expect(fromUtf8(await sealed.open(bob, "bob", alice.publicKeys, ReplayWindow.startingAt(9)))).toBe("secret");

    await expect(sealed.open(bob, "carol", alice.publicKeys, undefined)).rejects.toMatchObject({ code: "wrong_recipient" });
    await expect(sealed.open(bob, "bob", mallory.publicKeys, undefined)).rejects.toMatchObject({ code: "bad_signature" });
    await expect(sealed.open(bob, "bob", alice.publicKeys, ReplayWindow.startingAt(10))).rejects.toMatchObject({ code: "replay" });
    await expect(sealed.decrypt(mallory)).rejects.toBeInstanceOf(SyncError);

    const bytes = fromB64url(text);
    for (const at of [5, bytes.length >> 1, bytes.length - 70, bytes.length - 1]) {
      const copy = Uint8Array.from(bytes);
      copy[at]! ^= 1;
      let refused = false;
      try {
        await Sealed.fromBytes(copy).open(bob, "bob", alice.publicKeys, undefined);
      } catch {
        refused = true;
      }
      expect(refused).toBe(true);
    }
  });

  it("refuses oversize input both ways", async () => {
    const alice = RawIdentity.generate();
    const bob = RawIdentity.generate();
    const h = header("a", "b", "1", 1);
    await expect(seal(alice, bob.publicKeys.x, h, new Uint8Array(MAX_PLAINTEXT + 1))).rejects.toMatchObject({ code: "too_large" });
    await seal(alice, bob.publicKeys.x, h, new Uint8Array(MAX_PLAINTEXT));
    expect(() => Sealed.parse("A".repeat(MAX_ENVELOPE_TEXT + 1))).toThrow(SyncError);
  });

  it("refuses a low-order recipient key", async () => {
    const alice = RawIdentity.generate();
    await expect(seal(alice, new Uint8Array(32), header("a", "b", "1", 1), utf8("x"))).rejects.toMatchObject({ code: "crypto" });
  });

  it("counts and accepts each counter once within the window", () => {
    const counter = new Counter();
    expect([counter.next(), counter.next()]).toEqual([1, 2]);
    const window = ReplayWindow.startingAt(10);
    expect(() => window.check(10)).toThrow();
    for (const c of [12, 11, 15]) {
      window.check(c);
      window.record(c);
      expect(() => window.check(c)).toThrow();
    }
    window.check(13);
    window.record(100);
    expect(() => window.check(100 - REPLAY_WINDOW)).toThrow();
    window.check(100 - REPLAY_WINDOW + 1);
    window.record(1000);
    expect(window.seen).toBe(1n);
  });

  it("refuses ids outside the safe alphabet", async () => {
    const alice = RawIdentity.generate();
    await expect(seal(alice, alice.publicKeys.x, header("a b", "b", "1", 1), utf8("x"))).rejects.toMatchObject({ code: "malformed" });
    expect(validId("dev-1_x")).toBe(true);
    expect(validId("é")).toBe(false);
  });
});

describe("keys", () => {
  it("uses non-extractable WebCrypto keys that interoperate with raw keys", async () => {
    expect(await WebCryptoIdentity.supported()).toBe(true);
    const web = await WebCryptoIdentity.generate();
    expect(web.extractable).toBe(false);
    const raw = RawIdentity.generate();
    // raw → web
    const h = header("raw", "web", "1", 5);
    const text = await seal(raw, web.publicKeys.x, h, utf8("to the browser"));
    expect(fromUtf8(await Sealed.parse(text).open(web, "web", raw.publicKeys, undefined))).toBe("to the browser");
    // web → raw
    const back = await seal(web, raw.publicKeys.x, header("web", "raw", "1", 6), utf8("from the browser"));
    expect(fromUtf8(await Sealed.parse(back).open(raw, "raw", web.publicKeys, undefined))).toBe("from the browser");
    expect(fingerprint(web.publicKeys)).toMatch(/^[0-9a-f]{64}$/);
    expect(displayFingerprint(web.publicKeys)).toMatch(/^([0-9a-f]{4} ){15}[0-9a-f]{4}$/);
  });

  it("uses raw keys only when asked to, and says so", async () => {
    const keychain: KeyStorage = {
      storesCryptoKeys: false,
      get: async () => undefined,
      set: async () => undefined,
      delete: async () => undefined,
    };
    await expect(loadOrCreateIdentity(keychain)).rejects.toMatchObject({ code: "crypto" });
    const identity = await loadOrCreateIdentity(keychain, { allowRawKeys: true });
    expect(identity.kind).toBe("raw");
  });

  it("stores identities: CryptoKeys where possible, JSON strings for keychains", async () => {
    const browser = new MemoryKeyStorage();
    const first = await loadOrCreateIdentity(browser);
    expect(first).toBeInstanceOf(WebCryptoIdentity);
    const again = await loadOrCreateIdentity(browser);
    expect(again.publicKeys).toEqual(first.publicKeys);

    const strings = new Map<string, unknown>();
    const keychain: KeyStorage = {
      storesCryptoKeys: false,
      get: async (name) => strings.get(name),
      set: async (name, value) => {
        expect(typeof value).toBe("string");
        strings.set(name, value);
      },
      delete: async (name) => {
        strings.delete(name);
      },
    };
    const mobile = await loadOrCreateIdentity(keychain, { allowRawKeys: true });
    expect(mobile).toBeInstanceOf(RawIdentity);
    expect((await loadOrCreateIdentity(keychain, { allowRawKeys: true })).publicKeys).toEqual(mobile.publicKeys);
    await expect(loadOrCreateIdentity(keychain)).rejects.toThrow("allowRawKeys");
    await expect(restoreIdentity({ kind: "nope" })).rejects.toThrow();
  });
});
