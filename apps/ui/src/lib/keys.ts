// Keys on the approval card: Enter allows once, Esc denies. Enter is
// ignored for a moment after the card appears (it may still be the Enter
// that sent the message) and when held down (key repeat).

export const ENTER_GRACE_MS = 300;

export interface KeyLike {
  key: string;
  repeat?: boolean;
  shiftKey?: boolean;
  metaKey?: boolean;
  ctrlKey?: boolean;
  altKey?: boolean;
  /** The focused element's tag, upper case (as the DOM gives it). */
  targetTag?: string;
  editable?: boolean;
}

export function approvalKey(event: KeyLike, shownAt: number, now: number): "allow" | "deny" | null {
  if (event.metaKey || event.ctrlKey || event.altKey) return null;
  if (event.key === "Escape") return event.repeat ? null : "deny";
  if (event.key !== "Enter" || event.shiftKey || event.repeat) return null;
  if (now - shownAt < ENTER_GRACE_MS) return null;
  const tag = event.targetTag ?? "";
  if (event.editable || tag === "TEXTAREA" || tag === "INPUT" || tag === "BUTTON" || tag === "SELECT") return null;
  return "allow";
}
