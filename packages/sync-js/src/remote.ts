// The device side of the remote API: pairs with a host, sends API calls
// and subscriptions in sealed envelopes, verifies and decrypts everything
// the host sends back, and offers the UI's Transport interface.

import { sha256 } from "@noble/hashes/sha2.js";
import { b64url, fromB64, fromB64url32, fromUtf8, hex, utf8 } from "./encoding.js";
import { Counter, ReplayWindow, Sealed, header, seal } from "./envelope.js";
import { SyncError } from "./errors.js";
import { type Frame, streamOf } from "./frames.js";
import { argsSha256 } from "./jcs.js";
import type { DeviceIdentity, KeyStorage, PublicKeys } from "./keys.js";
import { checkRelayUrl } from "./relay.js";
import { type Grant, type PairingOffer, offerExpired, pairingProof, sas, verifyGrant } from "./pairing.js";

/** The interface the UI speaks, locally (HTTP + SSE) or remotely (this). */
export interface Transport {
  request(method: string, path: string, body?: unknown): Promise<{ status: number; json(): Promise<unknown> }>;
  subscribe(
    path: string,
    afterSeq: number,
    onEvent: (seq: number, data: unknown) => void,
    onEnd: (err?: Error) => void,
  ): () => void;
  fetchBlob(sha256: string): Promise<Blob>;
}

/** Persists the last counter accepted from each host (replay protection across restarts). */
export interface CounterStore {
  get(host: string): Promise<number | undefined>;
  set(host: string, counter: number): Promise<void>;
}

export interface RemoteHostOptions {
  identity: DeviceIdentity;
  deviceId: string;
  hostId: string;
  /** The host's keys, pinned at pairing; absent until paired. */
  hostKeys?: PublicKeys;
  /** Hands an envelope to the relay (RelayClient.send). */
  send: (to: string, envelope: string) => void;
  counters?: CounterStore;
  /**
   * This device's envelope counter. Use a PersistentSequence over the same
   * storage as the identity, so it survives reloads and is shared safely by
   * several tabs; the default counts in memory from 1 (tests).
   */
  sequence?: SequenceSource;
  /** How long a request waits for its answer. */
  timeoutMs?: number;
  /** Envelopes dropped, with the reason (for diagnostics). */
  onRejected?: (reason: string) => void;
}

export interface Response {
  status: number;
  body: unknown;
}

interface Pending {
  onFrame: (frame: Frame) => void;
}

interface Subscription {
  path: string;
  lastSeq: number;
  onEvent: (seq: number, data: unknown) => void;
  onEnd: (err?: Error) => void;
}

/** Where a device's envelope counters come from. */
export interface SequenceSource {
  next(): Promise<number>;
}

/** Counts in memory from 1. */
export class MemorySequence implements SequenceSource {
  private readonly counter = new Counter();
  async next(): Promise<number> {
    return this.counter.next();
  }
}

/**
 * A counter kept in the device's KeyStorage, incremented under a Web Lock
 * when the runtime has one (several tabs share one identity safely; the
 * host's replay window absorbs the reordering between tabs), otherwise
 * under an in-process lock.
 */
export class PersistentSequence implements SequenceSource {
  private chain: Promise<unknown> = Promise.resolve();
  constructor(
    private readonly storage: KeyStorage,
    private readonly key = "counter",
  ) {}

  next(): Promise<number> {
    const step = async () => {
      const stored = Number((await this.storage.get(this.key)) ?? 0);
      const next = (Number.isSafeInteger(stored) ? stored : 0) + 1;
      await this.storage.set(this.key, next);
      return next;
    };
    const locks = (globalThis as { navigator?: { locks?: { request(name: string, cb: () => Promise<number>): Promise<number> } } })
      .navigator?.locks;
    const run = this.chain.then(() => (locks ? locks.request(`hame-sync:${this.key}`, step) : step()));
    this.chain = run.catch(() => undefined);
    return run;
  }
}

/** A random stream id (52 bits): never reused by a restarted client. */
export function randomId(): number {
  const words = new Uint32Array(2);
  globalThis.crypto.getRandomValues(words);
  return (words[0]! & 0xfffff) * 2 ** 32 + words[1]!;
}

/**
 * The answer to an approval question as the host requires it: the
 * question's id and tool call id, and the sha256 of the arguments'
 * canonical JSON recomputed here from the arguments shown to the user. A
 * question whose stated hash does not match its arguments is refused.
 */
