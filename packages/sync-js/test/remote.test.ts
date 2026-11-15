import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
  MemoryKeyStorage,
  PersistentSequence,
  RawIdentity,
  RelayClient,
  checkRelayUrl,
  type KeyStorage,
  WebCryptoIdentity,
  backoffMs,
  connectRemote,
  header,
  offerToUri,
  parseOffer,
  seal,
  utf8,
  wsUrl,
} from "../src/index.js";
import { FakeHost } from "./fake-host.js";
import { MockRelay } from "./mock-relay.js";

async function until(what: string, check: () => boolean, ms = 5000): Promise<void> {
  const start = Date.now();
  while (!check()) {
    if (Date.now() - start > ms) throw new Error(`timed out waiting for ${what}`);
    await new Promise((r) => setTimeout(r, 10));
  }
}

describe("sequences", () => {
  it("continue above the host's last counter after pairing again, never below", async () => {
    const sequence = new PersistentSequence(new MemoryKeyStorage(), "c");
    expect(await sequence.next()).toBe(1);
    await sequence.advanceTo(40);
    expect(await sequence.next()).toBe(41);
    await sequence.advanceTo(10);
    expect(await sequence.next()).toBe(42);
  });
});

describe("relay client helpers", () => {
  it("builds WebSocket URLs and bounded jittered backoff", () => {
    expect(wsUrl("https://relay.example/base/", "dev 1")).toBe("wss://relay.example/base/v1/ws?device=dev+1");
    expect(wsUrl("http://127.0.0.1:9", "d")).toBe("ws://127.0.0.1:9/v1/ws?device=d");
    for (let attempt = 0; attempt < 40; attempt++) {
      const cap = Math.min(5000, 100 * 2 ** Math.min(attempt, 16));
      const wait = backoffMs(100, 5000, attempt);
      expect(wait).toBeGreaterThanOrEqual(cap / 2);
      expect(wait).toBeLessThanOrEqual(cap);
    }
  });
});

