//! Totals for one run, from its trace.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io;
use std::path::Path;

use hame_session::Usage;
use serde::{Deserialize, Serialize};

use crate::schema::{NestedUsage, SpanKind, SpanStatus, TraceRecord};
use crate::tariff::TariffTable;

/// What a run used. Token totals include nested model use; the nested part
/// is also given on its own. Fields that could not be observed are `None`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RunAccount {
    /// Model requests that completed.
    pub model_calls: u64,
    /// Every attempt, completed or failed.
    pub attempts: u64,
    pub failed_attempts: u64,
    /// Attempts sent after a failed attempt of the same model role, whether
    /// the decorator, the adapter or the agent loop sent them again.
    pub retries: u64,
    /// Model calls made inside adapters or delegated components that
    /// reported them.
    pub nested_model_calls: u64,
    /// False when any attempt's usage or any component's nested usage is
    /// unknown. Such a run must not support claims of total token use.
    pub usage_complete: bool,
    pub uncached_input: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub output: u64,
    /// The nested part of the totals above.
    pub nested_usage: Usage,
    /// What failed attempts consumed, where known (part of the totals).
    pub failed_attempt_usage: Usage,
    /// Reasoning tokens, when every completed attempt's provider exposed
    /// them. A subset of `output`, never added to it.
    pub reasoning: Option<u64>,
    /// Wire bytes, when known for every attempt.
    pub request_bytes: Option<u64>,
    pub response_bytes: Option<u64>,
    /// Attempts whose wire bytes are unknown.
    pub bytes_unknown_attempts: u64,
    /// Images sent to models and their encoded bytes, when known for every
    /// attempt.
    pub images: Option<u64>,
    pub image_bytes: Option<u64>,
    /// Images that tools produced (screenshots and the like).
    pub tool_images: u64,
    pub tool_calls: u64,
    pub failed_tool_calls: u64,
    /// Milliseconds per span kind, counting a span only when no enclosing
    /// span is of the same kind (a tool inside a script tool is not counted
    /// twice).
    pub stage_ms: BTreeMap<String, f64>,
    /// Spans that started and never ended: the process was stopped.
    pub interrupted_spans: u64,
    /// Total cost of every attempt, when every attempt is priced and its
    /// usage known.
    pub cost_usd: Option<f64>,
    /// The cost of the attempts that could be priced.
    pub priced_cost_usd: f64,
    /// Models that have no price in the tariff table.
    pub unpriced_models: Vec<String>,
    pub tariff_date: String,
    /// Models that answered, as reported or configured.
    pub models: Vec<String>,
    /// Median time to the first delta, as the decorator saw it.
    pub first_delta_ms_p50: Option<f64>,
}

/// Reads a trace file, skipping lines this build does not understand.
pub fn read_trace(path: &Path) -> io::Result<Vec<TraceRecord>> {
    let text = std::fs::read_to_string(path)?;
    Ok(text
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect())
}

fn add(total: &mut Usage, usage: &Usage) {
    total.input_tokens += usage.input_tokens;
    total.output_tokens += usage.output_tokens;
    total.cache_read_tokens += usage.cache_read_tokens;
    total.cache_write_tokens += usage.cache_write_tokens;
}

fn reasoning_of(raw: &serde_json::Value) -> Option<u64> {
    raw["output_tokens_details"]["reasoning_tokens"]
        .as_u64()
        .or_else(|| raw["completion_tokens_details"]["reasoning_tokens"].as_u64())
}

