export * from "./encoding.js";
export * from "./envelope.js";
export * from "./errors.js";
export * from "./frames.js";
export * from "./jcs.js";
export * from "./keys.js";
export * from "./pairing.js";
export * from "./relay.js";
export * from "./remote.js";

import type { DeviceIdentity, KeyStorage, PublicKeys } from "./keys.js";
import { RelayClient, type SocketFactory } from "./relay.js";
import { type CounterStore, PersistentSequence, RemoteHost, RemoteTransport } from "./remote.js";

export interface ConnectRemoteOptions {
  relayUrl: string;
  deviceId: string;
  token: () => Promise<string>;
  identity: DeviceIdentity;
  /** Where the identity lives: this device's envelope counter is kept there too. */
  storage: KeyStorage;
  hostId: string;
  /** The host's keys pinned at pairing (from the grant or the QR code). */
  hostKeys?: PublicKeys;
  counters?: CounterStore;
  socket?: SocketFactory;
  onRejected?: (reason: string) => void;
  onState?: (connected: boolean) => void;
}

/**
 * Wires a relay connection to a host: returns the transport for the UI, the
 * host handle (pairing, devices, revoke) and the relay client (stop()).
 */
export function connectRemote(options: ConnectRemoteOptions): {
  transport: RemoteTransport;
  host: RemoteHost;
  relay: RelayClient;
} {
  let relay: RelayClient | undefined;
  const host = new RemoteHost({
    identity: options.identity,
    deviceId: options.deviceId,
    hostId: options.hostId,
    hostKeys: options.hostKeys,
    counters: options.counters,
    sequence: new PersistentSequence(options.storage, `counter:${options.deviceId}`),
    onRejected: options.onRejected,
    send: (to, envelope) => relay!.send(to, envelope),
  });
  relay = new RelayClient({
    url: options.relayUrl,
    deviceId: options.deviceId,
    token: options.token,
    socket: options.socket,
    onState: options.onState,
    onDeliver: (from, envelope) => host.handleDelivery(from, envelope),
    // A host that comes back has lost its streams: subscribe again from
    // the last event seen.
    onPresence: (device, online) => {
      if (online && device === options.hostId) host.resubscribe();
    },
  });
  relay.start();
  return { transport: new RemoteTransport(host), host, relay };
}