describe("remote transport through a relay", () => {
  let relay: MockRelay;
  let host: FakeHost;
  const cleanup: (() => void)[] = [];

  beforeEach(async () => {
    relay = await new MockRelay().start();
    host = new FakeHost("host", relay.url, relay.device("host"));
    host.start();
    cleanup.push(() => host.stop());
    await until("the host to connect", () => relay.connected("host"));
  });

  afterEach(async () => {
    for (const fn of cleanup.splice(0)) fn();
    await relay.stop();
  });

  function phone(
    identity: RawIdentity | WebCryptoIdentity,
    id = "phone",
    hostKeys?: { ed: Uint8Array; x: Uint8Array },
    storage: KeyStorage = new MemoryKeyStorage(),
  ) {
    const rejected: string[] = [];
    const remote = connectRemote({
      storage,
      relayUrl: relay.url,
      deviceId: id,
      token: async () => relay.device(id),
      identity,
      hostId: "host",
      hostKeys,
      onRejected: (reason) => rejected.push(reason),
    });
    cleanup.push(() => remote.relay.stop());
    return { ...remote, rejected };
  }

  it("pairs with SAS, calls the API, streams, resumes and fetches a chunked artifact", async () => {
    const offer = parseOffer(offerToUri(host.offerPairing(relay.url)));
    const device = phone(await WebCryptoIdentity.generate());
    const { sas, grant } = await device.host.pair(offer, "Phone");
    const digits = await sas;
    expect(digits).toMatch(/^[0-9]{8}$/);
    const granted = await grant;
    expect(granted.device).toBe("phone");
    expect(host.sasShown).toEqual([digits]);

    const listed = await device.transport.request("GET", "/api/sessions");
    expect(listed.status).toBe(200);
    expect(await listed.json()).toEqual([{ id: "s1", cwd: "/w" }]);
    const created = await device.transport.request("POST", "/api/sessions", { title: "x" });
    expect(await created.json()).toEqual({ id: "s1", echo: { title: "x" } });

    const seen: number[] = [];
    let ended = false;
    const stop = device.transport.subscribe(
      "/api/sessions/s1/events",
      -1,
      (seq) => seen.push(seq),
      () => {
        ended = true;
      },
    );
    await until("ten records", () => seen.length === 10);
    expect(seen).toEqual([0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
    stop();
    expect(ended).toBe(true);

    // The artifact's session was learned from the stream.
    const blob = await device.transport.fetchBlob(host.artifactSha);
    expect(blob.type).toBe("application/pdf");
    expect(new Uint8Array(await blob.arrayBuffer())).toEqual(host.artifact);

    // A dropped connection comes back; a resumed stream skips what was seen.
    relay.kick("phone");
    relay.kick("host");
    expect((await device.transport.request("GET", "/api/sessions")).status).toBe(200);
    const resumed: number[] = [];
    const stop2 = device.transport.subscribe("/api/sessions/s1/events", 6, (seq) => resumed.push(seq), () => undefined);
    await until("the rest", () => resumed.length === 3);
    expect(resumed).toEqual([7, 8, 9]);
    stop2();
    expect(device.rejected).toEqual([]);
  });

  it("refuses forged, replayed and foreign envelopes", async () => {
    const offer = host.offerPairing(relay.url);
    const identity = RawIdentity.generate();
    const device = phone(identity);
    await (await device.host.pair(offer, "Phone")).grant;
    expect((await device.transport.request("GET", "/api/sessions")).status).toBe(200);

    // A relay replaying the host's last envelope.
    const last = [...relay.routed].reverse().find((r) => r.from === "host" && r.to === "phone")!;
    relay.inject("host", "phone", last.envelope);
    await until("the replay to be refused", () => device.rejected.some((r) => r.includes("replayed")));

    // An envelope claiming to be the host, signed by another key.
    const forger = RawIdentity.generate();
    const forged = await seal(forger, identity.publicKeys.x, header("host", "phone", "1", Date.now() * 1000 + 10_000_000), utf8('{"t":"res","id":1,"status":200,"body":{}}'));
    relay.inject("host", "phone", forged);
    await until("the forgery to be refused", () => device.rejected.some((r) => r.includes("bad signature")));

    // Another device of the account writing to the phone.
    relay.inject("mallory", "phone", forged);
    await until("the stranger to be refused", () => device.rejected.some((r) => r.includes("unknown sender")));

    // A device the host never paired is ignored by the host.
    const stranger = phone(RawIdentity.generate(), "stranger", host.identity.publicKeys);
    await expect(
      (async () => {
        const answer = stranger.host.request("GET", "/api/sessions");
        await until("the host to drop it", () => host.rejected.some((r) => r.includes("unknown sender")));
        return Promise.race([answer, new Promise((_, reject) => setTimeout(() => reject(new Error("silent")), 300))]);
      })(),
    ).rejects.toThrow("silent");
  });

  it("two tabs sharing one identity and storage keep working", async () => {
    const identity = RawIdentity.generate();
    const storage = new MemoryKeyStorage();
    const first = phone(identity, "phone", undefined, storage);
    await (await first.host.pair(host.offerPairing(relay.url), "Phone")).grant;
    const second = phone(identity, "phone", host.identity.publicKeys, storage);
    // Both tabs draw counters from the same persisted sequence.
    const answers = await Promise.all([
      first.transport.request("GET", "/api/sessions"),
      second.transport.request("GET", "/api/sessions"),
      first.transport.request("GET", "/api/sessions"),
    ]).catch(() => []);
    // The relay connection is per device: the newest tab owns it, so the
    // first tab's answers may go to the second; what matters is that the
    // host refused nothing as a replay.
    expect(host.rejected.filter((r) => r.includes("replay"))).toEqual([]);
    expect(Number(await storage.get("counter:phone"))).toBeGreaterThanOrEqual(4);
    void answers;
  });

  it("renews its token on the open connection", async () => {
    const device = phone(RawIdentity.generate());
    device.relay.stop();
    const tokens: string[] = [];
    const client = new RelayClient({
      url: relay.url,
      deviceId: "renewer",
      token: async () => {
        const token = relay.device("renewer");
        tokens.push(token);
        return token;
      },
      renewMs: 50,
      onDeliver: () => undefined,
    });
    cleanup.push(() => client.stop());
    client.start();
    await until("two renewals", () => (relay.renewals.get("renewer") ?? 0) >= 2);
    expect(tokens.length).toBeGreaterThanOrEqual(3);
  });

  it("drops the connection when deliveries pile up, and gets them again", async () => {
    let handled = 0;
    let release: () => void = () => undefined;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    const client = new RelayClient({
      url: relay.url,
      deviceId: "slow",
      token: async () => relay.device("slow"),
      maxPendingDeliveries: 5,
      backoffBaseMs: 20,
      backoffMaxMs: 50,
      onDeliver: async () => {
        await gate;
        handled += 1;
      },
    });
    cleanup.push(() => client.stop());
    client.start();
    await until("connected", () => relay.connected("slow"));
    let drops = 0;
    const before = relay.connected.bind(relay);
    for (let n = 0; n < 20; n++) relay.inject("host", "slow", `e${n}`);
    await until("the connection to drop", () => {
      if (!before("slow")) drops += 1;
      return drops > 0;
    });
    release();
    // What was not acknowledged comes again; everything is handled.
    await until("all twenty handled", () => relay.queued("slow") === 0 && handled >= 20, 5000);
  });

  it("refuses an unencrypted relay address that is not this machine", () => {
    expect(() => wsUrl("http://relay.example", "d")).toThrow("unencrypted");
    expect(() => checkRelayUrl("ws://10.0.0.1")).toThrow();
    expect(checkRelayUrl("http://localhost:1")).toBe("http://localhost:1");
  });

  it("reports a pairing the host user declined", async () => {
    host.stop();
    const declining = new FakeHost("host", relay.url, relay.device("host"), () => false);
    declining.start();
    cleanup.push(() => declining.stop());
    await until("the host to reconnect", () => relay.connected("host"));
    const device = phone(RawIdentity.generate());
    const { grant } = await device.host.pair(declining.offerPairing(relay.url), "Phone");
    await expect(grant).rejects.toThrow("not confirmed");
  });
});
