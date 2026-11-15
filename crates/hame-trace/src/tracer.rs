use std::fs::File;
use std::future::Future;
use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde_json::{Map, Value};

use crate::schema::{
    NestedRecord, NestedUsage, SCHEMA_VERSION, SpanKind, SpanRecord, SpanStart, SpanStatus,
    TraceHeader, TraceRecord,
};

/// Collects the records of one trace, in memory and, once a file is
/// attached, on disk. Cheap to clone; every clone writes to the same trace.
///
/// Telemetry never stops the work: a trace file that cannot be written is
/// reported once on standard error and the run carries on.
#[derive(Clone)]
pub struct Tracer(Arc<Inner>);

struct Inner {
    trace_id: String,
    origin: Instant,
    next_span: AtomicU64,
    write_failed: AtomicBool,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    records: Vec<TraceRecord>,
    file: Option<File>,
}

#[derive(Clone)]
struct Current {
    tracer: Tracer,
    span: u64,
}

tokio::task_local! {
    // The span the running code is inside of. Task-local and set per
    // future, so tool calls that run concurrently in one task each see
    // their own span.
    static CURRENT: Current;
}

impl Default for Tracer {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Tracer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tracer")
            .field("trace_id", &self.0.trace_id)
            .finish()
    }
}

impl Tracer {
    /// A new trace held in memory, with a fresh id.
    pub fn new() -> Self {
        Self::with_trace_id(uuid::Uuid::now_v7().simple().to_string())
    }

    pub fn with_trace_id(trace_id: impl Into<String>) -> Self {
        let trace_id = trace_id.into();
        let started_unix_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        let header = TraceRecord::Trace(TraceHeader {
            v: SCHEMA_VERSION,
            trace_id: trace_id.clone(),
            started_unix_ms,
            hame_version: env!("CARGO_PKG_VERSION").to_string(),
        });
        Self(Arc::new(Inner {
            trace_id,
            origin: Instant::now(),
            next_span: AtomicU64::new(1),
            write_failed: AtomicBool::new(false),
            state: Mutex::new(State {
                records: vec![header],
                file: None,
            }),
        }))
    }

    /// A new trace written to `path` as it happens.
    pub fn to_file(path: &Path) -> io::Result<Self> {
        let tracer = Self::new();
        tracer.attach_file(path)?;
        Ok(tracer)
    }

    /// Writes everything recorded so far to `path`, replacing what was
    /// there, and every later record as it happens. Each record is written
    /// whole with one call, so a crash leaves at most the last line cut.
    pub fn attach_file(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = File::create(path)?;
        let mut state = self.0.state.lock().expect("trace lock poisoned");
        for record in &state.records {
            file.write_all(line(record).as_bytes())?;
        }
        state.file = Some(file);
        Ok(())
    }

    pub fn trace_id(&self) -> &str {
        &self.0.trace_id
    }

    /// Every record so far, in the order written.
    pub fn records(&self) -> Vec<TraceRecord> {
        self.0
            .state
            .lock()
            .expect("trace lock poisoned")
            .records
            .clone()
    }

    /// Microseconds since the trace started, on the monotonic clock.
    pub fn now_us(&self) -> u64 {
        self.0.origin.elapsed().as_micros() as u64
    }

    /// Starts a span inside the span the calling code is running in (see
    /// [`Span::scope`]), or at the top of the trace.
    pub fn span(&self, kind: SpanKind, name: impl Into<String>) -> Span {
        let parent = CURRENT
            .try_with(|current| Arc::ptr_eq(&current.tracer.0, &self.0).then_some(current.span))
            .ok()
            .flatten();
        self.start(kind, name.into(), parent)
    }

    /// Starts a span at the top of the trace, whatever the caller is in.
    pub fn root_span(&self, kind: SpanKind, name: impl Into<String>) -> Span {
        self.start(kind, name.into(), None)
    }

    fn start(&self, kind: SpanKind, name: String, parent: Option<u64>) -> Span {
        let id = self.0.next_span.fetch_add(1, Ordering::Relaxed);
        let start_us = self.now_us();
        self.emit(TraceRecord::SpanStart(SpanStart {
            v: SCHEMA_VERSION,
            trace_id: self.0.trace_id.clone(),
            span_id: id,
            parent,
            kind,
            name: name.clone(),
            start_ms: ms(start_us),
        }));
        Span {
            tracer: self.clone(),
            id,
            parent,
            kind,
            name,
            start_us,
            attrs: Map::new(),
            ended: false,
        }
    }

