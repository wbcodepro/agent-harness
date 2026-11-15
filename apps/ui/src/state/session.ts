// A small store per open session: it follows the event stream, folds the
// events into a view model and tells the view at most once per frame.

import { useSyncExternalStore } from "react";
import { Folder, type ViewState } from "../model/fold";
import type { StreamEvent } from "../model/types";
import type { StreamState, Transport, Unsubscribe } from "../transport";

export type Connection = "connecting" | StreamState | "ended";

export interface SessionView {
  view: ViewState;
  connection: Connection;
  endReason?: string;
}

const frame: (callback: () => void) => void =
  typeof requestAnimationFrame === "function" ? (callback) => requestAnimationFrame(() => callback()) : (callback) => setTimeout(callback, 16);

export class SessionStore {
  private folder = new Folder();
  private listeners = new Set<() => void>();
  private unsubscribe: Unsubscribe | null = null;
  private scheduled = false;
  private snapshot: SessionView;
  private connection: Connection = "connecting";
  private endReason?: string;
  /** Called for every event, after folding (for the session list). */
  onEvent?: (event: StreamEvent) => void;

  constructor(
    readonly id: string,
    private transport: Transport,
  ) {
    this.snapshot = this.make();
  }

  private make(): SessionView {
    // A new outer object and a new items array each flush, so React sees
    // the change; unchanged items keep their identity.
    const state = this.folder.state;
    return { view: { ...state, items: [...state.items] }, connection: this.connection, endReason: this.endReason };
  }

  start() {
    if (this.unsubscribe) return;
    this.unsubscribe = this.transport.subscribe(
      `/api/sessions/${this.id}/events`,
      this.folder.state.lastSeq,
      (event) => {
        this.folder.apply(event as StreamEvent);
        this.onEvent?.(event as StreamEvent);
        this.schedule();
      },
      (error) => {
        this.connection = "ended";
        this.endReason = error?.message;
        this.schedule();
      },
      (state) => {
        this.connection = state;
        this.schedule();
      },
    );
  }

  stop() {
    this.unsubscribe?.();
    this.unsubscribe = null;
  }

  /** Applies an event that did not come through the stream (an answer given here). */
  local(event: StreamEvent) {
    this.folder.apply(event);
    this.schedule();
  }

  private schedule() {
    if (this.scheduled) return;
    this.scheduled = true;
    frame(() => {
      this.scheduled = false;
      this.snapshot = this.make();
      for (const listener of this.listeners) listener();
    });
  }

  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  getSnapshot = () => this.snapshot;
}

const EMPTY: SessionView = { view: new Folder().state, connection: "connecting" };
const noop = () => () => {};

export function useSession(store: SessionStore | null): SessionView {
  return useSyncExternalStore(store?.subscribe ?? noop, store?.getSnapshot ?? (() => EMPTY));
}
