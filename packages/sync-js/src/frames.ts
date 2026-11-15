// Relay WebSocket frames (visible to the relay) and remote API frames
// (only inside envelopes). Mirrors crates/hame-sync/src/frames.rs.

export type RelayFrame =
  | { op: "send"; to: string; envelope: string; push?: boolean }
  | { op: "sent"; to: string; seq: number }
  | { op: "deliver"; seq: number; from: string; envelope: string }
  | { op: "ack"; seq: number }
  | { op: "resume"; after: number }
  | { op: "presence"; device: string; online: boolean }
  | { op: "error"; code: string; message?: string; to?: string }
  | { op: "auth"; token?: string; expires_at?: number }
  | { op: "ping" }
  | { op: "pong" };

export type Frame =
  | { t: "req"; id: number; method: string; path: string; body?: unknown }
  | { t: "res"; id: number; status: number; body: unknown }
  | { t: "sub"; id: number; path: string; after: number }
  | { t: "evt"; id: number; seq: number; data: unknown }
  | { t: "end"; id: number; error?: string }
  | { t: "cancel"; id: number }
  | { t: "blob"; id: number; sha256: string; chunk: number; of: number; data: string }
  | { t: "pair"; ed: string; x: string; label: string; mac: string }
  | { t: "paired"; grant: string; sig: string; last_counter?: number }
  | { t: "pair_refused"; reason: string }
  | { t: "pair_challenge"; nonce: string }
  | { t: "revoke"; id: number; device: string }
  | { t: "devices"; id: number };

/** The stream id a frame belongs to; the envelope header carries it too. */
export function streamOf(frame: Frame): string {
  switch (frame.t) {
    case "pair":
    case "paired":
    case "pair_refused":
    case "pair_challenge":
      return "pair";
    default:
      return String(frame.id);
  }
}

/** Artifact chunk size before base64. */
export const BLOB_CHUNK = 256 * 1024;
