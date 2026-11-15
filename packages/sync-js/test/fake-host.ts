// A minimal host written with this library, for testing the device side
// without Rust: allowlist, pairing with SAS confirmation, a few API routes,
// an event stream and chunked artifacts. The real host is
// crates/hame-sync's connector.

import { sha256 } from "@noble/hashes/sha2.js";
import { Counter, ReplayWindow, Sealed, header, seal } from "../src/envelope.js";
import { b64, b64url, concat, fromB64url, fromB64url32, fromUtf8, hex, utf8 } from "../src/encoding.js";
import { type Frame, streamOf } from "../src/frames.js";
import { type PublicKeys, RawIdentity } from "../src/keys.js";
import { GRANT_CONTEXT, type PairingOffer, checkPairingProof, sas } from "../src/pairing.js";
import { RelayClient } from "../src/relay.js";

interface Device {
  keys: PublicKeys;
  window: ReplayWindow;
}

export class FakeHost {
  readonly identity = RawIdentity.generate();
  readonly devices = new Map<string, Device>();
  readonly rejected: string[] = [];
  readonly sasShown: string[] = [];
  private offer: PairingOffer | undefined;
  private readonly counter = new Counter();
  private readonly streams = new Map<string, ReturnType<typeof setInterval>>();
  private sending: Promise<void> = Promise.resolve();
  readonly relay: RelayClient;
  /** Blob served at /api/artifacts/<sha>. */
  readonly artifact = Uint8Array.from({ length: 600_000 }, (_, n) => (n * 31) % 251);
  readonly artifactSha = hex(sha256(this.artifact));

  constructor(
    readonly id: string,
    relayUrl: string,
    token: string,
    private readonly confirm: (sas: string) => boolean = () => true,
  ) {
    this.relay = new RelayClient({
      url: relayUrl,
      deviceId: id,
      token: async () => token,
      backoffBaseMs: 20,
      backoffMaxMs: 100,
      onDeliver: (from, envelope) => this.deliver(from, envelope).catch((e: Error) => void this.rejected.push(e.message)),
    });
  }

  start(): void {
    this.relay.start();
  }

  stop(): void {
    for (const timer of this.streams.values()) clearInterval(timer);
    this.relay.stop();
  }

  offerPairing(relay: string): PairingOffer {
    this.offer = {
      relay,
      host: this.id,
      hostKeys: this.identity.publicKeys,
      secret: crypto.getRandomValues(new Uint8Array(32)),
      expires: Math.floor(Date.now() / 1000) + 600,
    };
    return this.offer;
  }

  private send(to: string, x: Uint8Array, frame: Frame): Promise<void> {
    const run = this.sending.then(async () => {
      const envelope = await seal(this.identity, x, header(this.id, to, streamOf(frame), this.counter.next()), utf8(JSON.stringify(frame)));
      this.relay.send(to, envelope);
    });
    this.sending = run.catch(() => undefined);
    return run;
  }

  private async deliver(from: string, envelope: string): Promise<void> {
    const sealed = Sealed.parse(envelope);
    const device = this.devices.get(sealed.header.from);
    if (sealed.header.from !== from) throw new Error("sender mismatch");
    if (!device) {
      const offer = this.offer;
      if (!offer) throw new Error(`unknown sender ${from}`);
      const frame = JSON.parse(fromUtf8(await sealed.decrypt(this.identity))) as Frame;
      if (frame.t !== "pair") throw new Error(`unknown sender ${from}`);
      const keys = { ed: fromB64url32(frame.ed), x: fromB64url32(frame.x) };
      if (!sealed.verify(keys.ed)) throw new Error("bad signature");
      if (!checkPairingProof(offer.secret, keys, fromB64url(frame.mac))) throw new Error("bad proof");
      this.offer = undefined;
      const nonce = crypto.getRandomValues(new Uint8Array(32));
      await this.send(from, keys.x, { t: "pair_challenge", nonce: b64url(nonce) });
      const code = sas(offer.secret, this.identity.publicKeys.ed, keys.ed, nonce);
      this.sasShown.push(code);
      if (!this.confirm(code)) {
        await this.send(from, keys.x, { t: "pair_refused", reason: "not confirmed on the host" });
        return;
      }
      this.devices.set(from, { keys, window: ReplayWindow.startingAt(sealed.header.counter) });
      const grant = utf8(
        JSON.stringify({ v: 1, host: this.id, device: from, ed: frame.ed, x: frame.x, label: frame.label, issued: 1 }),
      );
      const sig = await this.identity.sign(concat(GRANT_CONTEXT, grant));
      await this.send(from, keys.x, { t: "paired", grant: b64url(grant), sig: b64url(sig) });
      return;
    }
    const plain = await sealed.open(this.identity, this.id, device.keys, device.window);
    device.window.record(sealed.header.counter);
    const frame = JSON.parse(fromUtf8(plain)) as Frame;
    const reply = (f: Frame) => this.send(from, device.keys.x, f);
    switch (frame.t) {
      case "req": {
        if (frame.path === "/api/sessions" && frame.method === "GET") {
          await reply({ t: "res", id: frame.id, status: 200, body: [{ id: "s1", cwd: "/w" }] });
        } else if (frame.path === "/api/sessions" && frame.method === "POST") {
          await reply({ t: "res", id: frame.id, status: 200, body: { id: "s1", echo: frame.body } });
        } else if (frame.path === `/api/artifacts/${this.artifactSha}?session=s1`) {
          const chunks: Uint8Array[] = [];
          for (let at = 0; at < this.artifact.length; at += 256 * 1024) chunks.push(this.artifact.subarray(at, at + 256 * 1024));
          await reply({
            t: "res",
            id: frame.id,
            status: 200,
            body: { blob: { sha256: this.artifactSha, mime: "application/pdf", size: this.artifact.length, chunks: chunks.length } },
          });
          for (const [n, chunk] of chunks.entries()) {
            await reply({ t: "blob", id: frame.id, sha256: this.artifactSha, chunk: n, of: chunks.length, data: b64(chunk) });
          }
        } else {
          await reply({ t: "res", id: frame.id, status: 404, body: "not found" });
        }
        return;
      }
      case "sub": {
        // Records 0..9, one every 10 ms, then the stream stays open.
        const key = `${from}/${frame.id}`;
        clearInterval(this.streams.get(key));
        let seq = frame.after + 1;
        const timer = setInterval(() => {
          if (seq > 9) {
            clearInterval(timer);
            return;
          }
          const record = { kind: "record", record: { seq, type: "assistant/delta", artifact: { sha256: this.artifactSha } } };
          void reply({ t: "evt", id: frame.id, seq, data: record });
          seq += 1;
        }, 10);
        this.streams.set(key, timer);
        return;
      }
      case "cancel": {
        const key = `${from}/${frame.id}`;
        clearInterval(this.streams.get(key));
        this.streams.delete(key);
        await reply({ t: "end", id: frame.id, error: "cancelled" });
        return;
      }
      default:
        return;
    }
  }
}
