import type { Transport } from "./types";

/**
 * Placeholder for the transport through the relay (end-to-end encrypted,
 * see docs/design/APPS-AND-SYNC.md §2 and §4). The real implementation
 * lives in packages/sync-js and keeps the contract in ./README.md.
 */
export function createRemoteTransport(): Transport {
  const unavailable = (): never => {
    throw new Error("remote transport: not available yet");
  };
  return {
    kind: "remote",
    request: unavailable,
    upload: unavailable,
    subscribe: unavailable,
    fetchBlob: unavailable,
  };
}
