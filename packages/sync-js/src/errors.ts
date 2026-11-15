export type SyncErrorCode =
  | "malformed"
  | "too_large"
  | "crypto"
  | "bad_signature"
  | "wrong_recipient"
  | "unknown_sender"
  | "replay"
  | "pairing"
  | "relay"
  | "closed"
  | "timeout";

export class SyncError extends Error {
  constructor(
    readonly code: SyncErrorCode,
    message: string,
  ) {
    super(message);
    this.name = "SyncError";
  }
}
