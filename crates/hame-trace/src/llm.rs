use std::sync::Arc;
use std::time::Instant;

use hame_kernel::BoxFuture;
use hame_llm::{Completion, DeltaSink, Llm, LlmError, Request, Retry, SinkError};
use hame_session::{Delta, StopReason, Usage};

use crate::schema::{SCHEMA_VERSION, SpanKind, SpanStatus, TraceRecord, UsageRecord};
use crate::tracer::Tracer;

/// A model with every request attempt traced.
///
/// Each call to the inner model is one `model_request` span and one usage
/// record, whether it succeeds or not. When given a [`Retry`] policy the
/// decorator also does the retrying, so that each retry is its own
/// attempt in the trace: build the inner adapter with `max_retries: 0`
/// then, or its own retries stay invisible inside one attempt. Only a
/// failure before any delta reached the caller is retried, as the
/// adapters do: what has been delivered cannot be taken back.
pub struct TracedLlm {
    inner: Arc<dyn Llm>,
    tracer: Tracer,
    role: String,
    model: Option<String>,
    retry: Option<Retry>,
}

impl TracedLlm {
    /// `role` says which of the harness's models this is (`main`,
    /// `summary`, `stronger`, ...).
    pub fn new(inner: Arc<dyn Llm>, tracer: Tracer, role: impl Into<String>) -> Self {
        Self {
            inner,
            tracer,
            role: role.into(),
            model: None,
            retry: None,
        }
    }

    /// The configured model name, used for pricing when the provider does
    /// not say which model answered.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    pub fn with_retry(mut self, retry: Retry) -> Self {
        self.retry = Some(retry);
        self
    }
}

/// Passes deltas on, noting how many there were and when the first came.
struct Counting<'a> {
    inner: &'a mut dyn DeltaSink,
    deltas: u64,
    first: Option<Instant>,
}

impl DeltaSink for Counting<'_> {
    fn delta(&mut self, delta: Delta) -> Result<(), SinkError> {
        self.first.get_or_insert_with(Instant::now);
        self.deltas += 1;
        self.inner.delta(delta)
    }
}

/// A short, secret-free name for what went wrong: the status code or the
/// kind of failure, never the provider's message.
pub fn error_class(error: &LlmError) -> String {
    if error.is_context_overflow() {
        return "context_overflow".into();
    }
    match error {
        LlmError::Http { status, .. } => format!("http_{status}"),
        LlmError::Transport(_) => "transport".into(),
        LlmError::Protocol(_) => "protocol".into(),
        LlmError::Sink(_) => "sink".into(),
    }
}

fn stop_name(stop: StopReason) -> &'static str {
    match stop {
        StopReason::EndTurn => "end_turn",
        StopReason::ToolUse => "tool_use",
        StopReason::MaxTokens => "max_tokens",
        StopReason::Refusal => "refusal",
    }
}

impl Llm for TracedLlm {
    fn stream<'a>(
        &'a self,
        request: &'a Request,
        sink: &'a mut dyn DeltaSink,
    ) -> BoxFuture<'a, Result<Completion, LlmError>> {
        Box::pin(async move {
            let mut attempt: u32 = 0;
            loop {
                attempt += 1;
                let mut span = self.tracer.span(SpanKind::ModelRequest, self.role.clone());
                span.set("role", self.role.clone());
                span.set("attempt", attempt);
                if let Some(model) = &self.model {
                    span.set("model", model.clone());
                }
                let started = Instant::now();
                let mut counting = Counting {
                    inner: &mut *sink,
                    deltas: 0,
                    first: None,
                };
                let result = span.scope(self.inner.stream(request, &mut counting)).await;
                let (deltas, first) = (counting.deltas, counting.first);
                let first_delta_ms =
                    first.map(|at| at.duration_since(started).as_micros() as f64 / 1000.0);
                span.set("deltas", deltas);
                let mut record = UsageRecord {
                    v: SCHEMA_VERSION,
                    trace_id: self.tracer.trace_id().to_string(),
                    span_id: span.id(),
                    role: self.role.clone(),
                    model: self.model.clone(),
                    attempt,
                    outcome: "ok".into(),
                    error_class: None,
                    usage: None,
                    raw_usage: None,
                    request_bytes: None,
                    response_bytes: None,
                    images: None,
                    image_bytes: None,
                    ttft_ms: None,
                    first_delta_ms,
                    stop: None,
                };
                match result {
                    Ok(completion) => {
                        let meta = &completion.meta;
                        record.model = meta.model.clone().or(record.model);
                        record.usage = Some(completion.usage);
                        record.raw_usage = meta.raw_usage.clone();
                        record.request_bytes = meta.request_bytes;
                        record.response_bytes = meta.response_bytes;
                        record.images = meta.images;
                        record.image_bytes = meta.image_bytes;
                        record.ttft_ms = meta.ttft_ms;
                        record.stop = Some(stop_name(completion.stop).into());
                        span.set("stop", stop_name(completion.stop));
                        self.tracer.emit(TraceRecord::Usage(record));
                        span.end_ok();
                        return Ok(completion);
                    }
                    Err(error) => {
                        let class = error_class(&error);
                        record.outcome = "error".into();
                        record.error_class = Some(class.clone());
                        // Nothing came back: the provider did not run the
                        // request, or did not bill for it. Once a response
                        // has begun, it may have been billed in part and the
                        // amount is unknown.
                        record.usage = (deltas == 0).then(Usage::default);
                        span.set("error_class", class);
                        if let LlmError::Http { status, .. } = &error {
                            span.set("http_status", *status);
                        }
                        self.tracer.emit(TraceRecord::Usage(record));
                        span.end(SpanStatus::Error);
                        let wait = match &self.retry {
                            Some(retry) if deltas == 0 => retry.delay(attempt - 1, &error),
                            _ => None,
                        };
                        let Some(wait) = wait else {
                            return Err(error);
                        };
                        let mut backoff = self.tracer.span(SpanKind::Wait, "retry_backoff");
                        backoff.set("delay_ms", wait.as_millis() as u64);
                        tokio::time::sleep(wait).await;
                        backoff.end_ok();
                    }
                }
            }
        })
    }
}
