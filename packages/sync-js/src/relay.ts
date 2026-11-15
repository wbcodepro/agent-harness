// A connection to the relay WebSocket (/v1/ws) that stays up: `resume` with
// the last acknowledged seq as the first frame, `ack` after each delivery is
// handled, `ping` every 25 s, an `auth` frame with a fresh token before the
// access token expires, reconnection with jittered exponential backoff when
// the socket fails, goes quiet or is closed as `unauthorized`, and a queue
// for frames sent while disconnected. A `send` stays unconfirmed until the
// relay answers it with `sent` (or an error naming the same recipient);
// unconfirmed sends are written again after a reconnection, since a socket
// can die with them in flight. A duplicate that reaches the recipient is
// refused there as a replay. Unconfirmed sends are bounded in count and bytes.

import { SyncError } from "./errors.js";
import type { RelayFrame } from "./frames.js";

/** The subset of the WebSocket API used here (browser, Node 22+, React Native). */
export interface SocketLike {
  readonly readyState: number;
  send(data: string): void;
  close(code?: number, reason?: string): void;
  onopen: ((event: unknown) => void) | null;
  onclose: ((event: unknown) => void) | null;
  onerror: ((event: unknown) => void) | null;
  onmessage: ((event: { data: unknown }) => void) | null;
}

export type SocketFactory = (url: string, protocols: string[]) => SocketLike;

export interface RelayClientOptions {
  /** The relay's base address, http(s)://host[:port][/prefix]. */
  url: string;
  deviceId: string;
  /** The device's bearer token, asked before every connection attempt. */
  token: () => Promise<string>;
  /** Handles one envelope; the delivery is acknowledged when it settles. */
  onDeliver: (from: string, envelope: string, seq: number) => Promise<void> | void;
  onPresence?: (device: string, online: boolean) => void;
  onError?: (code: string, message: string) => void;
  onState?: (connected: boolean) => void;
  socket?: SocketFactory;
  pingMs?: number;
  silenceMs?: number;
  backoffBaseMs?: number;
  backoffMaxMs?: number;
  /** Send a fresh token this often (default 10 minutes; tokens live 15). */
  renewMs?: number;
}

/**
 * Refuses relay addresses that are not encrypted, except on this machine:
 * https/wss anywhere, http/ws only for localhost, 127.0.0.1 and [::1].
 */
export function checkRelayUrl(base: string): string {
  const url = new URL(base);
  if (url.protocol === "https:" || url.protocol === "wss:") return base;
  if (url.protocol === "http:" || url.protocol === "ws:") {
    if (["localhost", "127.0.0.1", "[::1]"].includes(url.hostname)) return base;
    throw new SyncError("relay", "an unencrypted relay address is only allowed on this machine; use https");
  }
  throw new SyncError("relay", `unsupported scheme ${url.protocol}`);
}

/** Full jitter: a random wait between half and all of min(max, base · 2^attempt). */
export function backoffMs(baseMs: number, maxMs: number, attempt: number): number {
  const cap = Math.min(maxMs, baseMs * 2 ** Math.min(attempt, 16));
  return cap / 2 + Math.random() * (cap / 2);
}

export function wsUrl(base: string, deviceId: string): string {
  const url = new URL(checkRelayUrl(base));
  if (url.protocol === "http:") url.protocol = "ws:";
  else if (url.protocol === "https:") url.protocol = "wss:";
  else if (url.protocol !== "ws:" && url.protocol !== "wss:") throw new Error(`unsupported scheme ${url.protocol}`);
  url.pathname = `${url.pathname.replace(/\/+$/, "")}/v1/ws`;
  url.search = "";
  url.searchParams.set("device", deviceId);
  return url.toString();
}

/** The relay's WebSocket subprotocol. */
export const RELAY_PROTOCOL = "hame-relay.v1";
const OPEN = 1;
const MAX_UNCONFIRMED = 1024;
const MAX_UNCONFIRMED_BYTES = 16 * 1024 * 1024;

