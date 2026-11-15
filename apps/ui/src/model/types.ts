// Wire types of the hame web API: session records (crates/hame-session)
// and the other events of a session's stream (crates/hame-web).

export interface Usage {
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  cache_write_tokens: number;
}

export interface ArtifactRef {
  sha256: string;
  mime: string;
  bytes: number;
  width?: number;
  height?: number;
  name?: string;
  capture?: { target: string; ts: number };
}

export type Block =
  | { type: "text"; text: string }
  | { type: "thinking"; text: string; signature?: string }
  | { type: "tool_use"; id: string; name: string; input: unknown }
  | { type: "tool_result"; call_id: string; content: Block[]; is_error?: boolean }
  | { type: "fallback"; from: unknown; to: unknown }
  | { type: "image"; artifact: ArtifactRef }
  | { type: "artifact"; artifact: ArtifactRef }
  | { type: "data"; value: unknown }
  | { type: "unsupported"; kind: string; note: string };

export type Delta =
  | { type: "text"; text: string }
  | { type: "thinking"; text: string }
  | { type: "thinking_signature"; signature: string }
  | { type: "tool_use_start"; id: string; name: string }
  | { type: "tool_input"; json: string }
  | { type: "fallback"; from: unknown; to: unknown };

export type TurnEndReason = "completed" | "cancelled" | "failed" | "interrupted";
export type StopReason = "end_turn" | "tool_use" | "max_tokens" | "refusal";

export type Predicate = { kind: string } & Record<string, unknown>;

export interface PredicateResult {
  predicate: Predicate;
  passed: boolean;
  evidence: string;
}

export interface VerificationReport {
  task_id: string;
  passed: boolean;
  results: PredicateResult[];
  ms: number;
}

export interface TaskContract {
  id: string;
  outcome: string;
  constraints?: string[];
  acceptance: Predicate[];
}

interface R<T extends string, D> {
  seq: number;
  ts: number;
  type: T;
  data: D;
}

export type SessionRecord =
  | R<"session", { version: number; id: string; cwd: string }>
  | R<"turn/start", { turn: number }>
  | R<"turn/end", { turn: number; reason: TurnEndReason }>
  | R<"step/start", { turn: number; step: number }>
  | R<"step/end", { turn: number; step: number }>
  | R<"system/message", { text: string }>
  | R<"tools/declared", { tools: unknown[] }>
  | R<"user/message", { content: Block[] }>
  | R<"assistant/delta", { delta: Delta }>
  | R<"assistant/message", { content: Block[]; stop: StopReason; usage?: Partial<Usage> }>
  | R<"assistant/attempt", { content: Block[]; reason: "cancelled" | "failed" | "interrupted" }>
  | R<"tool/call", { call_id: string; name: string; input: unknown }>
  | R<"tool/result", { call_id: string; content: Block[]; is_error?: boolean }>
  | R<"compaction", { summary: string; up_to_seq: number; usage?: Partial<Usage> }>
  | R<"context/masked", { up_to_seq: number }>
  | R<"model/switched", { model: string }>
  | R<"checkpoint", { commit: string }>
  | R<"task/start", { contract: TaskContract }>
  | R<"action/receipt", { receipt: unknown }>
  | R<"task/verified", { report: VerificationReport }>
  | R<"mode/changed", { mode: ModeName; previous: ModeName; by: string; label?: string }>;

export interface ApprovalEvent {
  kind: "approval";
  id: number;
  question: string;
  tool?: string;
  /** The tool call asked about; an answer must name it. */
  tool_call_id?: string;
  /** sha256 of the arguments' canonical JSON (RFC 8785); an answer must name it. */
  args_sha256?: string | null;
  /** The exact arguments of the call. */
  arguments?: unknown;
  effect?: "read-only" | "write" | "execute" | "control";
  scope?: string;
}

export type ModeName = "default" | "read-only" | "ask" | "accept-edits" | "full-access";

export type StreamEvent =
  | { kind: "record"; record: SessionRecord }
  | ApprovalEvent
  | { kind: "approvals"; pending: ApprovalEvent[] }
  | { kind: "grants"; grants: string[] }
  | { kind: "approval_resolved"; id: number; allow: boolean }
  | { kind: "turn"; state: "started" | "ended"; error?: string | null }
  /** `origins[i]`: who queued `queued[i]`: null for this computer, else the remote device's label. */
  | { kind: "steering"; queued: string[]; origins?: (string | null)[] }
  | { kind: "warning"; text: string }
  | { kind: "settings"; mode: ModeName; provider?: string | null; model?: string | null }
  | { kind: "session"; session: SessionSummary };

export type SessionStatus = "running" | "needs_you" | "failed" | "verified" | "not_verified" | "idle";

export interface SessionSummary {
  id: string;
  cwd: string;
  title: string | null;
  named: boolean;
  pinned: boolean;
  status: SessionStatus;
  created: number;
  last_activity: number;
  turns: number;
  usage: Usage;
  mode: ModeName;
}

export interface SessionDetail extends SessionSummary {
  busy: boolean;
  provider: string | null;
  model: string | null;
  cost_usd: number | null;
  context: { tokens: number; compact_at: number | null };
  queued: string[];
  approvals: ApprovalEvent[];
  grants: string[];
}

export interface ModeInfo {
  name: ModeName;
  label: string;
  description: string;
}

export interface ServerInfo {
  version: string;
  cwd: string;
  folders: string[];
  modes: ModeInfo[];
  models: {
    fixed: boolean;
    current: { provider: string; model: string } | null;
    providers: { name: string; configured: boolean; model: string | null }[];
  };
  limits: { upload_bytes: number };
  warnings?: string[];
}
