import { useState } from "react";
import { FileDiff as FileDiffIcon, FilePlus, FileX, MoveRight } from "lucide-react";
import type { FileDiff } from "../model/diff";
import styles from "./DiffCard.module.css";

const FOLD_AFTER = 9;

const OP_LABEL: Record<FileDiff["op"], string> = {
  edit: "edited",
  write: "written",
  add: "added",
  delete: "deleted",
  move: "moved",
  lines: "edited",
};

export function DiffStat({ added, removed }: { added: number; removed: number }) {
  return (
    <span className={styles.stat} aria-label={`${added} lines added, ${removed} removed`}>
      <span className={styles.plus}>+{added}</span>
      <span className={styles.minus}>−{removed}</span>
    </span>
  );
}

/** A file's change: `+N −M`, hunks with `⋯` between them, folded after 9 rows. */
export function DiffCard({ diff, onOpen, folded = true }: { diff: FileDiff; onOpen?: () => void; folded?: boolean }) {
  const [open, setOpen] = useState(!folded);
  const rows: ({ kind: "gap"; header?: string } | { kind: "line"; line: FileDiff["hunks"][number]["lines"][number] })[] = [];
  diff.hunks.forEach((hunk, n) => {
    if (n > 0 || hunk.header) rows.push({ kind: "gap", header: hunk.header });
    for (const line of hunk.lines) rows.push({ kind: "line", line });
  });
  const shown = open ? rows : rows.slice(0, FOLD_AFTER);
  const Icon = diff.op === "add" || diff.op === "write" ? FilePlus : diff.op === "delete" ? FileX : diff.op === "move" ? MoveRight : FileDiffIcon;
  return (
    <div className={styles.card}>
      <div className={styles.head}>
        <Icon size={14} aria-hidden className={styles.icon} />
        {onOpen ? (
          <button type="button" className={styles.path} onClick={onOpen} title={`Open ${diff.path} in Changes`}>
            {diff.path}
          </button>
        ) : (
          <span className={styles.path}>{diff.path}</span>
        )}
        {diff.movedTo && <span className={styles.moved}>→ {diff.movedTo}</span>}
        <span className={styles.op}>{OP_LABEL[diff.op]}</span>
        <DiffStat added={diff.added} removed={diff.removed} />
      </div>
      {rows.length > 0 && (
        <div className={styles.body} role="table" aria-label={`Changes to ${diff.path}`}>
          {shown.map((row, n) =>
            row.kind === "gap" ? (
              <div key={n} className={styles.gap} role="row">
                <span role="cell">⋯ {row.header ?? ""}</span>
              </div>
            ) : (
              <div key={n} className={`${styles.line} ${styles[row.line.kind]}`} role="row">
                <span className={styles.sign} role="cell" aria-hidden>
                  {row.line.kind === "add" ? "+" : row.line.kind === "del" ? "−" : " "}
                </span>
                <span className={styles.text} role="cell">
                  <span className="sr-only">{row.line.kind === "add" ? "added: " : row.line.kind === "del" ? "removed: " : ""}</span>
                  {row.line.text || " "}
                </span>
              </div>
            ),
          )}
          {rows.length > FOLD_AFTER && (
            <button type="button" className={styles.more} onClick={() => setOpen(!open)}>
              {open ? "Show less" : `Show ${rows.length - FOLD_AFTER} more lines`}
            </button>
          )}
        </div>
      )}
    </div>
  );
}
