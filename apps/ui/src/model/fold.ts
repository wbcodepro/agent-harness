// Folds a session's event stream into what the transcript shows: user
// messages, assistant text, work groups of reasoning and tool calls,
// notices, verdicts and turn footers; plus the data of the side panel
// (changes, screens, commands, subagents) and the live state (running,
// approvals, queued messages).
//
// The folder is fed one event at a time and mutates its state in place for
// speed, but every item it changes is replaced by a new object, so a view
// can compare items by identity to know what to repaint.

import { diffsFromToolInput, type FileDiff } from "./diff";
import { categoryOf, describe, exitCode, summarize, type Category } from "./tools";
import type {
  ApprovalEvent,
  ArtifactRef,
  Block,
  ModeName,
  SessionRecord,
  StreamEvent,
  TaskContract,
  TurnEndReason,
  Usage,
  VerificationReport,
} from "./types";

export interface ToolRow {
  kind: "tool";
  key: string;
  callId: string;
  name: string;
  category: Category;
  title: string;
  detail?: string;
  path?: string;
  input: unknown;
  output: string;
  images: ArtifactRef[];
  files: ArtifactRef[];
  status: "running" | "waiting" | "done" | "failed" | "interrupted";
  exitCode?: number;
  startTs: number;
  endTs?: number;
  diffs: FileDiff[];
}

export interface ReasoningRow {
  kind: "reasoning";
  key: string;
  text: string;
  streaming: boolean;
}

export type WorkRow = ToolRow | ReasoningRow;

export interface UserItem {
  kind: "user";
  key: string;
  text: string;
  images: ArtifactRef[];
  files: ArtifactRef[];
  /** Sent while the turn was running. */
  steering: boolean;
}

export interface AssistantItem {
  kind: "assistant";
  key: string;
  text: string;
  streaming: boolean;
  /** The response was cut short ("cancelled", "failed", "interrupted"). */
  cut?: string;
}

export interface WorkItem {
  kind: "work";
  key: string;
  turn: number;
  rows: WorkRow[];
  running: boolean;
  summary: string;
  /** The latest row's description, for the live header. */
  live: string;
  startTs: number;
  endTs?: number;
}

export interface NoticeItem {
  kind: "notice";
  key: string;
  tone: "info" | "warn" | "error";
  text: string;
  /** "continue": offer to continue after a failure. */
  action?: "continue";
}

export interface TaskItem {
  kind: "task";
  key: string;
  contract: TaskContract;
}

export interface VerdictItem {
  kind: "verdict";
  key: string;
  report: VerificationReport;
}

export interface FooterItem {
  kind: "footer";
  key: string;
  turn: number;
  durationMs: number;
  usage: Usage;
  reason: TurnEndReason;
}

export type Item = UserItem | AssistantItem | WorkItem | NoticeItem | TaskItem | VerdictItem | FooterItem;

export interface Change {
  key: string;
  turn: number;
  diff: FileDiff;
}

export interface Screen {
  key: string;
  artifact: ArtifactRef;
  tool: string;
  ts: number;
}

export interface Command {
  key: string;
  command: string;
  output: string;
  exitCode?: number;
  running: boolean;
}

export interface ViewState {
  items: Item[];
  cwd: string | null;
  running: boolean;
  approvals: ApprovalEvent[];
  queued: string[];
  lastSeq: number | null;
  /** Error of the last failed turn, for "Continue". */
  error: string | null;
  turn: number;
  usage: Usage;
  model: string | null;
  settings: { mode: ModeName; provider?: string | null; model?: string | null } | null;
  changes: Change[];
  screens: Screen[];
  commands: Command[];
  subagents: ToolRow[];
  touched: { path: string; ops: string[] }[];
  verification: VerificationReport | null;
  /** Bumped on every change, for subscribers. */
  version: number;
}

export function emptyUsage(): Usage {
  return { input_tokens: 0, output_tokens: 0, cache_read_tokens: 0, cache_write_tokens: 0 };
}

function addUsage(total: Usage, used: Partial<Usage> | undefined): Usage {
  if (!used) return total;
  return {
    input_tokens: total.input_tokens + (used.input_tokens ?? 0),
    output_tokens: total.output_tokens + (used.output_tokens ?? 0),
    cache_read_tokens: total.cache_read_tokens + (used.cache_read_tokens ?? 0),
    cache_write_tokens: total.cache_write_tokens + (used.cache_write_tokens ?? 0),
  };
}