/** [to, envelope, push] of a queued send frame; to is "" for other frames. */
function unpackSend(text: string): [string, string, boolean] {
  const frame = JSON.parse(text) as RelayFrame;
  return frame.op === "send" ? [frame.to, frame.envelope, frame.push === true] : ["", "", false];
}

export class RelayClient {
  private socket: SocketLike | undefined;
  private lastAcked = 0;
  private attempt = 0;
  private stopped = false;
  private connected = false;
  private readonly queue: string[] = [];
  /** Sends written but not yet answered by `sent` or `error`, oldest first. */
  private readonly unconfirmed: string[] = [];
  private pingTimer: ReturnType<typeof setInterval> | undefined;
  private renewTimer: ReturnType<typeof setInterval> | undefined;
  private unconfirmedBytes = 0;
  private retryTimer: ReturnType<typeof setTimeout> | undefined;
  private lastHeard = 0;
  /** Deliveries are handled one at a time, in seq order. */
  private chain: Promise<void> = Promise.resolve();

  constructor(private readonly options: RelayClientOptions) {}

  get isConnected(): boolean {
    return this.connected;
  }

  get acknowledged(): number {
    return this.lastAcked;
  }

  start(): void {
    this.stopped = false;
    void this.connect();
  }

  stop(): void {
    this.stopped = true;
    clearTimeout(this.retryTimer);
    clearInterval(this.pingTimer);
    clearInterval(this.renewTimer);
    this.socket?.close(1000, "stopped");
    this.socket = undefined;
  }

  /** Sends an envelope; queued while disconnected. */
  send(to: string, envelope: string, push = false): void {
    const frame: RelayFrame = push ? { op: "send", to, envelope, push } : { op: "send", to, envelope };
    const text = JSON.stringify(frame);
    if (this.write(text)) {
      this.unconfirmed.push(text);
      this.unconfirmedBytes += text.length;
      while (this.unconfirmed.length > MAX_UNCONFIRMED || this.unconfirmedBytes > MAX_UNCONFIRMED_BYTES) {
        this.unconfirmedBytes -= this.unconfirmed.shift()?.length ?? 0;
      }
    }
  }

  /** Drops the current connection (tests; network changes). */
  reconnect(): void {
    this.socket?.close(4000, "reconnect");
  }

  /** Writes now (true) or queues for the next connection (false). */
  private write(text: string): boolean {
    if (this.socket && this.connected && this.socket.readyState === OPEN) {
      try {
        this.socket.send(text);
        return true;
      } catch {
        // falls through to the queue
      }
    }
    this.queue.push(text);
    return false;
  }

