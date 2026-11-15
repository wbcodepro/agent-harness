import { useState } from "react";
import { clip } from "../lib/clip";

/** Tool or command output, clipped to its first lines with "Show all". */
export function Output({ text, className, buttonClassName }: { text: string; className?: string; buttonClassName?: string }) {
  const [all, setAll] = useState(false);
  const clipped = clip(text);
  return (
    <>
      <pre className={className}>{all || !clipped.clipped ? text : clipped.text}</pre>
      {clipped.clipped && (
        <button type="button" className={buttonClassName} onClick={() => setAll(!all)}>
          {all ? "Show less" : clipped.hiddenLines > 0 ? `Show all (${clipped.hiddenLines} more lines)` : "Show all"}
        </button>
      )}
    </>
  );
}
