import { memo, useCallback, useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import {
  ArrowDown,
  Bot,
  Brain,
  ChevronRight,
  CircleCheck,
  CircleX,
  FileText,
  Globe,
  Link,
  ListChecks,
  Monitor,
  Pencil,
  Plug,
  RotateCw,
  Search,
  SquareTerminal,
  Wrench,
} from "lucide-react";
import type {
  AssistantItem,
  FooterItem,
  Item,
  NoticeItem,
  ReasoningRow,
  TaskItem,
  ToolRow,
  UserItem,
  VerdictItem,
  WorkItem,
} from "../model/fold";
import type { Category } from "../model/tools";
import { describePredicate } from "../model/tools";
import { duration, tokens } from "../state/util";
import { DiffCard } from "./DiffCard";
import { Markdown } from "./Markdown";
import { FileChip, Thumb } from "./Media";
import styles from "./Transcript.module.css";

const ICONS: Record<Category, typeof FileText> = {
  read: FileText,
  search: Search,
  edit: Pencil,
  run: SquareTerminal,
  browser: Globe,
  desktop: Monitor,
  subagent: Bot,
  web: Link,
  mcp: Plug,
  other: Wrench,
};

export interface TranscriptActions {
  onContinue: () => void;
  onOpenChange: (path: string) => void;
}

function UserMessage({ item }: { item: UserItem }) {
  return (
    <div className={styles.userRow}>
      <div className={styles.user}>
        {item.steering && <span className={styles.steerTag}>sent while working</span>}
        {item.text && <div className={styles.userText}>{item.text}</div>}
        {(item.images.length > 0 || item.files.length > 0) && (
          <div className={styles.attachments}>
            {item.images.map((image) => (
              <Thumb key={image.sha256} artifact={image} />
            ))}
            {item.files.map((file) => (
              <FileChip key={file.sha256} artifact={file} />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

function AssistantMessage({ item }: { item: AssistantItem }) {
  return (
    <div className={styles.assistant}>
      <Markdown text={item.text} live={item.streaming} />
      {item.streaming && <span className={styles.caret} aria-hidden />}
      {item.cut && <div className={styles.cut}>Response {item.cut === "cancelled" ? "stopped" : item.cut} before it finished.</div>}
    </div>
  );
}

function elapsed(row: ToolRow, now: number): string | null {
  const end = row.endTs ?? (row.status === "running" ? now : undefined);
  return end === undefined ? null : duration(Math.max(0, end - row.startTs));
}

function inputText(row: ToolRow): string {
  const input = row.input as Record<string, unknown> | null;
  if (row.name === "bash" && typeof input?.command === "string") return input.command;
  return JSON.stringify(row.input, null, 2);
}

const ToolRowView = memo(function ToolRowView({ row, actions, now }: { row: ToolRow; actions: TranscriptActions; now: number }) {
  const [open, setOpen] = useState(false);
  const Icon = ICONS[row.category];
  const time = elapsed(row, now);
  const failed = row.status === "failed";
  const editable = row.diffs.length > 0;
  return (
    <div className={styles.tool} data-status={row.status}>
      <button type="button" className={styles.toolLine} onClick={() => setOpen(!open)} aria-expanded={open}>
        <Icon size={15} aria-hidden className={styles.toolIcon} />
        <span className={styles.toolTitle}>
          <span className={row.status === "running" ? "shimmer" : undefined}>{row.title}</span>
          {row.detail && <code className={styles.toolDetail}>{row.detail}</code>}
        </span>
        {row.status === "waiting" && <span className={styles.waitCapsule}>needs approval</span>}
        {failed && (
          <span className={styles.failCapsule}>
            <span className={styles.redDot} aria-hidden />
            {row.exitCode !== undefined ? `exit ${row.exitCode}` : "failed"}
          </span>
        )}
        {row.status === "interrupted" && <span className={styles.neutralCapsule}>stopped</span>}
        {time && <span className={styles.time}>{time}</span>}
        <ChevronRight size={14} aria-hidden className={`${styles.chevron} ${open ? styles.open : ""}`} />
      </button>
      {editable && (
        <div className={styles.toolDiffs}>
          {row.diffs.map((diff) => (
            <DiffCard key={diff.path} diff={diff} onOpen={failed ? undefined : () => actions.onOpenChange(diff.path)} />
          ))}
        </div>
      )}
      {row.images.length > 0 && (
        <div className={styles.attachments}>
          {row.images.map((image) => (
            <Thumb key={image.sha256} artifact={image} />
          ))}
        </div>
      )}
      {open && (
        <div className={styles.toolBody}>
          <div className={styles.ioLabel}>Input</div>
          <pre className={styles.io}>{inputText(row)}</pre>
          {(row.output || row.status === "done" || failed) && (
            <>
              <div className={styles.ioLabel}>Output</div>
              <pre className={styles.io}>{row.output || "(no output)"}</pre>
            </>
          )}
          {row.files.map((file) => (
            <FileChip key={file.sha256} artifact={file} />
          ))}
        </div>
      )}
    </div>
  );
});

function Reasoning({ row }: { row: ReasoningRow }) {
  const [open, setOpen] = useState(false);
  const lines = row.text.split("\n").filter((line) => line.trim());
  const preview = row.streaming ? (lines[lines.length - 2] ?? "Thinking…") : (lines[0] ?? "Thought");
  return (
    <div className={styles.tool}>
      <button type="button" className={styles.toolLine} onClick={() => setOpen(!open)} aria-expanded={open}>
        <Brain size={15} aria-hidden className={styles.toolIcon} />
        <span className={`${styles.toolTitle} ${styles.reasoningTitle}`}>
          <span className={row.streaming ? "shimmer" : undefined}>{preview.replace(/[*_#`]/g, "")}</span>
        </span>
        <ChevronRight size={14} aria-hidden className={`${styles.chevron} ${open ? styles.open : ""}`} />
      </button>
      {open && <div className={styles.reasoning}>{row.text}</div>}
    </div>
  );
}

function useNow(active: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [active]);
  return now;
}

const WorkGroup = memo(function WorkGroup({ item, actions }: { item: WorkItem; actions: TranscriptActions }) {
  // Open while running; folds by itself when the turn ends, unless the
  // user opened or closed it.
  const [chosen, setChosen] = useState<boolean | null>(null);
  const open = chosen ?? item.running;
  const body = useRef<HTMLDivElement>(null);
  const now = useNow(item.running);
  useLayoutEffect(() => {
    const element = body.current;
    if (!element || !item.running) return;
    const near = element.scrollHeight - element.scrollTop - element.clientHeight < 60;
    if (near) element.scrollTop = element.scrollHeight;
  }, [item.rows, item.running]);
  const total = item.endTs ?? (item.running ? now : item.startTs);
  return (
    <section className={styles.work} aria-label={item.running ? `Working: ${item.live}` : item.summary}>
      <button type="button" className={styles.workHead} onClick={() => setChosen(!open)} aria-expanded={open}>
        <ChevronRight size={14} aria-hidden className={`${styles.chevron} ${open ? styles.open : ""}`} />
        <span className={styles.workTitle}>
          {item.running ? <span className="shimmer">{item.live}</span> : item.summary}
        </span>
        <span className={styles.time}>{duration(Math.max(0, total - item.startTs))}</span>
      </button>
      {open && (
        <div ref={body} className={styles.workBody}>
          {item.rows.map((row) =>
            row.kind === "tool" ? <ToolRowView key={row.key} row={row} actions={actions} now={now} /> : <Reasoning key={row.key} row={row} />,
          )}
        </div>
      )}
    </section>
  );
});

function Notice({ item, actions, last }: { item: NoticeItem; actions: TranscriptActions; last: boolean }) {
  return (
    <div className={`${styles.notice} ${styles[item.tone]}`} role={item.tone === "error" ? "alert" : undefined}>
      {item.tone !== "info" && <span className={item.tone === "error" ? styles.redDot : styles.amberDot} aria-hidden />}
      <span className={styles.noticeText}>{item.text}</span>
      {item.action === "continue" && last && (
        <button type="button" className={styles.continue} onClick={actions.onContinue}>
          <RotateCw size={13} aria-hidden /> Continue
        </button>
      )}
    </div>
  );
}

function TaskCard({ item }: { item: TaskItem }) {
  return (
    <div className={styles.card}>
      <div className={styles.cardHead}>
        <ListChecks size={16} aria-hidden /> Task
      </div>
      <div className={styles.cardBody}>
        <p className={styles.outcome}>{item.contract.outcome}</p>
        {item.contract.acceptance.length > 0 && (
          <ul className={styles.conditions}>
            {item.contract.acceptance.map((predicate, n) => (
              <li key={n}>{describePredicate(predicate)}</li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}

function VerdictCard({ item }: { item: VerdictItem }) {
  const { report } = item;
  return (
    <div className={`${styles.card} ${report.passed ? styles.verified : styles.unverified}`}>
      <div className={styles.cardHead}>
        {report.passed ? <CircleCheck size={16} aria-hidden /> : <CircleX size={16} aria-hidden />}
        {report.passed ? "Verified" : "Not verified"}
        <span className={styles.time}>{duration(report.ms)}</span>
      </div>
      <ul className={styles.results}>
        {report.results.map((result, n) => (
          <li key={n}>
            {result.passed ? (
              <CircleCheck size={14} aria-label="passed" className={styles.pass} />
            ) : (
              <CircleX size={14} aria-label="failed" className={styles.fail} />
            )}
            <div>
              <div>{describePredicate(result.predicate)}</div>
              {result.evidence && <div className={styles.evidence}>{result.evidence}</div>}
            </div>
          </li>
        ))}
      </ul>
    </div>
  );
}

function Footer({ item }: { item: FooterItem }) {
  const used = item.usage.input_tokens + item.usage.cache_read_tokens + item.usage.cache_write_tokens;
  return (
    <div className={styles.footer}>
      <span>{duration(item.durationMs)}</span>
      {used + item.usage.output_tokens > 0 && (
        <span>
          {tokens(used)} in · {tokens(item.usage.output_tokens)} out
        </span>
      )}
    </div>
  );
}

function ItemView({ item, actions, last }: { item: Item; actions: TranscriptActions; last: boolean }): ReactNode {
  switch (item.kind) {
    case "user":
      return <UserMessage item={item} />;
    case "assistant":
      return <AssistantMessage item={item} />;
    case "work":
      return <WorkGroup item={item} actions={actions} />;
    case "notice":
      return <Notice item={item} actions={actions} last={last} />;
    case "task":
      return <TaskCard item={item} />;
    case "verdict":
      return <VerdictCard item={item} />;
    case "footer":
      return <Footer item={item} />;
  }
}

/** The conversation, virtualized, following new output while pinned to the bottom. */
export function Transcript({
  items,
  actions,
  empty,
  sessionKey,
}: {
  items: Item[];
  actions: TranscriptActions;
  empty?: ReactNode;
  sessionKey: string;
}) {
  const scroller = useRef<HTMLDivElement>(null);
  const inner = useRef<HTMLDivElement>(null);
  const pinned = useRef(true);
  const [showJump, setShowJump] = useState(false);
  const virtualizer = useVirtualizer({
    count: items.length,
    getScrollElement: () => scroller.current,
    estimateSize: () => 96,
    overscan: 8,
    getItemKey: (index) => items[index]?.key ?? index,
  });

  const toBottom = useCallback(() => {
    const element = scroller.current;
    if (element) element.scrollTop = element.scrollHeight;
  }, []);

  // A newly opened session starts at its end.
  useLayoutEffect(() => {
    pinned.current = true;
    setShowJump(false);
    requestAnimationFrame(toBottom);
  }, [sessionKey, toBottom]);

  // Follow growth (new items, streamed text, measured heights) while pinned.
  useEffect(() => {
    const element = inner.current;
    if (!element) return;
    const observer = new ResizeObserver(() => {
      if (pinned.current) toBottom();
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, [toBottom]);

  useLayoutEffect(() => {
    if (pinned.current) toBottom();
  }, [items, toBottom]);

  const onScroll = () => {
    const element = scroller.current;
    if (!element) return;
    const atBottom = element.scrollHeight - element.scrollTop - element.clientHeight < 48;
    pinned.current = atBottom;
    setShowJump(!atBottom);
  };

  const virtual = virtualizer.getVirtualItems();
  return (
    <div className={styles.wrap}>
      <div ref={scroller} className={styles.scroller} onScroll={onScroll} role="log" aria-label="Conversation" aria-live="off">
        {items.length === 0 && empty}
        <div ref={inner} className={styles.inner} style={{ height: virtualizer.getTotalSize() }}>
          {virtual.map((row) => {
            const item = items[row.index];
            return (
              <div
                key={row.key}
                data-index={row.index}
                ref={virtualizer.measureElement}
                className={styles.item}
                data-kind={item.kind}
                style={{ transform: `translateY(${row.start}px)` }}
              >
                <ItemView item={item} actions={actions} last={row.index >= items.length - 2} />
              </div>
            );
          })}
        </div>
      </div>
      {showJump && (
        <button
          type="button"
          className={styles.jump}
          onClick={() => {
            pinned.current = true;
            toBottom();
          }}
          aria-label="Back to the latest message"
        >
          <ArrowDown size={16} aria-hidden />
        </button>
      )}
    </div>
  );
}
