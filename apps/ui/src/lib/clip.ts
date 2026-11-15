// Long tool and command output is shown clipped, with "show all".

export interface Clipped {
  text: string;
  /** Lines left out (0 when nothing was). */
  hiddenLines: number;
  clipped: boolean;
}

/** The first `maxLines` lines of `text`, at most `maxChars` characters. */
export function clip(text: string, maxLines = 40, maxChars = 4000): Clipped {
  const lines = text.split("\n");
  let shown = lines.slice(0, maxLines).join("\n");
  if (shown.length > maxChars) shown = shown.slice(0, maxChars);
  if (shown.length === text.length) return { text, hiddenLines: 0, clipped: false };
  const shownLines = shown.split("\n").length;
  return { text: shown, hiddenLines: Math.max(0, lines.length - shownLines), clipped: true };
}