    /// Records model use inside a component. `span` is where it ran.
    pub fn nested(&self, span: Option<u64>, component: impl Into<String>, usage: NestedUsage) {
        self.emit(TraceRecord::Nested(NestedRecord {
            v: SCHEMA_VERSION,
            trace_id: self.0.trace_id.clone(),
            span_id: span,
            component: component.into(),
            usage,
        }));
    }

    pub(crate) fn emit(&self, record: TraceRecord) {
        let mut state = self.0.state.lock().expect("trace lock poisoned");
        if let Some(file) = &mut state.file
            && let Err(e) = file.write_all(line(&record).as_bytes())
            && !self.0.write_failed.swap(true, Ordering::Relaxed)
        {
            eprintln!("hame: the trace could not be written ({e}); the run continues without it");
        }
        state.records.push(record);
    }
}

fn line(record: &TraceRecord) -> String {
    let mut text = serde_json::to_string(record).unwrap_or_default();
    text.push('\n');
    text
}

pub(crate) fn ms(us: u64) -> f64 {
    us as f64 / 1000.0
}

/// A span in progress. Ending it writes its record; dropping it unended
/// writes it as cancelled, which is what happens to the spans of a future
/// that is dropped mid-flight.
pub struct Span {
    tracer: Tracer,
    id: u64,
    parent: Option<u64>,
    kind: SpanKind,
    name: String,
    start_us: u64,
    attrs: Map<String, Value>,
    ended: bool,
}

impl Span {
    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn tracer(&self) -> &Tracer {
        &self.tracer
    }

    /// Adds an attribute. Only names, ids, sizes, counts and classes
    /// belong here: never raw inputs, outputs or error bodies.
    pub fn set(&mut self, key: &str, value: impl Into<Value>) {
        self.attrs.insert(key.to_string(), value.into());
    }

    /// Starts a span inside this one, whatever the caller is running in.
    pub fn child(&self, kind: SpanKind, name: impl Into<String>) -> Span {
        self.tracer.start(kind, name.into(), Some(self.id))
    }

    /// Runs `future` inside this span: spans started while it runs, in the
    /// same task, become this span's children.
    pub fn scope<F: Future>(&self, future: F) -> impl Future<Output = F::Output> {
        CURRENT.scope(
            Current {
                tracer: self.tracer.clone(),
                span: self.id,
            },
            future,
        )
    }

    pub fn end(mut self, status: SpanStatus) {
        self.finish(status);
    }

    pub fn end_ok(self) {
        self.end(SpanStatus::Ok);
    }

    fn finish(&mut self, status: SpanStatus) {
        if std::mem::replace(&mut self.ended, true) {
            return;
        }
        let end_us = self.tracer.now_us().max(self.start_us);
        self.tracer.emit(TraceRecord::Span(SpanRecord {
            v: SCHEMA_VERSION,
            trace_id: self.tracer.0.trace_id.clone(),
            span_id: self.id,
            parent: self.parent,
            kind: self.kind,
            name: std::mem::take(&mut self.name),
            start_ms: ms(self.start_us),
            dur_ms: ms(end_us - self.start_us),
            status,
            attrs: std::mem::take(&mut self.attrs),
        }));
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        self.finish(SpanStatus::Cancelled);
    }
}

/// The tracer and span the calling code runs in, if any.
pub fn current_span() -> Option<(Tracer, u64)> {
    CURRENT
        .try_with(|current| (current.tracer.clone(), current.span))
        .ok()
}

/// Lets a component that runs inside a traced tool call (an adapter, a
/// delegated agent) say what model use it had. Returns whether there was a
/// trace to record it in.
///
/// A component that cannot know must report [`NestedUsage::Unknown`]: the
/// run's usage is then marked incomplete instead of silently undercounted.
pub fn report_nested(component: &str, usage: NestedUsage) -> bool {
    match current_span() {
        Some((tracer, span)) => {
            tracer.nested(Some(span), component, usage);
            true
        }
        None => false,
    }
}
