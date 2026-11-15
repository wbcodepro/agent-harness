// Streamed Markdown is parsed again as it grows. To keep that cheap, the
// text is cut into blocks at blank lines outside fenced code: every block
// but the last is finished and is parsed once; only the last one changes.

/** `text` as finished blocks followed by the block still being written. */
export function splitBlocks(text: string): string[] {
  const blocks: string[] = [];
  let current: string[] = [];
  let fence: string | null = null;
  for (const line of text.split("\n")) {
    const marker = /^\s{0,3}(`{3,}|~{3,})/.exec(line)?.[1];
    if (marker) {
      if (fence === null) fence = marker[0];
      else if (marker[0] === fence) fence = null;
    }
    if (fence === null && line.trim() === "") {
      if (current.length > 0) {
        blocks.push(current.join("\n"));
        current = [];
      }
      continue;
    }
    current.push(line);
  }
  if (current.length > 0 || blocks.length === 0) blocks.push(current.join("\n"));
  return blocks;
}