export function textOf(content: Block[] | undefined): string {
  return (content ?? [])
    .filter((block): block is { type: "text"; text: string } => block.type === "text")
    .map((block) => block.text)
    .join("");
}

function outputOf(content: Block[] | undefined): string {
  const parts: string[] = [];
  for (const block of content ?? []) {
    if (block.type === "text") parts.push(block.text);
    else if (block.type === "data") parts.push(JSON.stringify(block.value, null, 2));
    else if (block.type === "unsupported") parts.push(`[${block.kind} content not carried: ${block.note}]`);
  }
  return parts.join("\n");
}

function media(content: Block[] | undefined): { images: ArtifactRef[]; files: ArtifactRef[] } {
  const images: ArtifactRef[] = [];
  const files: ArtifactRef[] = [];
  for (const block of content ?? []) {
    if (block.type === "image" && /^[0-9a-f]{64}$/.test(block.artifact?.sha256 ?? "")) images.push(block.artifact);
    if (block.type === "artifact" && /^[0-9a-f]{64}$/.test(block.artifact?.sha256 ?? "")) files.push(block.artifact);
  }
  return { images, files };
}

export class Folder {
  state: ViewState;
  private groups = new Map<string, number>(); // work item key -> index
  private rows = new Map<string, { group: string }>(); // call id -> group key
  private openGroup: string | null = null;
  private streamingText: number | null = null; // item index
  private streamingThought: { group: string; key: string } | null = null;
  private sawDeltas = false;
  private turnStart = 0;
  private turnUsage = emptyUsage();
  private turnOpen = false;
  private userInTurn = 0;
  private counter = 0;
  /** The "turn failed" notice of the last turn, which the error text replaces. */
  private failure: number | null = null;
  /** Time of the latest record. */
  private lastTs = 0;

  constructor() {
    this.state = {
      items: [],
      cwd: null,
      running: false,
      approvals: [],
      queued: [],
      lastSeq: null,
      error: null,
      turn: 0,
      usage: emptyUsage(),
      model: null,
      settings: null,
      changes: [],
      screens: [],
      commands: [],
      subagents: [],
      touched: [],
      verification: null,
      version: 0,
    };
  }

  private key(prefix: string): string {
    this.counter += 1;
    return `${prefix}${this.counter}`;
  }

  private push(item: Item): number {
    this.state.items.push(item);
    return this.state.items.length - 1;
  }

  private replace(index: number, change: (item: Item) => Item) {
    this.state.items[index] = change(this.state.items[index]);
  }

  private notice(tone: NoticeItem["tone"], text: string, action?: "continue") {
    this.closeGroup();
    this.push({ kind: "notice", key: this.key("n"), tone, text, action });
  }

  /** Ends the open group: a group that text or a message follows is finished and folds. */
  private closeGroup() {
    if (this.openGroup === null) return;
    const key = this.openGroup;
    this.openGroup = null;
    const index = this.groups.get(key);
    if (index === undefined || !(this.state.items[index] as WorkItem).running) return;
    const work = this.state.items[index] as WorkItem;
    const end = Math.max(this.lastTs, ...work.rows.map((row) => (row.kind === "tool" ? (row.endTs ?? 0) : 0)));
    this.updateGroup(
      key,
      (rows) => rows.map((row) => (row.kind === "reasoning" && row.streaming ? { ...row, streaming: false } : row)),
      { running: false, endTs: end },
    );
  }

  private group(ts: number): string {
    if (this.openGroup !== null) return this.openGroup;
    this.streamingText = null;
    const key = this.key("w");
    const index = this.push({
      kind: "work",
      key,
      turn: this.state.turn,
      rows: [],
      running: true,
      summary: "Working",
      live: "Working",
      startTs: ts,
    });
    this.groups.set(key, index);
    this.openGroup = key;
    return key;
  }