  private async connect(): Promise<void> {
    if (this.stopped) return;
    let token: string;
    try {
      token = await this.options.token();
    } catch {
      this.scheduleRetry();
      return;
    }
    if (this.stopped) return;
    // Browsers cannot set headers on a WebSocket: the token travels as a
    // second subprotocol, and the relay selects "hame-relay.v1".
    const url = wsUrl(this.options.url, this.options.deviceId);
    const protocols = [RELAY_PROTOCOL, `bearer.${token}`];
    const factory: SocketFactory =
      this.options.socket ?? ((u, p) => new (globalThis as unknown as { WebSocket: new (u: string, p: string[]) => SocketLike }).WebSocket(u, p));
    let socket: SocketLike;
    try {
      socket = factory(url, protocols);
    } catch {
      this.scheduleRetry();
      return;
    }
    this.socket = socket;
    socket.onopen = () => {
      if (this.socket !== socket) return;
      socket.send(JSON.stringify({ op: "resume", after: this.lastAcked } satisfies RelayFrame));
      this.connected = true;
      this.lastHeard = Date.now();
      this.options.onState?.(true);
      // Unconfirmed sends first, in their order, then what waited.
      const again = this.unconfirmed.splice(0);
      this.unconfirmedBytes = 0;
      const waiting = this.queue.splice(0);
      for (const text of again) this.send(...unpackSend(text));
      for (const text of waiting) {
        const parsed = unpackSend(text);
        if (parsed[0] !== "") this.send(...parsed);
        else this.write(text);
      }
      clearInterval(this.pingTimer);
      this.pingTimer = setInterval(() => this.ping(socket), this.options.pingMs ?? 25_000);
      clearInterval(this.renewTimer);
      this.renewTimer = setInterval(() => void this.renew(socket), this.options.renewMs ?? 600_000);
    };
    socket.onmessage = (event) => {
      if (this.socket !== socket) return;
      this.lastHeard = Date.now();
      this.attempt = 0;
      const text = typeof event.data === "string" ? event.data : String(event.data);
      let frame: RelayFrame;
      try {
        frame = JSON.parse(text) as RelayFrame;
      } catch {
        return;
      }
      this.handle(frame);
    };
    socket.onerror = () => {
      // onclose follows
    };
    socket.onclose = () => {
      if (this.socket !== socket) return;
      this.socket = undefined;
      clearInterval(this.pingTimer);
      clearInterval(this.renewTimer);
      if (this.connected) {
        this.connected = false;
        this.options.onState?.(false);
      }
      this.scheduleRetry();
    };
  }

  /** Sends a fresh access token; a token that cannot be had ends the connection. */
  private async renew(socket: SocketLike): Promise<void> {
    let token: string;
    try {
      token = await this.options.token();
    } catch {
      socket.close(4002, "no token");
      return;
    }
    if (this.socket === socket && socket.readyState === OPEN) socket.send(JSON.stringify({ op: "auth", token } satisfies RelayFrame));
  }

  private confirm(to: string): void {
    const at = this.unconfirmed.findIndex((text) => unpackSend(text)[0] === to);
    if (at >= 0) this.unconfirmedBytes -= this.unconfirmed.splice(at, 1)[0]!.length;
  }

  private ping(socket: SocketLike): void {
    if (Date.now() - this.lastHeard > (this.options.silenceMs ?? 60_000)) {
      socket.close(4001, "the relay went quiet");
      return;
    }
    if (socket.readyState === OPEN) socket.send(JSON.stringify({ op: "ping" }));
  }

  private scheduleRetry(): void {
    if (this.stopped) return;
    const wait = backoffMs(this.options.backoffBaseMs ?? 500, this.options.backoffMaxMs ?? 30_000, this.attempt);
    this.attempt += 1;
    clearTimeout(this.retryTimer);
    this.retryTimer = setTimeout(() => void this.connect(), wait);
  }

  private handle(frame: RelayFrame): void {
    switch (frame.op) {
      case "deliver": {
        const { seq, from, envelope } = frame;
        this.chain = this.chain.then(async () => {
          try {
            await this.options.onDeliver(from, envelope, seq);
          } catch {
            // Refused envelopes are acknowledged too: the relay may drop them.
          }
          if (seq > this.lastAcked) this.lastAcked = seq;
          this.write(JSON.stringify({ op: "ack", seq } satisfies RelayFrame));
        });
        return;
      }
      case "sent":
        this.confirm(frame.to);
        return;
      case "ping":
        this.write(JSON.stringify({ op: "pong" }));
        return;
      case "presence":
        this.options.onPresence?.(frame.device, frame.online);
        return;
      case "error":
        // The relay answers a send it refused with an error naming its
        // recipient instead of sent.
        if (frame.to !== undefined) this.confirm(frame.to);
        this.options.onError?.(frame.code, frame.message ?? "");
        // The relay closes the socket after this; reconnect with a fresh token.
        if (frame.code === "unauthorized") this.socket?.close(4003, "unauthorized");
        return;
      default:
        return;
    }
  }
}