export function approvalAnswer(
  question: { id: number; tool_call_id: string; arguments: unknown; args_sha256?: string },
  allow: boolean,
): { id: number; allow: boolean; tool_call_id: string; args_sha256: string } {
  const hash = argsSha256(question.arguments);
  if (question.args_sha256 !== undefined && question.args_sha256 !== hash) {
    throw new SyncError("malformed", "the question's hash does not match its arguments");
  }
  return { id: question.id, allow, tool_call_id: question.tool_call_id, args_sha256: hash };
}

export class RemoteHost {
  private hostKeys: PublicKeys | undefined;
  private readonly sequence: SequenceSource;
  private hostWindow: ReplayWindow | undefined;
  private loaded = false;
  // Request ids start from the clock, so a restarted client never reuses
  // an id an old answer could be replayed for.

  private readonly pending = new Map<number, Pending>();
  private readonly subscriptions = new Map<number, Subscription>();
  private pairingWaiter: ((frame: Frame) => void) | undefined;
  private challengeWaiter: ((nonce: Uint8Array) => void) | undefined;
  /** sha256 → session id, learned from the events a subscription delivers. */
  private readonly artifactSessions = new Map<string, string>();
  /** Envelopes are opened one at a time so counters are checked in order. */
  private chain: Promise<void> = Promise.resolve();
  private sendChain: Promise<void> = Promise.resolve();

  constructor(private readonly options: RemoteHostOptions) {
    this.hostKeys = options.hostKeys;
    this.sequence = options.sequence ?? new MemorySequence();
  }

  get hostId(): string {
    return this.options.hostId;
  }

  get pinnedHostKeys(): PublicKeys | undefined {
    return this.hostKeys;
  }

  /** Feed every relay delivery here (RelayClient onDeliver). */
  handleDelivery(from: string, envelope: string): Promise<void> {
    const next = this.chain.then(() => this.accept(from, envelope));
    this.chain = next.catch(() => undefined);
    return next.catch((error: unknown) => {
      this.options.onRejected?.(error instanceof Error ? error.message : String(error));
    });
  }

  /** Re-sends live subscriptions after the host came back (relay presence). */
  resubscribe(): void {
    for (const [id, sub] of this.subscriptions) {
      void this.send({ t: "sub", id, path: sub.path, after: sub.lastSeq });
    }
  }

  private async accept(from: string, envelope: string): Promise<void> {
    const sealed = Sealed.parse(envelope);
    if (sealed.header.from !== this.options.hostId || from !== this.options.hostId) {
      throw new SyncError("unknown_sender", `unknown sender ${sealed.header.from}`);
    }
    if (!this.hostKeys) throw new SyncError("unknown_sender", "no host key pinned");
    if (!this.loaded) {
      this.loaded = true;
      const stored = await this.options.counters?.get(this.options.hostId);
      if (stored !== undefined && !this.hostWindow) this.hostWindow = ReplayWindow.startingAt(stored);
    }
    const plain = await sealed.open(this.options.identity, this.options.deviceId, this.hostKeys, this.hostWindow);
    const counter = sealed.header.counter;
    if (this.hostWindow) this.hostWindow.record(counter);
    else this.hostWindow = ReplayWindow.startingAt(counter);
    await this.options.counters?.set(this.options.hostId, this.hostWindow.last);
    const frame = JSON.parse(fromUtf8(plain)) as Frame;
    if (streamOf(frame) !== sealed.header.stream) throw new SyncError("malformed", "frame and header name different streams");
    this.route(frame);
  }

  private route(frame: Frame): void {
    switch (frame.t) {
      case "pair_challenge": {
        const waiter = this.challengeWaiter;
        this.challengeWaiter = undefined;
        waiter?.(fromB64url32(frame.nonce));
        return;
      }
      case "paired":
      case "pair_refused": {
        const waiter = this.pairingWaiter;
        this.pairingWaiter = undefined;
        waiter?.(frame);
        return;
      }
      case "evt": {
        const sub = this.subscriptions.get(frame.id);
        if (!sub) return;
        sub.lastSeq = Math.max(sub.lastSeq, frame.seq);
        const session = /^\/api\/sessions\/([^/]+)\/events/.exec(sub.path)?.[1];
        if (session) this.learnArtifacts(frame.data, session);
        sub.onEvent(frame.seq, frame.data);
        return;
      }
      case "end": {
        const sub = this.subscriptions.get(frame.id);
        if (sub) {
          this.subscriptions.delete(frame.id);
          sub.onEnd(frame.error && frame.error !== "cancelled" ? new Error(frame.error) : undefined);
        }
        this.pending.get(frame.id)?.onFrame(frame);
        return;
      }
      case "res":
      case "blob":
        this.pending.get(frame.id)?.onFrame(frame);
        return;
      default:
        return;
    }
  }