  /** Rebuilds a work item's rows through `change`, and its summary. */
  private updateGroup(key: string, change: (rows: WorkRow[]) => WorkRow[], extra?: Partial<WorkItem>) {
    const index = this.groups.get(key);
    if (index === undefined) return;
    this.replace(index, (item) => {
      const work = item as WorkItem;
      const rows = change(work.rows);
      const tools = rows.filter((row): row is ToolRow => row.kind === "tool");
      const thoughts = rows.length - tools.length;
      const last = rows[rows.length - 1];
      const live = last?.kind === "tool" ? [last.title, last.detail].filter(Boolean).join(" ") : "Thinking";
      return {
        ...work,
        rows,
        summary: summarize(
          tools.map((row) => ({ category: row.category, path: row.path, failed: row.status === "failed" })),
          thoughts,
        ),
        live,
        ...extra,
      };
    });
  }

  private updateRow(callId: string, change: (row: ToolRow) => ToolRow) {
    const where = this.rows.get(callId);
    if (!where) return;
    this.updateGroup(where.group, (rows) => rows.map((row) => (row.kind === "tool" && row.callId === callId ? change(row) : row)));
  }

  private touch(path: string, op: string) {
    const found = this.state.touched.find((entry) => entry.path === path);
    if (found) {
      if (!found.ops.includes(op)) found.ops = [...found.ops, op];
      this.state.touched = [...this.state.touched];
    } else {
      this.state.touched = [...this.state.touched, { path, ops: [op] }];
    }
  }

  /** Applies one event of the session's stream. */
  apply(event: StreamEvent) {
    switch (event.kind) {
      case "record":
        this.record(event.record);
        break;
      case "approval":
        if (!this.state.approvals.some((waiting) => waiting.id === event.id)) {
          this.state.approvals = [...this.state.approvals, event];
        }
        if (event.call_id) this.updateRow(event.call_id, (row) => ({ ...row, status: row.status === "running" ? "waiting" : row.status }));
        break;
      case "approval_resolved": {
        const resolved = this.state.approvals.find((waiting) => waiting.id === event.id);
        this.state.approvals = this.state.approvals.filter((waiting) => waiting.id !== event.id);
        if (resolved?.call_id) this.updateRow(resolved.call_id, (row) => ({ ...row, status: row.status === "waiting" ? "running" : row.status }));
        break;
      }
      case "turn":
        if (event.state === "started") {
          this.state.running = true;
          this.state.error = null;
        } else {
          this.state.running = false;
          this.state.approvals = [];
          if (event.error) {
            const error = event.error;
            const already = this.state.error === error;
            this.state.error = error;
            if (this.failure !== null) {
              this.replace(this.failure, (item) => ({ ...(item as NoticeItem), text: error }));
            } else if (!already) {
              this.notice("error", error, "continue");
              this.failure = this.state.items.length - 1;
            }
          }
        }
        break;
      case "steering":
        this.state.queued = event.queued;
        break;
      case "warning":
        this.notice("warn", event.text);
        break;
      case "settings":
        this.state.settings = { mode: event.mode, provider: event.provider, model: event.model };
        if (event.model) this.state.model = event.model;
        break;
      case "session":
        break;
    }
    this.state.version += 1;
  }

  private record(record: SessionRecord) {
    if (this.state.lastSeq !== null && record.seq <= this.state.lastSeq) return;
    this.state.lastSeq = record.seq;
    this.lastTs = record.ts;
    switch (record.type) {
      case "session":
        this.state.cwd = record.data.cwd;
        break;
      case "turn/start":
        this.state.turn = record.data.turn;
        this.turnStart = record.ts;
        this.turnUsage = emptyUsage();
        this.turnOpen = true;
        this.userInTurn = 0;
        this.failure = null;
        this.closeGroup();
        this.streamingText = null;
        this.streamingThought = null;
        break;
      case "turn/end":
        this.endTurn(record.data.reason, record.ts);
        break;
      case "user/message":
        this.userMessage(record.data.content);
        break;
      case "assistant/delta":
        this.delta(record);
        break;
      case "assistant/message":
        this.settle(record.data.content, record.ts);
        this.state.usage = addUsage(this.state.usage, record.data.usage);
        this.turnUsage = addUsage(this.turnUsage, record.data.usage);
        if (record.data.stop === "refusal") this.notice("warn", "The model declined to answer.");
        if (record.data.stop === "max_tokens") this.notice("warn", "The answer was cut off at the model's output limit.");
        break;
      case "assistant/attempt":
        if (this.streamingText !== null) {
          const reason = record.data.reason;
          this.replace(this.streamingText, (item) => ({ ...(item as AssistantItem), streaming: false, cut: reason }));
        }
        if (this.streamingThought) this.finishThought();
        this.streamingText = null;
        this.sawDeltas = false;
        break;
      case "tool/call":
        this.toolCall(record.data.call_id, record.data.name, record.data.input, record.ts);
        break;
      case "tool/result":
        this.toolResult(record.data.call_id, record.data.content, record.data.is_error ?? false, record.ts);
        break;
      case "compaction":
        this.state.usage = addUsage(this.state.usage, record.data.usage);
        this.turnUsage = addUsage(this.turnUsage, record.data.usage);
        this.notice("info", "Earlier messages were summarized to keep the context small.");
        break;
      case "model/switched":
        this.state.model = record.data.model;
        this.notice("info", `Model: ${record.data.model}`);
        break;
      case "task/start":
        this.closeGroup();
        this.push({ kind: "task", key: this.key("t"), contract: record.data.contract });
        break;
      case "task/verified":
        this.closeGroup();
        this.state.verification = record.data.report;
        this.push({ kind: "verdict", key: this.key("v"), report: record.data.report });
        break;
      default:
        break;
    }
  }

