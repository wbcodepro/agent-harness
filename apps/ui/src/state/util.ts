import { createContext, useContext, useEffect, useState, type KeyboardEvent as ReactKeyboardEvent } from "react";
import type { Api } from "./api";

/** Per-viewer conveniences (theme, drafts, what was read); never required. */
export const storage = {
  get(key: string): string | null {
    try {
      return localStorage.getItem(key);
    } catch {
      return null;
    }
  },
  set(key: string, value: string | null) {
    try {
      if (value === null) localStorage.removeItem(key);
      else localStorage.setItem(key, value);
    } catch {
      // Storage unavailable (private window, blocked): nothing is kept.
    }
  },
};

export const isMac = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
export const modKey = isMac ? "⌘" : "Ctrl";

export function isMod(event: KeyboardEvent | ReactKeyboardEvent): boolean {
  return isMac ? event.metaKey : event.ctrlKey;
}

export function relativeTime(ts: number, now = Date.now()): string {
  if (!ts) return "";
  const seconds = Math.round((now - ts) / 1000);
  if (seconds < 45) return "now";
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours}h`;
  const days = Math.round(hours / 24);
  if (days < 7) return `${days}d`;
  return new Date(ts).toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

export function duration(ms: number): string {
  if (ms < 1000) return `${ms} ms`;
  const seconds = ms / 1000;
  if (seconds < 60) return `${seconds < 10 ? seconds.toFixed(1) : Math.round(seconds)} s`;
  const minutes = Math.floor(seconds / 60);
  return `${minutes} min ${Math.round(seconds % 60)} s`;
}

export function tokens(n: number): string {
  if (n < 1000) return String(n);
  if (n < 100_000) return `${(n / 1000).toFixed(1)}k`;
  return `${Math.round(n / 1000)}k`;
}

export function money(usd: number): string {
  return usd < 0.01 ? `$${usd.toFixed(4)}` : `$${usd.toFixed(2)}`;
}

export function folderName(path: string | null | undefined): string {
  if (!path) return "";
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

export interface Services {
  api: Api;
  session: string | null;
  openImage: (image: { sha256: string; alt: string }) => void;
}

export const ServicesContext = createContext<Services | null>(null);

export function useServices(): Services {
  const services = useContext(ServicesContext);
  if (!services) throw new Error("ServicesContext missing");
  return services;
}

const urls = new Map<string, Promise<string>>();

/** An object URL for an artifact's bytes, fetched once per page. */
export function useArtifactUrl(sha256: string): { url: string | null; error: string | null } {
  const { api, session } = useServices();
  const [state, setState] = useState<{ url: string | null; error: string | null }>({ url: null, error: null });
  useEffect(() => {
    if (!session) return;
    let live = true;
    let pending = urls.get(sha256);
    if (!pending) {
      pending = api.transport.fetchBlob(sha256, session).then((blob) => URL.createObjectURL(blob));
      urls.set(sha256, pending);
      pending.catch(() => urls.delete(sha256));
    }
    pending.then(
      (url) => live && setState({ url, error: null }),
      (error: Error) => live && setState({ url: null, error: error.message }),
    );
    return () => {
      live = false;
    };
  }, [api, session, sha256]);
  return state;
}