  private learnArtifacts(value: unknown, session: string, depth = 0): void {
    if (depth > 12 || value === null || typeof value !== "object") return;
    if (Array.isArray(value)) {
      for (const item of value) this.learnArtifacts(item, session, depth + 1);
      return;
    }
    for (const [key, item] of Object.entries(value)) {
      if (key === "sha256" && typeof item === "string" && /^[0-9a-f]{64}$/.test(item)) {
        this.artifactSessions.set(item, session);
      } else {
        this.learnArtifacts(item, session, depth + 1);
      }
    }
  }

  /**
   * Seals and hands over frames strictly one after another, so they reach
   * the host in counter order (it refuses a lower counter after a higher).
   */
  private send(frame: Frame): Promise<void> {
    const run = this.sendChain.then(async () => {
      const keys = this.hostKeys;
      if (!keys) throw new SyncError("pairing", "not paired with this host");
      const h = header(this.options.deviceId, this.options.hostId, streamOf(frame), await this.sequence.next());
      const envelope = await seal(this.options.identity, keys.x, h, utf8(JSON.stringify(frame)));
      this.options.send(this.options.hostId, envelope);
    });
    this.sendChain = run.catch(() => undefined);
    return run;
  }

  /**
   * Answers a pairing offer (scanned QR code). `sas` resolves to the eight
   * digits to show once the host's nonce arrived (the host shows the same);
   * `grant` to the host's verified grant.
   */
  async pair(offer: PairingOffer, label: string): Promise<{ sas: Promise<string>; grant: Promise<Grant> }> {
    checkRelayUrl(offer.relay);
    if (offerExpired(offer)) throw new SyncError("pairing", "the pairing code has expired");
    if (offer.host !== this.options.hostId) throw new SyncError("pairing", "the code is for another host");
    this.hostKeys = offer.hostKeys;
    const me = this.options.identity.publicKeys;
    const answer = new Promise<Frame>((resolve) => {
      this.pairingWaiter = resolve;
    });
    const challenge = new Promise<Uint8Array>((resolve) => {
      this.challengeWaiter = resolve;
    });
    await this.send({ t: "pair", ed: b64url(me.ed), x: b64url(me.x), label, mac: b64url(pairingProof(offer.secret, me)) });
    const grant = answer.then((frame) => {
      if (frame.t === "pair_refused") throw new SyncError("pairing", frame.reason);
      if (frame.t !== "paired") throw new SyncError("pairing", "unexpected answer");
      const verified = verifyGrant({ grant: frame.grant, sig: frame.sig }, offer.hostKeys.ed);
      if (verified.device !== this.options.deviceId || verified.host !== this.options.hostId || verified.ed !== b64url(me.ed)) {
        throw new SyncError("pairing", "the grant names someone else");
      }
      return verified;
    });
    // A refusal before the challenge settles the digits with the same error.
    const digits = Promise.race([
      challenge.then((nonce) => sas(offer.secret, offer.hostKeys.ed, me.ed, nonce)),
      grant.then(() => {
        throw new SyncError("pairing", "the host sent no pairing challenge");
      }),
    ]);
    digits.catch(() => undefined);
    grant.catch(() => undefined);
    return { sas: digits, grant };
  }

  private open(id: number, onFrame: (frame: Frame) => void): () => void {
    this.pending.set(id, { onFrame });
    return () => this.pending.delete(id);
  }

  private async exchange(make: (id: number) => Frame, timeoutMs = this.options.timeoutMs ?? 30_000): Promise<Response> {
    const id = randomId();
    let close: () => void = () => undefined;
    const answer = new Promise<Response>((resolve, reject) => {
      const timer = setTimeout(() => {
        close();
        reject(new SyncError("timeout", "no answer from the host"));
      }, timeoutMs);
      close = this.open(id, (frame) => {
        if (frame.t === "res") {
          clearTimeout(timer);
          close();
          resolve({ status: frame.status, body: frame.body });
        }
      });
    });
    try {
      await this.send(make(id));
    } catch (error) {
      close();
      throw error;
    }
    return answer;
  }

