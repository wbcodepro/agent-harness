// Line diffs for the diff cards, built from what the edit tools were asked
// to do (their input), since the session log does not keep file contents.

export type LineKind = "context" | "add" | "del";

export interface DiffLine {
  kind: LineKind;
  text: string;
}

export interface Hunk {
  /** A label such as "lines 12–14", when the tool said where. */
  header?: string;
  lines: DiffLine[];
}

export interface FileDiff {
  path: string;
  /** "edit" (exact replacement), "write" (whole file), "add", "delete", "move", "lines" (by line anchors). */
  op: "edit" | "write" | "add" | "delete" | "move" | "lines";
  added: number;
  removed: number;
  hunks: Hunk[];
  /** For a move: where the file went. */
  movedTo?: string;
}

const MAX_CELLS = 2_000_000;

function splitLines(text: string): string[] {
  if (text === "") return [];
  const lines = text.split("\n");
  if (lines[lines.length - 1] === "") lines.pop();
  return lines;
}

/** The lines of `before` and `after` as kept, removed and added lines (LCS). */
export function lineDiff(before: string, after: string): DiffLine[] {
  const a = splitLines(before);
  const b = splitLines(after);
  // Common prefix and suffix first: most edits touch a few lines.
  let start = 0;
  while (start < a.length && start < b.length && a[start] === b[start]) start++;
  let endA = a.length;
  let endB = b.length;
  while (endA > start && endB > start && a[endA - 1] === b[endB - 1]) {
    endA--;
    endB--;
  }
  const head: DiffLine[] = a.slice(0, start).map((text) => ({ kind: "context", text }));
  const tail: DiffLine[] = a.slice(endA).map((text) => ({ kind: "context", text }));
  const midA = a.slice(start, endA);
  const midB = b.slice(start, endB);
  let middle: DiffLine[];
  if (midA.length * midB.length > MAX_CELLS) {
    middle = [
      ...midA.map((text): DiffLine => ({ kind: "del", text })),
      ...midB.map((text): DiffLine => ({ kind: "add", text })),
    ];
  } else {
    const n = midA.length;
    const m = midB.length;
    // lengths[i][j]: LCS length of midA[i..] and midB[j..].
    const lengths: Uint32Array[] = Array.from({ length: n + 1 }, () => new Uint32Array(m + 1));
    for (let i = n - 1; i >= 0; i--) {
      for (let j = m - 1; j >= 0; j--) {
        lengths[i][j] = midA[i] === midB[j] ? lengths[i + 1][j + 1] + 1 : Math.max(lengths[i + 1][j], lengths[i][j + 1]);
      }
    }
    middle = [];
    let i = 0;
    let j = 0;
    while (i < n || j < m) {
      if (i < n && j < m && midA[i] === midB[j]) {
        middle.push({ kind: "context", text: midA[i] });
        i++;
        j++;
      } else if (i < n && (j === m || lengths[i + 1][j] >= lengths[i][j + 1])) {
        // Removed lines before the added lines that replace them.
        middle.push({ kind: "del", text: midA[i] });
        i++;
      } else {
        middle.push({ kind: "add", text: midB[j] });
        j++;
      }
    }
  }
  return [...head, ...middle, ...tail];
}

/** Groups changed lines with `context` unchanged lines around them. */
export function toHunks(lines: DiffLine[], context = 3): Hunk[] {
  const changed = lines.map((line, index) => (line.kind === "context" ? -1 : index)).filter((index) => index >= 0);
  if (changed.length === 0) return [];
  const hunks: Hunk[] = [];
  let from = Math.max(0, changed[0] - context);
  let to = Math.min(lines.length - 1, changed[0] + context);
  for (const index of changed.slice(1)) {
    if (index - context <= to + 1) {
      to = Math.min(lines.length - 1, index + context);
    } else {
      hunks.push({ lines: lines.slice(from, to + 1) });
      from = Math.max(0, index - context);
      to = Math.min(lines.length - 1, index + context);
    }
  }
  hunks.push({ lines: lines.slice(from, to + 1) });
  return hunks;
}

function count(hunks: Hunk[]): { added: number; removed: number } {
  let added = 0;
  let removed = 0;
  for (const hunk of hunks) {
    for (const line of hunk.lines) {
      if (line.kind === "add") added++;
      else if (line.kind === "del") removed++;
    }
  }
  return { added, removed };
}

