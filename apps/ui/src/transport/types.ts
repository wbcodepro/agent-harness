// The one interface the UI talks to a hame host through. See README.md in
// this folder for the exact contract an implementation must keep.

/** The part of a fetch `Response` the UI uses. */
export interface ResponseLike {
  readonly ok: boolean;
  readonly status: number;
  json(): Promise<unknown>;
  text(): Promise<string>;
}

/** Stops a subscription. Calling it twice is harmless. */
export type Unsubscribe = () => void;

/** How a subscription's connection is doing, for a status line. */
export type StreamState = "open" | "reconnecting";

export interface Transport {
  /** "local" for the hame web server on this machine, "remote" through the relay. */
  readonly kind: "local" | "remote";

  /**
   * One API call. `path` is the full API path, such as
   * `/api/sessions/abc/prompt`. `body`, when given, is sent as JSON.
   * Resolves with the response whatever its status; rejects only when the
   * host cannot be reached.
   */
  request(method: string, path: string, body?: unknown): Promise<ResponseLike>;

  /**
   * Sends raw bytes (an attachment) with a content type. `path` includes
   * its query, such as `/api/sessions/abc/attachments?name=shot.png`.
   */
  upload(path: string, data: Blob, contentType: string): Promise<ResponseLike>;

  /**
   * Follows a session's event stream (`/api/sessions/{id}/events`).
   * `afterSeq` is the last record seq the caller already has, or null for
   * everything. `onEvent` gets each event's parsed JSON and, for records,
   * its seq. The transport reconnects by itself after a dropped connection
   * and resumes after the last seq it delivered; `onState` reports that.
   * `onEnd` is called once if the stream ends for good (the session no
   * longer exists, or the transport gave up), never after unsubscribing.
   */
  subscribe(
    path: string,
    afterSeq: number | null,
    onEvent: (event: unknown, seq: number | null) => void,
    onEnd: (error?: Error) => void,
    onState?: (state: StreamState) => void,
  ): Unsubscribe;

  /**
   * The bytes of an artifact (an image or file a session refers to), by
   * its sha256. `session` is the session that refers to it: the host serves
   * an artifact only to a caller that names such a session.
   */
  fetchBlob(sha256: string, session: string): Promise<Blob>;
}