/// Totals for the run the records describe.
pub fn account(records: &[TraceRecord], tariffs: &TariffTable) -> RunAccount {
    let mut out = RunAccount {
        usage_complete: true,
        tariff_date: tariffs.date.clone(),
        ..RunAccount::default()
    };
    let mut total = Usage::default();
    let mut cost: Option<f64> = Some(0.0);
    let mut unpriced = BTreeSet::new();
    let mut models = BTreeSet::new();
    let (mut request_bytes, mut response_bytes) = (0u64, 0u64);
    let (mut images, mut image_bytes, mut images_known) = (0u64, 0u64, true);
    let mut reasoning: Option<u64> = Some(0);
    let mut last_failed: HashMap<String, bool> = HashMap::new();
    let mut first_deltas = Vec::new();

    for record in records {
        match record {
            TraceRecord::Usage(attempt) => {
                out.attempts += 1;
                let failed = attempt.outcome != "ok";
                if last_failed.insert(attempt.role.clone(), failed) == Some(true) {
                    out.retries += 1;
                }
                if failed {
                    out.failed_attempts += 1;
                } else {
                    out.model_calls += 1;
                }
                if let Some(model) = &attempt.model {
                    models.insert(model.clone());
                }
                if let Some(ms) = attempt.first_delta_ms {
                    first_deltas.push(ms);
                }
                match (attempt.request_bytes, attempt.response_bytes) {
                    (Some(sent), Some(received)) => {
                        request_bytes += sent;
                        response_bytes += received;
                    }
                    _ => out.bytes_unknown_attempts += 1,
                }
                match (attempt.images, attempt.image_bytes) {
                    (Some(n), Some(bytes)) => {
                        images += u64::from(n);
                        image_bytes += bytes;
                    }
                    _ => images_known = false,
                }
                if !failed {
                    reasoning = match (reasoning, attempt.raw_usage.as_ref().and_then(reasoning_of))
                    {
                        (Some(sum), Some(more)) => Some(sum + more),
                        _ => None,
                    };
                }
                let Some(usage) = &attempt.usage else {
                    out.usage_complete = false;
                    cost = None;
                    continue;
                };
                add(&mut total, usage);
                if failed {
                    add(&mut out.failed_attempt_usage, usage);
                }
                if *usage == Usage::default() {
                    continue;
                }
                let priced = attempt
                    .model
                    .as_deref()
                    .and_then(|model| tariffs.cost(model, usage, attempt.raw_usage.as_ref()));
                match priced {
                    Some(dollars) => {
                        out.priced_cost_usd += dollars;
                        cost = cost.map(|sum| sum + dollars);
                    }
                    None => {
                        unpriced.insert(attempt.model.clone().unwrap_or_else(|| "unknown".into()));
                        cost = None;
                    }
                }
            }
            TraceRecord::Nested(nested) => match &nested.usage {
                NestedUsage::None => {}
                NestedUsage::Unknown => {
                    out.usage_complete = false;
                    cost = None;
                }
                NestedUsage::Reported {
                    calls,
                    usage,
                    model,
                    cost_usd,
                } => {
                    out.nested_model_calls += calls;
                    add(&mut total, usage);
                    add(&mut out.nested_usage, usage);
                    let priced = cost_usd.or_else(|| {
                        model
                            .as_deref()
                            .and_then(|model| tariffs.cost(model, usage, None))
                    });
                    match priced {
                        Some(dollars) => {
                            out.priced_cost_usd += dollars;
                            cost = cost.map(|sum| sum + dollars);
                        }
                        None if *usage == Usage::default() => {}
                        None => {
                            unpriced
                                .insert(model.clone().unwrap_or_else(|| nested.component.clone()));
                            cost = None;
                        }
                    }
                }
            },
            _ => {}
        }
    }

    // Spans: stage times, tool counts, and starts that never ended.
    let mut kinds: HashMap<u64, (SpanKind, Option<u64>)> = HashMap::new();
    let mut ended = BTreeSet::new();
    for record in records {
        match record {
            TraceRecord::SpanStart(start) => {
                kinds.insert(start.span_id, (start.kind, start.parent));
            }
            TraceRecord::Span(span) => {
                kinds.insert(span.span_id, (span.kind, span.parent));
                ended.insert(span.span_id);
            }
            _ => {}
        }
    }
    let nested_in_same_kind = |mut parent: Option<u64>, kind: SpanKind| {
        let mut hops = 0;
        while let Some(id) = parent {
            let Some((parent_kind, grandparent)) = kinds.get(&id) else {
                return false;
            };
            if *parent_kind == kind {
                return true;
            }
            parent = *grandparent;
            hops += 1;
            if hops > 10_000 {
                return false;
            }
        }
        false
    };
    for record in records {
        match record {
            TraceRecord::Span(span) => {
                if span.kind == SpanKind::Tool {
                    out.tool_calls += 1;
                    if span.status != SpanStatus::Ok {
                        out.failed_tool_calls += 1;
                    }
                    out.tool_images += span
                        .attrs
                        .get("images")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                }
                if !nested_in_same_kind(span.parent, span.kind) {
                    *out.stage_ms
                        .entry(span.kind.as_str().to_string())
                        .or_default() += span.dur_ms;
                }
            }
            TraceRecord::SpanStart(start) if !ended.contains(&start.span_id) => {
                out.interrupted_spans += 1;
            }
            _ => {}
        }
    }
    for value in out.stage_ms.values_mut() {
        *value = (*value * 1000.0).round() / 1000.0;
    }

    out.uncached_input = total.input_tokens;
    out.cache_read = total.cache_read_tokens;
    out.cache_write = total.cache_write_tokens;
    out.output = total.output_tokens;
    out.reasoning = if out.model_calls > 0 { reasoning } else { None };
    if out.bytes_unknown_attempts == 0 {
        out.request_bytes = Some(request_bytes);
        out.response_bytes = Some(response_bytes);
    }
    if images_known {
        out.images = Some(images);
        out.image_bytes = Some(image_bytes);
    }
    out.cost_usd = cost;
    out.unpriced_models = unpriced.into_iter().collect();
    out.models = models.into_iter().collect();
    first_deltas.sort_by(f64::total_cmp);
    out.first_delta_ms_p50 =
        (!first_deltas.is_empty()).then(|| first_deltas[first_deltas.len() / 2]);
    out
}
