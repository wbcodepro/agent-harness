import type { ResponseLike, StreamState, Transport, Unsubscribe } from "./types";

const TOKEN_KEY = "hame.token";

/**
 * The token `hame web` printed in the address it opened. It is read from
 * `?token=` once, kept in this tab's session storage, and removed from the
 * address bar so it does not end up in a screenshot or a shared link.
 */
export function readToken(location: Location = window.location): string {
  const params = new URLSearchParams(location.search);
  const fromUrl = params.get("token");
  if (fromUrl) {
    try {
      sessionStorage.setItem(TOKEN_KEY, fromUrl);
    } catch {
      // Storage can be unavailable; the token still works for this load.
    }
    params.delete("token");
    const rest = params.toString();
    try {
      history.replaceState(null, "", location.pathname + (rest ? `?${rest}` : "") + location.hash);
    } catch {
      // Not fatal: the address just keeps the token.
    }
    return fromUrl;
  }
  try {
    return sessionStorage.getItem(TOKEN_KEY) ?? "";
  } catch {
    return "";
  }
}

/** The seq of a `{"kind":"record"}` event, null for other events. */
function recordSeq(event: unknown): number | null {
  if (typeof event !== "object" || event === null) return null;
  const { kind, record } = event as { kind?: unknown; record?: { seq?: unknown } };
  return kind === "record" && typeof record?.seq === "number" ? record.seq : null;
}

/** Waits 1 s, 2 s, 4 s … up to 15 s between reconnection attempts. */
function backoff(attempt: number): number {
  return Math.min(15000, 1000 * 2 ** Math.min(attempt, 4));
}

/** HTTP and server-sent events to the hame web server that served this page. */
export function createLocalTransport(token: string, base = ""): Transport {
  const headers = (extra?: Record<string, string>) => ({ "x-hame-token": token, ...extra });

  return {
    kind: "local",

    request(method, path, body) {
      return fetch(base + path, {
        method,
        headers: headers(body === undefined ? undefined : { "content-type": "application/json" }),
        body: body === undefined ? undefined : JSON.stringify(body),
      }) as Promise<ResponseLike>;
    },

    upload(path, data, contentType) {
      return fetch(base + path, {
        method: "POST",
        headers: headers({ "content-type": contentType || "application/octet-stream" }),
        body: data,
      }) as Promise<ResponseLike>;
    },

    subscribe(path, afterSeq, onEvent, onEnd, onState?: (state: StreamState) => void): Unsubscribe {
      let last = afterSeq;
      let source: EventSource | null = null;
      let timer: ReturnType<typeof setTimeout> | null = null;
      let attempt = 0;
      let stopped = false;

      const open = () => {
        if (stopped) return;
        const query = new URLSearchParams({ token });
        if (last !== null) query.set("after", String(last));
        source = new EventSource(`${base}${path}?${query}`);
        source.onopen = () => {
          attempt = 0;
          onState?.("open");
        };
        source.onmessage = (message) => {
          let event: unknown;
          try {
            event = JSON.parse(message.data);
          } catch {
            return;
          }
          const seq = recordSeq(event);
          if (seq !== null) {
            if (last !== null && seq <= last) return;
            last = seq;
          }
          onEvent(event, seq);
        };
        source.onerror = () => {
          if (stopped || !source) return;
          // The browser retries by itself (sending Last-Event-ID) unless the
          // server refused the stream; then retry from here, after the last
          // record delivered.
          onState?.("reconnecting");
          if (source.readyState === EventSource.CLOSED) {
            source = null;
            void probe();
          }
        };
      };

      // A stream refused for good (the session was deleted) ends; anything
      // else is retried with growing pauses.
      const probe = async () => {
        try {
          const response = await fetch(`${base}${path.replace(/\/events$/, "")}`, { headers: headers() });
          if (response.status === 404 || response.status === 403) {
            stopped = true;
            onEnd(new Error(response.status === 404 ? "the session no longer exists" : "the token was refused"));
            return;
          }
        } catch {
          // Unreachable for now; keep trying.
        }
        if (stopped) return;
        timer = setTimeout(open, backoff(attempt++));
      };

      open();
      return () => {
        stopped = true;
        if (timer) clearTimeout(timer);
        source?.close();
        source = null;
      };
    },

    async fetchBlob(sha256, session) {
      const query = new URLSearchParams({ session });
      const response = await fetch(`${base}/api/artifacts/${encodeURIComponent(sha256)}?${query}`, {
        headers: headers(),
      });
      if (!response.ok) throw new Error((await response.text()) || `artifact unavailable (${response.status})`);
      return response.blob();
    },
  };
}
