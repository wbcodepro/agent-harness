/**
 * The desktop app's shell, when the interface runs inside it.
 *
 * The desktop app (apps/desktop) serves this interface from the hame web
 * server it runs in process and, before any script of the page runs,
 * defines `window.__HAME_DESKTOP__` with the API token. Everywhere else
 * (`hame web` in a browser) it is absent and nothing here does anything.
 *
 * The shell sends the page `hame:desktop` DOM events: `{action: "open",
 * session}` (a notification or a `hame://` link was clicked) and
 * `{action: "new"}` (New Session in the menu or the tray).
 */

export interface DesktopBridge {
  readonly token: string;
  readonly platform: "macos" | "windows" | "linux";
  readonly version: string;
}

export type DesktopEvent = { action: "open"; session: string } | { action: "new" };

interface TauriInternals {
  invoke(command: string, args?: Record<string, unknown>): Promise<unknown>;
}

declare global {
  interface Window {
    __HAME_DESKTOP__?: DesktopBridge;
    __TAURI_INTERNALS__?: TauriInternals;
  }
}

export const desktop: DesktopBridge | null = (typeof window !== "undefined" && window.__HAME_DESKTOP__) || null;

function invoke(command: string, args?: Record<string, unknown>): Promise<unknown> {
  const internals = window.__TAURI_INTERNALS__;
  if (!desktop || !internals) return Promise.reject(new Error("not in the desktop app"));
  return internals.invoke(command, args);
}

/** Marks the page for the shell's styles (title bar inset). */
export function startDesktop(): void {
  if (!desktop) return;
  document.documentElement.dataset.desktop = desktop.platform;
}

/**
 * Tells the shell the page listens for its events. Until then the shell
 * keeps them (a link that launched the app, a notification clicked while
 * the page loaded); call it once the `hame:desktop` listener is attached.
 */
export function desktopReady(): void {
  if (!desktop) return;
  invoke("desktop_ready").catch(() => {
    // An older shell without the command: nothing depends on it.
  });
}

/** The native folder picker; `null` when the user cancels. */
export async function pickFolder(start?: string | null): Promise<string | null> {
  const folder = await invoke("pick_folder", { start: start ?? null });
  return typeof folder === "string" && folder ? folder : null;
}

/** Calls `handler` for each event from the shell; returns the unsubscribe. */
export function onDesktopEvent(handler: (event: DesktopEvent) => void): () => void {
  const listener = (event: Event) => {
    const detail = (event as CustomEvent<unknown>).detail as Partial<{ action: unknown; session: unknown }> | null;
    if (detail?.action === "new") handler({ action: "new" });
    else if (detail?.action === "open" && typeof detail.session === "string" && /^[A-Za-z0-9_-]{1,64}$/.test(detail.session)) {
      handler({ action: "open", session: detail.session });
    }
  };
  window.addEventListener("hame:desktop", listener);
  return () => window.removeEventListener("hame:desktop", listener);
}
