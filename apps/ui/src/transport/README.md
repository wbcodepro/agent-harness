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
  other kinds (`approval`, `approvals`, `approval_resolved`, `turn`,
  `steering`, `grants`, `warning`, `settings`, `session`) it is `null`.
- Records arrive with strictly increasing `seq`. The transport must never
  deliver a record with a `seq` at or below one it already delivered on
  this subscription (after a reconnect the host replays from `after`).
- Reconnects are the transport's job: after a dropped connection it calls
  `onState("reconnecting")`, resubscribes with `after` = the last record seq
  it delivered, and calls `onState("open")` once events flow again.
  Non-record events are not resumable: on every subscription the host
  sends the whole live state after the records — `turn` (started, or ended
  with the last error), `approvals` (the full list of waiting questions,
  which replaces the page's), `steering` (the queue) and `grants`.
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

## Approvals

An approval event names the call it is about:
`{"kind":"approval","id":N,"question":…,"tool":…,"tool_call_id":…,"arguments":…,"args_sha256":…,"effect":…,"scope":…}`,
where `arguments` are the call's exact arguments and `args_sha256` the
lowercase hex SHA-256 of their canonical JSON (RFC 8785), or `null` when
they have no canonical form (integers beyond 2^53). The answer
`POST /api/sessions/{id}/approval {"id":N,"allow":bool,"always":bool,"tool_call_id":…,"args_sha256":…}`
must carry the same `tool_call_id` and `args_sha256`, or the host answers
400 (a question without a hash can only be answered on the host itself).
The relay connector checks the pair too before passing an answer on, and a
device recomputes the hash from the arguments it shows (APPS-AND-SYNC.md
§4).

## Remote callers

The host connector calls the router in process (`hame_web::router`) and
marks each request with `hame_web::RemoteCaller { device_id, label }`
(`RemoteCaller::attach` or `hame_web::mark_remote`). Such requests need no
token and get the remote policy of `[remote]` in the host's config.toml,
on every route that acts on a session (prompt, steer, withdraw, approval,
attachments, settings, grants, delete; viewing and stopping are always
allowed):

- the session's folder must be in `roots` (default: folders that already
  have a session, and the folder `hame web` started in);
- unless `allow_full_access = true`: no choosing "Full access", no
  "always" answers, and no prompting, queueing, answering or uploading in a
  session that runs without the sandbox and without asking (Full access,
  or the default mode while `[sandbox] mode = "danger-full-access"`); a
  device may still lower such a session's mode;
- "always allow" grants given on the host do not answer questions in turns
  a remote device started, or that received (or will receive) a message it
  queued; the `steering` event names who queued each message in
  `origins` (null for the host, else the device's label).

Refusals are 403 with the reason as text. Choosing "Full access" (local or remote) is
answered 428 `{"confirm": nonce, "message": …}` first; the same request
again with `"confirm": nonce` applies it.

## Errors and statuses the UI relies on

| Status | Meaning |
|---|---|
| 400 | bad input (message in the body as text) |
| 403 | missing or wrong token (local), or refused by the remote policy |
| 404 | no such session or artifact |
| 409 | a turn is running (prompt, settings, delete) or none is (steer), or a queued message was already delivered |
| 423 | another hame process (a terminal, an editor) has the session open |
| 413 | upload too large |
| 428 | "Full access" needs the confirmation nonce in the body |