  /** Calls the host's API. */
  request(method: string, path: string, body?: unknown): Promise<Response> {
    return this.exchange((id) => (body === undefined ? { t: "req", id, method, path } : { t: "req", id, method, path, body }));
  }

  devices(): Promise<Response> {
    return this.exchange((id) => ({ t: "devices", id }));
  }

  revoke(device: string): Promise<Response> {
    return this.exchange((id) => ({ t: "revoke", id, device }));
  }

  /** Subscribes to an event stream; records up to `after` are skipped (-1: all). */
  subscribe(path: string, after: number, onEvent: (seq: number, data: unknown) => void, onEnd: (err?: Error) => void): () => void {
    const id = randomId();
    this.subscriptions.set(id, { path, lastSeq: after, onEvent, onEnd });
    this.send({ t: "sub", id, path, after }).catch((error: unknown) => {
      this.subscriptions.delete(id);
      onEnd(error instanceof Error ? error : new Error(String(error)));
    });
    return () => {
      if (this.subscriptions.delete(id)) {
        onEnd();
        void this.send({ t: "cancel", id }).catch(() => undefined);
      }
    };
  }

  /** Fetches an artifact (of `session`, or the session it was seen in) and checks its hash. */
  async fetchBlob(digest: string, session?: string): Promise<{ mime: string; bytes: Uint8Array }> {
    const owner = session ?? this.artifactSessions.get(digest);
    if (!owner) throw new SyncError("malformed", "no session refers to this artifact yet");
    const id = randomId();
    const timeoutMs = this.options.timeoutMs ?? 60_000;
    let close: () => void = () => undefined;
    const result = new Promise<{ mime: string; bytes: Uint8Array }>((resolve, reject) => {
      let chunks: (Uint8Array | undefined)[] = [];
      let have = 0;
      let mime = "";
      let timer = setTimeout(() => fail(new SyncError("timeout", "the artifact stopped arriving")), timeoutMs);
      const fail = (error: Error) => {
        clearTimeout(timer);
        close();
        reject(error);
      };
      const finish = () => {
        clearTimeout(timer);
        close();
        const total = chunks.reduce((n, c) => n + (c?.length ?? 0), 0);
        const bytes = new Uint8Array(total);
        let at = 0;
        for (const c of chunks) {
          bytes.set(c!, at);
          at += c!.length;
        }
        if (hex(sha256(bytes)) !== digest) {
          reject(new SyncError("crypto", "the artifact does not match its hash"));
          return;
        }
        resolve({ mime, bytes });
      };
      close = this.open(id, (frame) => {
        clearTimeout(timer);
        timer = setTimeout(() => fail(new SyncError("timeout", "the artifact stopped arriving")), timeoutMs);
        if (frame.t === "res") {
          const meta = (frame.body as { blob?: { sha256: string; mime: string; chunks: number } } | null)?.blob;
          if (frame.status !== 200 || !meta || meta.sha256 !== digest) {
            fail(new SyncError("relay", `the host answered ${frame.status}: ${JSON.stringify(frame.body)}`));
            return;
          }
          mime = meta.mime;
          chunks = new Array(meta.chunks).fill(undefined);
          if (meta.chunks === 0) finish();
        } else if (frame.t === "blob") {
          if (frame.sha256 !== digest || frame.chunk >= chunks.length || chunks[frame.chunk]) return;
          chunks[frame.chunk] = fromB64(frame.data);
          have += 1;
          if (have === chunks.length) finish();
        }
      });
    });
    try {
      await this.send({ t: "req", id, method: "GET", path: `/api/artifacts/${digest}?session=${encodeURIComponent(owner)}` });
    } catch (error) {
      close();
      throw error;
    }
    return result;
  }
}

/** The UI's Transport over a paired host. */
export class RemoteTransport implements Transport {
  constructor(private readonly host: RemoteHost) {}

  async request(method: string, path: string, body?: unknown): Promise<{ status: number; json(): Promise<unknown> }> {
    const response = await this.host.request(method, path, body);
    return { status: response.status, json: async () => response.body };
  }

  subscribe(path: string, afterSeq: number, onEvent: (seq: number, data: unknown) => void, onEnd: (err?: Error) => void): () => void {
    return this.host.subscribe(path, afterSeq, onEvent, onEnd);
  }

  async fetchBlob(digest: string, session?: string): Promise<Blob> {
    const { mime, bytes } = await this.host.fetchBlob(digest, session);
    return new Blob([bytes as BlobPart], { type: mime });
  }
}
