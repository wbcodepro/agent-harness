// Device identities: an Ed25519 signing key and an X25519 key.
//
// Two kinds:
// - WebCryptoIdentity: non-extractable CryptoKeys (browsers, desktop web
//   views); the private keys can be stored in IndexedDB but never read out.
// - RawIdentity: seeds held in memory and in an injectable KeyStorage (a
//   mobile keychain), for runtimes without WebCrypto Ed25519/X25519.

import { ed25519, x25519 } from "@noble/curves/ed25519.js";
import { sha256 } from "@noble/hashes/sha2.js";
import { b64url, concat, fromB64url32, hex, randomBytes } from "./encoding.js";
import { SyncError } from "./errors.js";

export interface PublicKeys {
  ed: Uint8Array;
  x: Uint8Array;
}

export interface DeviceIdentity {
  /**
   * "webcrypto": non-extractable keys; "raw": seeds in memory and in the
   * storage. The UI should say so when it is "raw".
   */
  readonly kind: "webcrypto" | "raw";
  readonly publicKeys: PublicKeys;
  /** Ed25519 signature of `message`. */
  sign(message: Uint8Array): Promise<Uint8Array>;
  /** X25519 with a peer's public key; refuses an all-zero result. */
  agree(peerX: Uint8Array): Promise<Uint8Array>;
}

function isZero(bytes: Uint8Array): boolean {
  let acc = 0;
  for (const b of bytes) acc |= b;
  return acc === 0;
}

/** X25519 with a raw scalar (also used for the ephemeral key). */
export function x25519Agree(secret: Uint8Array, peer: Uint8Array): Uint8Array {
  let shared: Uint8Array;
  try {
    shared = x25519.getSharedSecret(secret, peer);
  } catch {
    throw new SyncError("crypto", "low-order key");
  }
  if (isZero(shared)) throw new SyncError("crypto", "low-order key");
  return shared;
}

export function x25519Public(secret: Uint8Array): Uint8Array {
  return x25519.getPublicKey(secret);
}

/** Strict Ed25519 verification (no ZIP-215 leniency), like Rust's verify_strict. */
export function verifySignature(publicKey: Uint8Array, message: Uint8Array, signature: Uint8Array): boolean {
  if (signature.length !== 64 || publicKey.length !== 32) return false;
  try {
    return ed25519.verify(signature, message, publicKey, { zip215: false });
  } catch {
    return false;
  }
}

/** Lowercase hex of SHA256(ed ‖ x): the relay's `fingerprint` of a device. */
export function fingerprint(keys: PublicKeys): string {
  return hex(sha256(concat(keys.ed, keys.x)));
}

/** A fingerprint in groups of four digits, for people to compare. */
export function displayFingerprint(keys: PublicKeys): string {
  return fingerprint(keys).match(/.{4}/g)!.join(" ");
}

export class RawIdentity implements DeviceIdentity {
  readonly kind = "raw" as const;
  readonly publicKeys: PublicKeys;

  private constructor(
    private readonly edSeed: Uint8Array,
    private readonly xSeed: Uint8Array,
  ) {
    this.publicKeys = { ed: ed25519.getPublicKey(edSeed), x: x25519.getPublicKey(xSeed) };
  }

  static fromSeeds(edSeed: Uint8Array, xSeed: Uint8Array): RawIdentity {
    if (edSeed.length !== 32 || xSeed.length !== 32) throw new SyncError("malformed", "seeds are 32 bytes");
    return new RawIdentity(Uint8Array.from(edSeed), Uint8Array.from(xSeed));
  }

  static generate(): RawIdentity {
    return new RawIdentity(randomBytes(32), randomBytes(32));
  }

  /** The record a KeyStorage keeps (strings only, for keychains). */
  toStored(): StoredRawIdentity {
    return { kind: "raw", edSeed: b64url(this.edSeed), xSeed: b64url(this.xSeed) };
  }

  async sign(message: Uint8Array): Promise<Uint8Array> {
    return ed25519.sign(message, this.edSeed);
  }

  async agree(peerX: Uint8Array): Promise<Uint8Array> {
    return x25519Agree(this.xSeed, peerX);
  }
}

export interface StoredRawIdentity {
  kind: "raw";
  edSeed: string;
  xSeed: string;
}

export interface StoredWebCryptoIdentity {
  kind: "webcrypto";
  ed: CryptoKeyPair;
  x: CryptoKeyPair;
}

export type StoredIdentity = StoredRawIdentity | StoredWebCryptoIdentity;

export class WebCryptoIdentity implements DeviceIdentity {
  readonly kind = "webcrypto" as const;
  private constructor(
    readonly publicKeys: PublicKeys,
    private readonly ed: CryptoKeyPair,
    private readonly x: CryptoKeyPair,
  ) {}

  /** True when this runtime's WebCrypto has Ed25519 and X25519. */
  static async supported(): Promise<boolean> {
    try {
      await WebCryptoIdentity.generate();
      return true;
    } catch {
      return false;
    }
  }

  /** New non-extractable keys. */
  static async generate(): Promise<WebCryptoIdentity> {
    const subtle = globalThis.crypto.subtle;
    const ed = (await subtle.generateKey({ name: "Ed25519" }, false, ["sign", "verify"])) as CryptoKeyPair;
    const x = (await subtle.generateKey({ name: "X25519" }, false, ["deriveBits"])) as CryptoKeyPair;
    return WebCryptoIdentity.fromKeys(ed, x);
  }

  static async fromKeys(ed: CryptoKeyPair, x: CryptoKeyPair): Promise<WebCryptoIdentity> {
    const subtle = globalThis.crypto.subtle;
    const edPublic = new Uint8Array(await subtle.exportKey("raw", ed.publicKey));
    const xPublic = new Uint8Array(await subtle.exportKey("raw", x.publicKey));
    return new WebCryptoIdentity({ ed: edPublic, x: xPublic }, ed, x);
  }

