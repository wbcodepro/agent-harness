//! Run traces and resource accounting.
//!
//! A trace is a separate JSONL stream (`trace.v1.jsonl`) written beside a
//! session or an eval run. It never enters the model prompt and never holds
//! raw tool inputs or provider error bodies: spans carry names, ids, sizes,
//! timings and status; usage records carry the provider's token counts.
//!
//! - [`Tracer`]: the writer, and the source of correlated, monotonic spans.
//! - [`TracedLlm`]: an [`hame_llm::Llm`] decorator with one span and one
//!   usage record per attempt, failed attempts and retries included.
//! - [`trace_layer`]: a tool-pipeline layer with one span per tool call,
//!   including the inner calls a script or batch tool sends through the
//!   same pipeline.
//! - [`account()`]: totals for a run, with costs from a dated
//!   [`TariffTable`]. Unknown stays unknown: `None`, never zero.

mod account;
mod layer;
mod llm;
mod schema;
mod tariff;
mod tracer;

pub use account::{RunAccount, account, read_trace};
pub use layer::{TraceLayer, trace_layer};
pub use llm::{TracedLlm, error_class};
pub use schema::{
    NestedRecord, NestedUsage, SCHEMA_VERSION, SpanKind, SpanRecord, SpanStart, SpanStatus,
    TRACE_FILE, TraceHeader, TraceRecord, UsageRecord,
};
pub use tariff::{Rates, TARIFF_DATE, TARIFF_SOURCE, Tariff, TariffTable};
pub use tracer::{Span, Tracer, current_span, report_nested};
