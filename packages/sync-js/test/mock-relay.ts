// A small relay for tests, with the frames of the design's §3 (the same
// behaviour as crates/hame-sync's mock relay): mailboxes with a per-recipient
// seq, resume first, ack, sent, presence, ping/pong, 1 MiB envelopes, and
// controls to drop connections or inject envelopes.

import { createServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { WebSocketServer, type WebSocket } from "ws";
import type { RelayFrame } from "../src/frames.js";

interface Queued {
  seq: number;
  from: string;
  envelope: string;
}

export class MockRelay {
  readonly tokens = new Map<string, string>(); // token → device
  private readonly queues = new Map<string, Queued[]>();
  private readonly nextSeq = new Map<string, number>();
  private readonly sockets = new Map<string, WebSocket>();
  readonly routed: { from: string; to: string; envelope: string }[] = [];
  readonly renewals = new Map<string, number>();
  private server!: Server;
  private wss!: WebSocketServer;
  url = "";

  async start(): Promise<this> {
    this.server = createServer((_, res) => {
      res.statusCode = 404;
      res.end();
    });
    this.wss = new WebSocketServer({
      noServer: true,
      handleProtocols: (protocols) => (protocols.has("hame-relay.v1") ? "hame-relay.v1" : false),
    });
    this.server.on("upgrade", (request, socket, head) => {
      const url = new URL(request.url ?? "/", "http://x");
      const device = url.searchParams.get("device") ?? "";
      const offered = String(request.headers["sec-websocket-protocol"] ?? "")
        .split(",")
        .map((p) => p.trim());
      const token = offered.find((p) => p.startsWith("bearer."))?.slice(7) ?? "";
      if (url.pathname !== "/v1/ws" || this.tokens.get(token) !== device) {
        socket.write("HTTP/1.1 401 Unauthorized\r\n\r\n");
        socket.destroy();
        return;
      }
      this.wss.handleUpgrade(request, socket, head, (ws) => this.connection(device, ws));
    });
    await new Promise<void>((resolve) => this.server.listen(0, "127.0.0.1", resolve));
    this.url = `http://127.0.0.1:${(this.server.address() as AddressInfo).port}`;
    return this;
  }

  async stop(): Promise<void> {
    for (const socket of this.sockets.values()) socket.terminate();
    this.wss.close();
    await new Promise<void>((resolve) => this.server.close(() => resolve()));
  }

  device(id: string): string {
    const token = `token-${id}-${Math.random().toString(36).slice(2)}`;
    this.tokens.set(token, id);
    return token;
  }

  kick(device: string): void {
    this.sockets.get(device)?.terminate();
  }

  connected(device: string): boolean {
    return this.sockets.has(device);
  }

  inject(from: string, to: string, envelope: string): void {
    this.enqueue(from, to, envelope);
  }

  queued(device: string): number {
    return this.queues.get(device)?.length ?? 0;
  }

  private send(socket: WebSocket, frame: RelayFrame): void {
    socket.send(JSON.stringify(frame));
  }

  private enqueue(from: string, to: string, envelope: string): number {
    this.routed.push({ from, to, envelope });
    const seq = (this.nextSeq.get(to) ?? 0) + 1;
    this.nextSeq.set(to, seq);
    const queue = this.queues.get(to) ?? [];
    queue.push({ seq, from, envelope });
    this.queues.set(to, queue);
    const socket = this.sockets.get(to);
    if (socket) this.send(socket, { op: "deliver", seq, from, envelope });
    return seq;
  }

  private presence(device: string, online: boolean): void {
    for (const [other, socket] of this.sockets) {
      if (other !== device) this.send(socket, { op: "presence", device, online });
    }
  }

  private connection(device: string, socket: WebSocket): void {
    let resumed = false;
    socket.on("message", (raw) => {
      let frame: RelayFrame;
      try {
        frame = JSON.parse(String(raw)) as RelayFrame;
      } catch {
        this.send(socket, { op: "error", code: "bad_request", message: "not a relay frame" });
        return;
      }
      if (!resumed) {
        if (frame.op !== "resume") {
          this.send(socket, { op: "error", code: "bad_request", message: "the first frame must be resume" });
          socket.close();
          return;
        }
        resumed = true;
        this.sockets.get(device)?.close();
        this.sockets.set(device, socket);
        const queue = (this.queues.get(device) ?? []).filter((q) => q.seq > frame.after);
        this.queues.set(device, queue);
        for (const q of queue) this.send(socket, { op: "deliver", ...q });
        this.presence(device, true);
        return;
      }
      switch (frame.op) {
        case "send": {
          if (frame.envelope.length > 1024 * 1024) {
            this.send(socket, { op: "error", code: "too_large", message: "envelopes are at most 1 MiB", to: frame.to });
            return;
          }
          if (![...this.tokens.values()].includes(frame.to)) {
            this.send(socket, { op: "error", code: "not_found", message: `no device ${frame.to}`, to: frame.to });
            return;
          }
          const seq = this.enqueue(device, frame.to, frame.envelope);
          this.send(socket, { op: "sent", to: frame.to, seq });
          return;
        }
        case "ack": {
          const queue = this.queues.get(device) ?? [];
          this.queues.set(
            device,
            queue.filter((q) => q.seq > frame.seq),
          );
          return;
        }
        case "ping":
          this.send(socket, { op: "pong" });
          return;
        case "auth": {
          if (frame.token && this.tokens.get(frame.token) === device) {
            this.renewals.set(device, (this.renewals.get(device) ?? 0) + 1);
            this.send(socket, { op: "auth", expires_at: Math.floor(Date.now() / 1000) + 900 });
          } else {
            this.send(socket, { op: "error", code: "unauthorized", message: "bad token" });
            socket.close();
          }
          return;
        }
        default:
          return;
      }
    });
    socket.on("close", () => {
      if (this.sockets.get(device) === socket) {
        this.sockets.delete(device);
        this.presence(device, false);
      }
    });
  }
}