function file(path: string, op: FileDiff["op"], hunks: Hunk[], extra?: Partial<FileDiff>): FileDiff {
  return { path, op, hunks, ...count(hunks), ...extra };
}

/** Parses the `*** Begin Patch` format of the apply_patch tool. */
export function parsePatch(patch: string): FileDiff[] {
  const files: FileDiff[] = [];
  let current: { path: string; op: FileDiff["op"]; hunks: Hunk[]; movedTo?: string } | null = null;
  let hunk: Hunk | null = null;
  const finish = () => {
    if (current) {
      if (hunk && hunk.lines.length) current.hunks.push(hunk);
      files.push(file(current.path, current.op, current.hunks, current.movedTo ? { movedTo: current.movedTo } : {}));
    }
    current = null;
    hunk = null;
  };
  for (const raw of patch.split("\n")) {
    const line = raw.replace(/\r$/, "");
    const header = /^\*\*\* (Update|Add|Delete) File: (.+)$/.exec(line);
    if (header) {
      finish();
      const op = header[1] === "Add" ? "add" : header[1] === "Delete" ? "delete" : "edit";
      current = { path: header[2].trim(), op, hunks: [] };
      hunk = { lines: [] };
      continue;
    }
    if (!current) continue;
    const move = /^\*\*\* Move to: (.+)$/.exec(line);
    if (move) {
      current.movedTo = move[1].trim();
      current.op = "move";
      continue;
    }
    if (line.startsWith("*** ")) continue; // End Patch, End of File
    if (line.startsWith("@@")) {
      if (hunk && hunk.lines.length) current.hunks.push(hunk);
      const label = line.replace(/^@@\s*/, "").replace(/\s*@@$/, "");
      hunk = { header: label || undefined, lines: [] };
      continue;
    }
    hunk ??= { lines: [] };
    if (line.startsWith("+")) hunk.lines.push({ kind: "add", text: line.slice(1) });
    else if (line.startsWith("-")) hunk.lines.push({ kind: "del", text: line.slice(1) });
    else if (line.startsWith(" ")) hunk.lines.push({ kind: "context", text: line.slice(1) });
    else if (line === "") hunk.lines.push({ kind: "context", text: "" });
  }
  finish();
  // A trailing empty context line is the patch's own line break.
  for (const diff of files) {
    for (const h of diff.hunks) {
      while (h.lines.length && h.lines[h.lines.length - 1].kind === "context" && h.lines[h.lines.length - 1].text === "") h.lines.pop();
    }
  }
  return files;
}

function str(value: unknown): string | undefined {
  return typeof value === "string" ? value : undefined;
}

function anchorLine(anchor: unknown): number | undefined {
  const match = typeof anchor === "string" ? /^(\d+)/.exec(anchor) : null;
  return match ? Number(match[1]) : undefined;
}

/**
 * The file changes an edit tool call asks for, from its input; empty for
 * tools that change no file.
 */
export function diffsFromToolInput(name: string, input: unknown): FileDiff[] {
  if (typeof input !== "object" || input === null) return [];
  const args = input as Record<string, unknown>;
  const path = str(args.path) ?? str(args.file_path) ?? "";
  switch (name) {
    case "edit": {
      const before = str(args.old_string);
      const after = str(args.new_string);
      if (!path || before === undefined || after === undefined) return [];
      return [file(path, "edit", toHunks(lineDiff(before, after)))];
    }
    case "write": {
      const content = str(args.content);
      if (!path || content === undefined) return [];
      return [file(path, "write", toHunks(lineDiff("", content), 0))];
    }
    case "apply_patch": {
      const patch = str(args.patch) ?? str(args.input);
      return patch ? parsePatch(patch) : [];
    }
    case "edit_lines": {
      if (!path || !Array.isArray(args.edits)) return [];
      const hunks: Hunk[] = [];
      let removed = 0;
      for (const edit of args.edits as Record<string, unknown>[]) {
        const from = anchorLine(edit.from);
        const to = anchorLine(edit.to) ?? from;
        const text = str(edit.new_text) ?? "";
        if (from !== undefined && to !== undefined) removed += Math.max(0, to - from + 1);
        hunks.push({
          header: from === undefined ? undefined : from === to ? `line ${from}` : `lines ${from}–${to}`,
          lines: splitLines(text).map((line) => ({ kind: "add", text: line })),
        });
      }
      const diff = file(path, "lines", hunks);
      diff.removed = removed;
      return [diff];
    }
    default:
      return [];
  }
}
