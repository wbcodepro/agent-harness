# Transports

The UI talks to a hame host only through the `Transport` interface in
`types.ts`. Two implementations exist:

| Kind | File | What |
|---|---|---|
| `local` | `local.ts` | HTTP and server-sent events to the `hame web` server that served the page (or that the desktop app runs in process). |
| `remote` | `remote.ts` here, real one in `packages/sync-js` | The same API through the relay, end-to-end encrypted (`docs/design/APPS-AND-SYNC.md` §2 and §4). The placeholder here throws `remote transport: not available yet`. |

A remote implementation must behave exactly like the local one as seen
through this interface; the UI has no code path that knows which one it
has (it only reads `kind` for labels).

## The interface

```ts
interface Transport {
  readonly kind: "local" | "remote";
  request(method: string, path: string, body?: unknown): Promise<ResponseLike>;
  upload(path: string, data: Blob, contentType: string): Promise<ResponseLike>;
  subscribe(
    path: string,
    afterSeq: number | null,
    onEvent: (event: unknown, seq: number | null) => void,
    onEnd: (error?: Error) => void,
    onState?: (state: "open" | "reconnecting") => void,
  ): Unsubscribe;
  fetchBlob(sha256: string, session: string): Promise<Blob>;
}

interface ResponseLike {
  readonly ok: boolean;      // status in 200..299
  readonly status: number;   // the HTTP status the hame web router returned
  json(): Promise<unknown>;
  text(): Promise<string>;
}

type Unsubscribe = () => void;
```

### `request(method, path, body?)`

- `method`: `GET`, `POST`, `PUT`, `PATCH` or `DELETE`.
- `path`: the full API path including any query, e.g.
  `/api/sessions/0193…/prompt`. Never a full URL.
- `body`: a JSON value or `undefined`. Local sends it as
  `content-type: application/json`; remote puts it in the `req` frame's
  `body` field as JSON (not a string).
- Resolves with the response **whatever its status** (a 409 is a normal
  answer the UI handles). Rejects only when the host cannot be reached
  (network down, relay unreachable, host offline). A remote implementation
  should reject with an `Error` whose message says which ("the host is
  offline").
- Remote mapping: `{"t":"req","id":N,"method":…,"path":…,"body":…}` →
  `{"t":"res","id":N,"status":S,"body":…}`. `json()` returns `body`;
  `text()` returns `body` as a string when it is a string (the router's
  error messages are plain text), otherwise `JSON.stringify(body)`.

### `upload(path, data, contentType)`

Raw bytes for `POST /api/sessions/{id}/attachments?name=…`. `contentType`
is the file's type (`image/png`, `text/plain`, … or
`application/octet-stream`); the host decides from it whether the upload
is an image. Limit: `limits.upload_bytes` from `GET /api/info` (20 MiB);
the host answers 413 above it. Remote: base64 the bytes into the `req`
frame body as `{"base64": "…", "content_type": "…"}` or chunk them like
blobs; the host connector turns it back into a raw request body with that
content type before calling the router.

### `subscribe(path, afterSeq, onEvent, onEnd, onState?)`

Follows `GET /api/sessions/{id}/events`.

- `afterSeq`: the last record `seq` the caller has, or `null` for the whole
  session. Local sends `?after=N`; remote sends `{"t":"sub",…,"after":N}`.
- `onEvent(event, seq)`: each event's parsed JSON, in order. For
  `{"kind":"record","record":{…}}` events `seq` is `record.seq`; for all
  other kinds (`approval`, `approval_resolved`, `turn`, `steering`,
  `warning`, `settings`, `session`) it is `null`.
- Records arrive with strictly increasing `seq`. The transport must never
  deliver a record with a `seq` at or below one it already delivered on
  this subscription (after a reconnect the host replays from `after`).
- Reconnects are the transport's job: after a dropped connection it calls
  `onState("reconnecting")`, resubscribes with `after` = the last record seq
  it delivered, and calls `onState("open")` once events flow again.
  Non-record events are not resumable: the host re-sends the live state
  (pending approvals, the queued messages, "turn started") on every
  subscription, and the UI's folding ignores duplicates.
- `onEnd(error)`: called once when the stream ends for good — the session
  was deleted (404), the token or device was refused (403), or the
  transport gave up. Never called after the caller unsubscribed.
- The returned function stops the subscription (remote: send
  `{"t":"cancel","id":N}`). Calling it twice is harmless.

### `fetchBlob(sha256, session)`

The bytes of an artifact (screenshot, image, file) by its lowercase hex
SHA-256, for `GET /api/artifacts/{sha256}?session={session}`. The host only
serves an artifact that the named session refers to. Rejects with an
`Error` when it is missing (404) or refused. Remote: request
`{"t":"blob","sha256":…}` and reassemble the 256 KiB `blob` chunks; check
that the SHA-256 of the result matches before resolving. The UI turns the
blob into an object URL and caches it per page.

## Errors and statuses the UI relies on

| Status | Meaning |
|---|---|
| 400 | bad input (message in the body as text) |
| 403 | missing or wrong token (local) / device not allowed (remote) |
| 404 | no such session or artifact |
| 409 | a turn is running (prompt, settings, delete) or none is (steer), or a queued message was already delivered |
| 413 | upload too large |
