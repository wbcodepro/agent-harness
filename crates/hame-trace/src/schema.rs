//! The trace file format, version 1.
//!
//! One JSON object per line, each with `v` (the schema version), `type` and
//! `trace_id`. A reader skips lines whose `type` it does not know, so later
//! versions can add record types without breaking older readers; a change to
//! the meaning of an existing field bumps the version and the file name.

use hame_session::Usage;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Version written in every record's `v` field.
pub const SCHEMA_VERSION: u32 = 1;
/// The file a trace is written to, beside a session log or in an eval run's
/// directory.
pub const TRACE_FILE: &str = "trace.v1.jsonl";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TraceRecord {
    /// Always the first record.
    Trace(TraceHeader),
    /// Written when a span begins, so a process that dies mid-span still
    /// leaves evidence of what it was doing.
    SpanStart(SpanStart),
    /// Written when a span ends.
    Span(SpanRecord),
    /// What one model request attempt used, as the provider reported it.
    Usage(UsageRecord),
    /// Model use inside a component (an adapter, a delegated agent) that
    /// did not go through a traced model.
    Nested(NestedRecord),
}

impl TraceRecord {
    pub fn trace_id(&self) -> &str {
        match self {
            TraceRecord::Trace(r) => &r.trace_id,
            TraceRecord::SpanStart(r) => &r.trace_id,
            TraceRecord::Span(r) => &r.trace_id,
            TraceRecord::Usage(r) => &r.trace_id,
            TraceRecord::Nested(r) => &r.trace_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraceHeader {
    pub v: u32,
    pub trace_id: String,
    /// Wall-clock start, milliseconds since the Unix epoch. Every other
    /// time in the trace is a monotonic offset from this moment.
    pub started_unix_ms: u64,
    /// The hame build that wrote the trace.
    pub hame_version: String,
}

/// What a span measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpanKind {
    /// A whole run (one eval attempt, one `hame run`).
    Run,
    /// One agent turn.
    Turn,
    /// One attempt at a model request.
    ModelRequest,
    /// One tool call.
    Tool,
    /// A call into an external adapter (browser, desktop, MCP server).
    Adapter,
    /// Capturing an observation (snapshot, screenshot, accessibility tree).
    Observe,
    /// Waiting: retry backoff, an event, a page load.
    Wait,
    /// Checking an outcome (a hidden checker, a postcondition).
    Verification,
    /// Recovering from a failure (reconnect, reconcile, resume).
    Recovery,
    /// Summarizing history to fit the context window.
    Compaction,
    /// Preparing an environment before the agent starts.
    Setup,
}

impl SpanKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SpanKind::Run => "run",
            SpanKind::Turn => "turn",
            SpanKind::ModelRequest => "model_request",
            SpanKind::Tool => "tool",
            SpanKind::Adapter => "adapter",
            SpanKind::Observe => "observe",
            SpanKind::Wait => "wait",
            SpanKind::Verification => "verification",
            SpanKind::Recovery => "recovery",
            SpanKind::Compaction => "compaction",
            SpanKind::Setup => "setup",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpanStatus {
    Ok,
    Error,
    /// Dropped before it finished: cancelled, or its process was stopped.
    Cancelled,
    Timeout,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpanStart {
    pub v: u32,
    pub trace_id: String,
    pub span_id: u64,
    pub parent: Option<u64>,
    pub kind: SpanKind,
    pub name: String,
    pub start_ms: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpanRecord {
    pub v: u32,
    pub trace_id: String,
    pub span_id: u64,
    pub parent: Option<u64>,
    pub kind: SpanKind,
    pub name: String,
    /// Monotonic offset from the trace start, in milliseconds (microsecond
    /// precision).
    pub start_ms: f64,
    pub dur_ms: f64,
    pub status: SpanStatus,
    /// Names, ids, sizes and classes only; never raw inputs or secrets.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub attrs: Map<String, Value>,
}

impl SpanRecord {
    pub fn end_ms(&self) -> f64 {
        self.start_ms + self.dur_ms
    }
}

/// One model request attempt. `usage` is `None` when what the attempt
/// consumed is unknown (it failed after the response had begun streaming);
/// a failure before any response is a known zero.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageRecord {
    pub v: u32,
    pub trace_id: String,
    /// The `model_request` span of this attempt.
    pub span_id: u64,
    /// Which of the harness's models: `main`, `summary`, `stronger`, ...
    pub role: String,
    /// The model the provider says answered, else the one configured.
    pub model: Option<String>,
    /// 1 for the first attempt of a call, 2 for its first retry, ...
    pub attempt: u32,
    /// `ok` or `error`.
    pub outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_class: Option<String>,
    /// Normalized usage: `input_tokens` is uncached input; cache reads,
    /// cache writes and output are separate and never overlap.
    pub usage: Option<Usage>,
    /// The provider's usage object exactly as received.
    pub raw_usage: Option<Value>,
    pub request_bytes: Option<u64>,
    pub response_bytes: Option<u64>,
    pub images: Option<u32>,
    pub image_bytes: Option<u64>,
    /// Time to first streamed delta as the adapter measured it.
    pub ttft_ms: Option<u64>,
    /// Time from the call to the first delta reaching the harness, as the
    /// decorator measured it (includes connection set-up).
    pub first_delta_ms: Option<f64>,
    pub stop: Option<String>,
}

/// Model use inside a component the harness does not see the requests of.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NestedUsage {
    /// The component made no model calls.
    None,
    /// The component reports what its model calls used.
    Reported {
        calls: u64,
        usage: Usage,
        #[serde(default)]
        model: Option<String>,
        /// What the component says it was billed, when it knows better than
        /// the tariff table (a delegated service with its own prices).
        #[serde(default)]
        cost_usd: Option<f64>,
    },
    /// The component may have used a model and cannot say how much. Any run
    /// with such a record has incomplete usage.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NestedRecord {
    pub v: u32,
    pub trace_id: String,
    /// The span the component ran in, when there was one.
    pub span_id: Option<u64>,
    pub component: String,
    pub usage: NestedUsage,
}