  private userMessage(content: Block[]) {
    this.closeGroup();
    this.streamingText = null;
    const text = textOf(content);
    if (text.startsWith("[hame verify]")) {
      this.notice("warn", "Checks failed; the details were sent back to the model.");
      return;
    }
    if (text.startsWith("[hame")) {
      this.notice("info", text.replace(/^\[hame[^\]]*\]\s*/, ""));
      return;
    }
    const { images, files } = media(content);
    this.push({
      kind: "user",
      key: this.key("u"),
      text,
      images,
      files,
      steering: this.turnOpen && this.userInTurn > 0,
    });
    this.userInTurn += 1;
  }

  private delta(record: Extract<SessionRecord, { type: "assistant/delta" }>) {
    const delta = record.data.delta;
    if (delta.type === "text") {
      this.sawDeltas = true;
      if (this.streamingThought) this.finishThought();
      if (this.streamingText === null) {
        this.closeGroup();
        this.streamingText = this.push({ kind: "assistant", key: this.key("a"), text: "", streaming: true });
      }
      this.replace(this.streamingText, (item) => ({ ...(item as AssistantItem), text: (item as AssistantItem).text + delta.text }));
    } else if (delta.type === "thinking") {
      this.sawDeltas = true;
      if (!this.streamingThought) {
        const group = this.group(record.ts);
        const key = this.key("r");
        this.streamingThought = { group, key };
        this.updateGroup(group, (rows) => [...rows, { kind: "reasoning", key, text: "", streaming: true }]);
      }
      const { group, key } = this.streamingThought;
      this.updateGroup(group, (rows) =>
        rows.map((row) => (row.kind === "reasoning" && row.key === key ? { ...row, text: row.text + delta.text } : row)),
      );
    } else if (delta.type === "tool_use_start") {
      this.sawDeltas = true;
      if (this.streamingThought) this.finishThought();
      if (this.streamingText !== null) {
        this.replace(this.streamingText, (item) => ({ ...(item as AssistantItem), streaming: false }));
        this.streamingText = null;
      }
    }
  }

  private finishThought() {
    if (!this.streamingThought) return;
    const { group, key } = this.streamingThought;
    this.updateGroup(group, (rows) => rows.map((row) => (row.kind === "reasoning" && row.key === key ? { ...row, streaming: false } : row)));
    this.streamingThought = null;
  }

  /** An `assistant/message` settles what streamed, or adds it when nothing did. */
  private settle(content: Block[], ts: number) {
    if (this.sawDeltas) {
      if (this.streamingThought) this.finishThought();
      if (this.streamingText !== null) {
        const final = textOf(content);
        this.replace(this.streamingText, (item) => ({
          ...(item as AssistantItem),
          text: final || (item as AssistantItem).text,
          streaming: false,
        }));
      }
    } else {
      for (const block of content) {
        if (block.type === "thinking" && block.text.trim()) {
          const group = this.group(ts);
          this.updateGroup(group, (rows) => [...rows, { kind: "reasoning", key: this.key("r"), text: block.text, streaming: false }]);
        } else if (block.type === "text" && block.text.trim()) {
          this.closeGroup();
          this.push({ kind: "assistant", key: this.key("a"), text: block.text, streaming: false });
        }
      }
    }
    this.streamingText = null;
    this.streamingThought = null;
    this.sawDeltas = false;
  }

  private toolCall(callId: string, name: string, input: unknown, ts: number) {
    if (this.streamingText !== null) {
      this.replace(this.streamingText, (item) => ({ ...(item as AssistantItem), streaming: false }));
      this.streamingText = null;
    }
    const group = this.group(ts);
    const described = describe(name, input);
    const row: ToolRow = {
      kind: "tool",
      key: callId,
      callId,
      name,
      category: categoryOf(name),
      title: described.title,
      detail: described.detail,
      path: described.path,
      input,
      output: "",
      images: [],
      files: [],
      status: "running",
      startTs: ts,
      diffs: diffsFromToolInput(name, input),
    };
    this.rows.set(callId, { group });
    this.updateGroup(group, (rows) => [...rows, row]);
    if (row.category === "run" && name === "bash") {
      const command = typeof (input as { command?: unknown })?.command === "string" ? (input as { command: string }).command : "";
      this.state.commands = [...this.state.commands, { key: callId, command, output: "", running: true }];
    }
    if (row.category === "subagent") this.state.subagents = [...this.state.subagents, row];
  }

  private toolResult(callId: string, content: Block[], isError: boolean, ts: number) {
    const output = outputOf(content);
    const { images, files } = media(content);
    const code = exitCode(output);
    let finished: ToolRow | null = null;
    this.updateRow(callId, (row) => {
      finished = {
        ...row,
        output,
        images,
        files,
        status: isError ? "failed" : "done",
        exitCode: code,
        endTs: ts,
      };
      return finished;
    });
    const row = finished as ToolRow | null;
    if (!row) return;
    if (!isError) {
      for (const diff of row.diffs) {
        this.state.changes = [...this.state.changes, { key: `${callId}:${diff.path}`, turn: this.state.turn, diff }];
        this.touch(diff.path, diff.op === "write" ? "wrote" : "edited");
      }
      if (row.category === "read" && row.path) this.touch(row.path, "read");
    }
    if (images.length) {
      this.state.screens = [...this.state.screens, ...images.map((artifact, n) => ({ key: `${callId}:${n}`, artifact, tool: row.name, ts }))];
    }
    if (row.name === "bash") {
      this.state.commands = this.state.commands.map((command) =>
        command.key === callId ? { ...command, output, exitCode: code, running: false } : command,
      );
    }
    if (row.category === "subagent") {
      this.state.subagents = this.state.subagents.map((agent) => (agent.callId === callId ? row : agent));
    }
  }

  private endTurn(reason: TurnEndReason, ts: number) {
    if (this.streamingThought) this.finishThought();
    if (this.streamingText !== null) {
      this.replace(this.streamingText, (item) => ({ ...(item as AssistantItem), streaming: false }));
    }
    // Every group of the turn stops; calls left running were cut off.
    for (const [key, index] of this.groups) {
      const item = this.state.items[index] as WorkItem;
      if (!item.running) continue;
      this.updateGroup(
        key,
        (rows) =>
          rows.map((row) =>
            row.kind === "tool" && (row.status === "running" || row.status === "waiting")
              ? { ...row, status: "interrupted", endTs: ts }
              : row.kind === "reasoning" && row.streaming
                ? { ...row, streaming: false }
                : row,
          ),
        { running: false, endTs: ts },
      );
    }
    this.state.commands = this.state.commands.map((command) => (command.running ? { ...command, running: false } : command));
    this.closeGroup();
    this.streamingText = null;
    this.streamingThought = null;
    this.sawDeltas = false;
    this.turnOpen = false;
    if (reason === "failed" || reason === "interrupted") {
      this.notice("error", reason === "failed" ? "The turn failed." : "The turn was interrupted when hame stopped.", "continue");
      this.failure = this.state.items.length - 1;
    } else if (reason === "cancelled") {
      this.notice("info", "Stopped.");
    }
    this.push({
      kind: "footer",
      key: this.key("f"),
      turn: this.state.turn,
      durationMs: Math.max(0, ts - this.turnStart),
      usage: this.turnUsage,
      reason,
    });
  }
}

/** Folds a whole list of events; for tests and for replays. */
export function foldAll(events: StreamEvent[]): ViewState {
  const folder = new Folder();
  for (const event of events) folder.apply(event);
  return folder.state;
}
