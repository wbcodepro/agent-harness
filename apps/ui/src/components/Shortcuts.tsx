import { useEffect, useRef } from "react";
import { X } from "lucide-react";
import { modKey } from "../state/util";
import styles from "./Shortcuts.module.css";

const SHORTCUTS: [string, string][] = [
  [`${modKey} K`, "Search sessions"],
  [`${modKey} N`, "New session"],
  [`${modKey} /`, "Show these shortcuts"],
  ["Enter", "Send the message (Shift Enter for a new line)"],
  ["Esc Esc", "Stop the running turn"],
  ["Enter", "Allow once, while an approval waits"],
  ["Esc", "Deny, while an approval waits"],
];

export function Shortcuts({ onClose }: { onClose: () => void }) {
  const close = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    const before = document.activeElement as HTMLElement | null;
    close.current?.focus();
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      before?.focus?.();
    };
  }, [onClose]);
  return (
    <div className={styles.scrim} onClick={onClose}>
      <div className={styles.dialog} role="dialog" aria-modal="true" aria-labelledby="shortcuts-title" onClick={(event) => event.stopPropagation()}>
        <div className={styles.head}>
          <h2 id="shortcuts-title">Keyboard shortcuts</h2>
          <button ref={close} type="button" onClick={onClose} aria-label="Close">
            <X size={16} aria-hidden />
          </button>
        </div>
        <dl>
          {SHORTCUTS.map(([keys, what]) => (
            <div key={what} className={styles.row}>
              <dt>
                {keys.split(" ").map((key, n) => (
                  <kbd key={n}>{key}</kbd>
                ))}
              </dt>
              <dd>{what}</dd>
            </div>
          ))}
        </dl>
      </div>
    </div>
  );
}