  toStored(): StoredWebCryptoIdentity {
    return { kind: "webcrypto", ed: this.ed, x: this.x };
  }

  /** Whether the private keys can be read out (they never should). */
  get extractable(): boolean {
    return this.ed.privateKey.extractable || this.x.privateKey.extractable;
  }

  async sign(message: Uint8Array): Promise<Uint8Array> {
    const signature = await globalThis.crypto.subtle.sign({ name: "Ed25519" }, this.ed.privateKey, message as BufferSource);
    return new Uint8Array(signature);
  }

  async agree(peerX: Uint8Array): Promise<Uint8Array> {
    const subtle = globalThis.crypto.subtle;
    let shared: Uint8Array;
    try {
      const peer = await subtle.importKey("raw", peerX as BufferSource, { name: "X25519" }, true, []);
      shared = new Uint8Array(
        await subtle.deriveBits({ name: "X25519", public: peer } as unknown as AlgorithmIdentifier, this.x.privateKey, 256),
      );
    } catch {
      throw new SyncError("crypto", "low-order key");
    }
    if (isZero(shared)) throw new SyncError("crypto", "low-order key");
    return shared;
  }
}

/**
 * Where a device keeps its identity and small protocol state. Browsers
 * use IndexedDbKeyStorage (CryptoKeys are stored as is, still
 * non-extractable); mobile apps pass an adapter over the OS keychain that
 * stores JSON strings (`storesCryptoKeys: false`).
 */
export interface KeyStorage {
  /** Whether values may be CryptoKey objects (structured clone). */
  readonly storesCryptoKeys: boolean;
  get(name: string): Promise<unknown>;
  set(name: string, value: unknown): Promise<void>;
  delete(name: string): Promise<void>;
}

export class MemoryKeyStorage implements KeyStorage {
  readonly storesCryptoKeys = true;
  private readonly values = new Map<string, unknown>();
  async get(name: string): Promise<unknown> {
    return this.values.get(name);
  }
  async set(name: string, value: unknown): Promise<void> {
    this.values.set(name, value);
  }
  async delete(name: string): Promise<void> {
    this.values.delete(name);
  }
}

/** IndexedDB, one object store; CryptoKeys survive as non-extractable. */
export class IndexedDbKeyStorage implements KeyStorage {
  readonly storesCryptoKeys = true;
  constructor(
    private readonly database = "hame-sync",
    private readonly store = "keys",
  ) {}

  private open(): Promise<IDBDatabase> {
    return new Promise((resolve, reject) => {
      const request = indexedDB.open(this.database, 1);
      request.onupgradeneeded = () => request.result.createObjectStore(this.store);
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error);
    });
  }

  private async run<T>(mode: IDBTransactionMode, work: (store: IDBObjectStore) => IDBRequest): Promise<T> {
    const db = await this.open();
    try {
      return await new Promise<T>((resolve, reject) => {
        const request = work(db.transaction(this.store, mode).objectStore(this.store));
        request.onsuccess = () => resolve(request.result as T);
        request.onerror = () => reject(request.error);
      });
    } finally {
      db.close();
    }
  }

  get(name: string): Promise<unknown> {
    return this.run("readonly", (store) => store.get(name));
  }
  async set(name: string, value: unknown): Promise<void> {
    await this.run("readwrite", (store) => store.put(value, name));
  }
  async delete(name: string): Promise<void> {
    await this.run("readwrite", (store) => store.delete(name));
  }
}

/** Restores an identity from a stored record. */
export async function restoreIdentity(stored: unknown): Promise<DeviceIdentity> {
  const record = (typeof stored === "string" ? JSON.parse(stored) : stored) as StoredIdentity;
  if (record?.kind === "raw") {
    return RawIdentity.fromSeeds(fromB64url32(record.edSeed), fromB64url32(record.xSeed));
  }
  if (record?.kind === "webcrypto") {
    return WebCryptoIdentity.fromKeys(record.ed, record.x);
  }
  throw new SyncError("malformed", "unknown stored identity");
}

/**
 * The device's identity under `name`, created on first use: non-extractable
 * WebCrypto keys when the runtime and the storage support them. Raw seeds
 * (readable by code in the app) only with `allowRawKeys: true`, which a
 * mobile app storing them in the OS keychain passes; the identity's `kind`
 * tells the UI which it got.
 */
export async function loadOrCreateIdentity(
  storage: KeyStorage,
  options: { name?: string; allowRawKeys?: boolean } = {},
): Promise<DeviceIdentity> {
  const name = options.name ?? "identity";
  const stored = await storage.get(name);
  if (stored !== undefined && stored !== null) {
    const identity = await restoreIdentity(stored);
    if (identity.kind === "raw" && !options.allowRawKeys) {
      throw new SyncError("crypto", "the stored identity has raw keys; pass allowRawKeys to use it");
    }
    return identity;
  }
  if (storage.storesCryptoKeys && (await WebCryptoIdentity.supported())) {
    const identity = await WebCryptoIdentity.generate();
    await storage.set(name, identity.toStored());
    return identity;
  }
  if (!options.allowRawKeys) {
    throw new SyncError(
      "crypto",
      storage.storesCryptoKeys
        ? "this runtime's WebCrypto lacks Ed25519/X25519; raw keys need allowRawKeys"
        : "this storage cannot hold CryptoKeys; raw keys need allowRawKeys",
    );
  }
  const identity = RawIdentity.generate();
  await storage.set(name, storage.storesCryptoKeys ? identity.toStored() : JSON.stringify(identity.toStored()));
  return identity;
}
