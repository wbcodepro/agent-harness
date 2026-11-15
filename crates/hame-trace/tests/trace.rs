use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hame_agent::{ToolPipeline, ToolRequest};
use hame_kernel::BoxFuture;
use hame_llm::{CallMeta, Completion, DeltaSink, Llm, LlmError, Request, Retry};
use hame_session::{Delta, StopReason, Usage};
use hame_tools::{Effect, ToolOutput};
use hame_trace::{
    NestedUsage, SpanKind, SpanRecord, SpanStatus, TariffTable, TraceRecord, TracedLlm, Tracer,
    account, read_trace, report_nested, trace_layer,
};
use serde_json::{Value, json};

enum Step {
    Fail(LlmError),
    FailAfterDelta(LlmError),
    Ok {
        usage: Usage,
        raw: Option<Value>,
        model: Option<String>,
    },
}

struct Fake(Mutex<VecDeque<Step>>);

impl Fake {
    fn new(steps: Vec<Step>) -> Arc<Self> {
        Arc::new(Self(Mutex::new(steps.into())))
    }
}

impl Llm for Fake {
    fn stream<'a>(
        &'a self,
        _request: &'a Request,
        sink: &'a mut dyn DeltaSink,
    ) -> BoxFuture<'a, Result<Completion, LlmError>> {
        Box::pin(async move {
            let step = self
                .0
                .lock()
                .unwrap()
                .pop_front()
                .expect("script exhausted");
            tokio::time::sleep(Duration::from_millis(2)).await;
            match step {
                Step::Fail(error) => Err(error),
                Step::FailAfterDelta(error) => {
                    sink.delta(Delta::Text { text: "par".into() })
                        .map_err(LlmError::Sink)?;
                    Err(error)
                }
                Step::Ok { usage, raw, model } => {
                    sink.delta(Delta::Text {
                        text: "done".into(),
                    })
                    .map_err(LlmError::Sink)?;
                    Ok(Completion {
                        stop: StopReason::EndTurn,
                        usage,
                        meta: CallMeta {
                            model,
                            raw_usage: raw,
                            request_bytes: Some(1000),
                            response_bytes: Some(200),
                            images: Some(0),
                            image_bytes: Some(0),
                            ttft_ms: Some(1),
                        },
                    })
                }
            }
        })
    }
}

fn http(status: u16) -> LlmError {
    LlmError::Http {
        status,
        body: "provider says: key sk-ant-SECRET rejected".into(),
        retry_after: None,
    }
}

fn ok(input: u64, read: u64, write: u64, output: u64) -> Step {
    Step::Ok {
        usage: Usage {
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: read,
            cache_write_tokens: write,
        },
        raw: None,
        model: None,
    }
}

fn fast_retry() -> Retry {
    Retry {
        max_retries: 3,
        base_delay: Duration::from_millis(1),
    }
}

async fn call(llm: &dyn Llm) -> Result<Completion, LlmError> {
    let mut sink = |_delta: Delta| -> Result<(), hame_llm::SinkError> { Ok(()) };
    llm.stream(&Request::default(), &mut sink).await
}

fn spans(records: &[TraceRecord]) -> Vec<SpanRecord> {
    records
        .iter()
        .filter_map(|record| match record {
            TraceRecord::Span(span) => Some(span.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn retries_are_one_span_each_and_their_tokens_are_counted_once() {
    let tracer = Tracer::new();
    let fake = Fake::new(vec![
        Step::Fail(http(503)),
        Step::Fail(http(429)),
        ok(100, 50, 10, 20),
        ok(7, 0, 0, 3),
    ]);
    let llm = TracedLlm::new(fake, tracer.clone(), "main")
        .with_model("claude-haiku-5-5")
        .with_retry(fast_retry());
    call(&llm).await.unwrap();
    call(&llm).await.unwrap();

    let records = tracer.records();
    let requests: Vec<_> = spans(&records)
        .into_iter()
        .filter(|span| span.kind == SpanKind::ModelRequest)
        .collect();
    assert_eq!(requests.len(), 4, "one span per attempt");
    let statuses: Vec<_> = requests.iter().map(|span| span.status).collect();
    assert_eq!(
        statuses,
        [
            SpanStatus::Error,
            SpanStatus::Error,
            SpanStatus::Ok,
            SpanStatus::Ok
        ]
    );
    assert_eq!(requests[0].attrs["error_class"], "http_503");
    assert_eq!(requests[1].attrs["error_class"], "http_429");
    assert_eq!(requests[2].attrs["attempt"], 3);
    let waits = spans(&records)
        .into_iter()
        .filter(|span| span.kind == SpanKind::Wait)
        .count();
    assert_eq!(waits, 2, "a backoff span before each retry");

    let run = account(&records, &TariffTable::anthropic());
    assert_eq!(run.attempts, 4);
    assert_eq!(run.failed_attempts, 2);
    assert_eq!(run.retries, 2);
    assert_eq!(run.model_calls, 2);
    assert!(
        run.usage_complete,
        "failures before any response cost nothing"
    );
    assert_eq!(
        (
            run.uncached_input,
            run.cache_read,
            run.cache_write,
            run.output
        ),
        (107, 50, 10, 23)
    );
    assert_eq!(
        run.request_bytes, None,
        "failed attempts' bytes are unknown"
    );
    let expected = (107.0 * 0.10 + 50.0 * 0.01 + 10.0 * 0.125 + 23.0 * 0.50) / 1e6;
    assert!((run.cost_usd.unwrap() - expected).abs() < 1e-12);

    // The provider's error text, which here quotes a key, is not traced.
    let text = serde_json::to_string(&records).unwrap();
    assert!(!text.contains("sk-ant-SECRET"));
}

#[tokio::test]
async fn a_response_that_broke_midway_makes_usage_incomplete_and_is_not_retried_here() {
    let tracer = Tracer::new();
    let fake = Fake::new(vec![
        Step::FailAfterDelta(LlmError::Transport("reset".into())),
        ok(10, 0, 0, 5),
    ]);
    let llm = TracedLlm::new(fake, tracer.clone(), "main")
        .with_model("claude-haiku-5-5")
        .with_retry(fast_retry());
    assert!(call(&llm).await.is_err(), "deltas were delivered; no retry");
    // The agent loop sends it again itself.
    call(&llm).await.unwrap();

    let run = account(&tracer.records(), &TariffTable::anthropic());
    assert_eq!((run.attempts, run.failed_attempts, run.retries), (2, 1, 1));
    assert!(!run.usage_complete);
    assert_eq!(run.cost_usd, None, "part of the bill is unknown");
    assert!(run.priced_cost_usd > 0.0);
    assert_eq!(run.output, 5);
}

#[tokio::test]
async fn provider_usage_fixtures_are_not_double_counted() {
    // Anthropic reports uncached input, cache writes and cache reads
    // separately; the adapter normalizes them one to one.
    let anthropic = json!({
        "input_tokens": 1200, "cache_creation_input_tokens": 300,
        "cache_read_input_tokens": 5000, "output_tokens": 400,
        "cache_creation": {"ephemeral_5m_input_tokens": 300, "ephemeral_1h_input_tokens": 0}
    });
    // OpenAI-style input includes the cached part, and reasoning is part of
    // output; normalized, input is the uncached remainder.
    let openai = json!({
        "prompt_tokens": 6200, "prompt_tokens_details": {"cached_tokens": 5000},
        "completion_tokens": 400, "completion_tokens_details": {"reasoning_tokens": 150}
    });
    let normalized = Usage {
        input_tokens: 1200,
        output_tokens: 400,
        cache_read_tokens: 5000,
        cache_write_tokens: 300,
    };
    let tracer = Tracer::new();
    let fake = Fake::new(vec![Step::Ok {
        usage: normalized,
        raw: Some(anthropic.clone()),
        model: Some("claude-sonnet-5-5".into()),
    }]);
    call(&TracedLlm::new(fake, tracer.clone(), "main"))
        .await
        .unwrap();
    let run = account(&tracer.records(), &TariffTable::anthropic());
    assert_eq!(
        (
            run.uncached_input,
            run.cache_write,
            run.cache_read,
            run.output
        ),
        (1200, 300, 5000, 400)
    );
    assert_eq!(run.reasoning, None, "not exposed by this provider");
    let expected = (1200.0 * 2.0 + 300.0 * 2.5 + 5000.0 * 0.10 + 400.0 * 10.0) / 1e6;
    assert!((run.cost_usd.unwrap() - expected).abs() < 1e-12);
    // The raw object is kept as received, for audit.
    let raw = tracer.records().iter().find_map(|record| match record {
        TraceRecord::Usage(usage) => usage.raw_usage.clone(),
        _ => None,
    });
    assert_eq!(raw, Some(anthropic));

    let tracer = Tracer::new();
    let fake = Fake::new(vec![Step::Ok {
        usage: Usage {
            input_tokens: 1200,
            output_tokens: 400,
            cache_read_tokens: 5000,
            cache_write_tokens: 0,
        },
        raw: Some(openai),
        model: Some("gpt-x-mini".into()),
    }]);
    call(&TracedLlm::new(fake, tracer.clone(), "main"))
        .await
        .unwrap();
    let run = account(&tracer.records(), &TariffTable::anthropic());
    assert_eq!(
        run.uncached_input, 1200,
        "not 6200: cached input is separate"
    );
    assert_eq!(run.cache_read, 5000);
    assert_eq!(run.output, 400, "reasoning is inside output, not added");
    assert_eq!(run.reasoning, Some(150));
    assert_eq!(
        run.cost_usd, None,
        "an unknown model has no price, not zero"
    );
    assert_eq!(run.priced_cost_usd, 0.0);
    assert_eq!(run.unpriced_models, vec!["gpt-x-mini".to_string()]);
}

/// A tool that, like the script tool, sends calls of its own through the
/// pipeline it is part of.
async fn run_tool(pipeline: Arc<ToolPipeline>, name: &str, input: Value) -> ToolOutput {
    let request = ToolRequest {
        call_id: format!("{name}-1"),
        name: name.into(),
        input,
        effect: Effect::ReadOnly,
    };
    let inner_pipeline = Arc::clone(&pipeline);
    pipeline
        .run(request, move |request: ToolRequest| {
            let pipeline = Arc::clone(&inner_pipeline);
            async move {
                if request.name == "script" {
                    for call in 0..2 {
                        let inner = ToolRequest {
                            call_id: format!("script-{call}"),
                            name: "read".into(),
                            input: json!({"path": "a.txt"}),
                            effect: Effect::ReadOnly,
                        };
                        pipeline
                            .run(inner, |_| async {
                                tokio::time::sleep(Duration::from_millis(1)).await;
                                ToolOutput::ok("contents")
                            })
                            .await;
                    }
                    report_nested(
                        "fake-adapter",
                        NestedUsage::Reported {
                            calls: 2,
                            usage: Usage {
                                input_tokens: 40,
                                output_tokens: 4,
                                ..Usage::default()
                            },
                            model: Some("claude-haiku-5-5".into()),
                            cost_usd: None,
                        },
                    );
                    ToolOutput::ok("script done")
                } else {
                    ToolOutput::error("denied")
                }
            }
        })
        .await
}

#[tokio::test]
async fn spans_nest_under_the_code_that_started_them_with_monotonic_times() {
    let tracer = Tracer::new();
    let pipeline = Arc::new(ToolPipeline::new());
    pipeline.push(trace_layer(&tracer));
    let llm = TracedLlm::new(Fake::new(vec![ok(10, 0, 0, 1)]), tracer.clone(), "main")
        .with_model("claude-haiku-5-5");

    let turn = tracer.span(SpanKind::Turn, "turn");
    turn.scope(async {
        call(&llm).await.unwrap();
        run_tool(
            Arc::clone(&pipeline),
            "script",
            json!({"code": "print(open('.env').read())", "token": "sk-live-123"}),
        )
        .await;
        run_tool(
            Arc::clone(&pipeline),
            "bash",
            json!({"command": "rm -rf /"}),
        )
        .await;
    })
    .await;
    turn.end_ok();

    let records = tracer.records();
    let all = spans(&records);
    let by_id = |id: u64| all.iter().find(|span| span.span_id == id).unwrap();
    let turn = all.iter().find(|span| span.kind == SpanKind::Turn).unwrap();
    let script = all.iter().find(|span| span.name == "script").unwrap();
    let reads: Vec<_> = all.iter().filter(|span| span.name == "read").collect();
    let model = all
        .iter()
        .find(|span| span.kind == SpanKind::ModelRequest)
        .unwrap();
    let bash = all.iter().find(|span| span.name == "bash").unwrap();

    assert_eq!(turn.parent, None);
    assert_eq!(model.parent, Some(turn.span_id));
    assert_eq!(script.parent, Some(turn.span_id));
    assert_eq!(reads.len(), 2);
    assert!(reads.iter().all(|read| read.parent == Some(script.span_id)));
    assert_eq!(bash.status, SpanStatus::Error);
    // Times are whole microseconds shown as f64 milliseconds, and end_ms()
    // adds two of them: a span that ends in the same microsecond as its
    // parent (or as its next sibling starts) can come out one rounding step
    // later, so compare to well under a microsecond.
    const ROUNDING: f64 = 1e-6;
    for span in &all {
        assert!(span.dur_ms >= 0.0);
        if let Some(parent) = span.parent {
            let parent = by_id(parent);
            assert!(
                span.start_ms >= parent.start_ms,
                "{span:?} starts before {parent:?}"
            );
            assert!(
                span.end_ms() <= parent.end_ms() + ROUNDING,
                "{span:?} ends after {parent:?}"
            );
        }
    }
    // Siblings in order: each starts after the previous one ended.
    assert!(script.start_ms + ROUNDING >= model.end_ms());
    assert!(bash.start_ms + ROUNDING >= script.end_ms());
    assert!(reads[1].start_ms + ROUNDING >= reads[0].end_ms());

    let run = account(&records, &TariffTable::anthropic());
    assert_eq!(run.tool_calls, 4);
    assert_eq!(run.failed_tool_calls, 1);
    assert_eq!(run.model_calls, 1);
    assert_eq!(run.nested_model_calls, 2);
    assert_eq!(run.uncached_input, 50, "10 direct + 40 nested");
    assert_eq!(run.nested_usage.input_tokens, 40);
    assert!(run.usage_complete);
    // The script's own span covers its inner reads; they are not added
    // again to the tool stage time.
    assert!((run.stage_ms["tool"] - (script.dur_ms + bash.dur_ms)).abs() < 0.01);
    assert!(run.cost_usd.is_some());

    // Sizes are traced, contents are not.
    assert!(script.attrs["input_bytes"].as_u64().unwrap() > 10);
    let text = serde_json::to_string(&records).unwrap();
    assert!(!text.contains("sk-live-123") && !text.contains("rm -rf") && !text.contains(".env"));
}

#[tokio::test]
async fn an_unknown_nested_component_makes_usage_incomplete() {
    let tracer = Tracer::new();
    let span = tracer.span(SpanKind::Adapter, "desktop");
    span.scope(async {
        assert!(report_nested("desktop-agent", NestedUsage::Unknown));
    })
    .await;
    span.end_ok();
    assert!(
        !report_nested("outside", NestedUsage::None),
        "no trace here"
    );
    let run = account(&tracer.records(), &TariffTable::anthropic());
    assert!(!run.usage_complete);
    assert_eq!(run.cost_usd, None);
}

#[tokio::test]
async fn the_file_holds_every_record_and_interrupted_spans_are_visible() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run").join(hame_trace::TRACE_FILE);
    let tracer = Tracer::new();
    tracer.span(SpanKind::Setup, "before the file").end_ok();
    tracer.attach_file(&path).unwrap();
    {
        let _dropped = tracer.span(SpanKind::Tool, "cancelled");
    }
    let open = tracer.span(SpanKind::Verification, "never ends");
    std::mem::forget(open);

    let read = read_trace(&path).unwrap();
    assert_eq!(read, tracer.records());
    assert!(matches!(&read[0], TraceRecord::Trace(header) if header.v == 1));
    assert!(
        read.iter()
            .all(|record| record.trace_id() == tracer.trace_id())
    );
    let cancelled = spans(&read)
        .into_iter()
        .find(|span| span.name == "cancelled")
        .unwrap();
    assert_eq!(cancelled.status, SpanStatus::Cancelled);
    let run = account(&read, &TariffTable::anthropic());
    assert_eq!(run.interrupted_spans, 1);
    assert_eq!(run.cost_usd, Some(0.0), "no model use is a known zero");

    // Lines a newer writer adds are skipped, not fatal.
    let mut text = std::fs::read_to_string(&path).unwrap();
    text.push_str("{\"v\":2,\"type\":\"future_record\",\"trace_id\":\"x\"}\n");
    std::fs::write(&path, text).unwrap();
    assert_eq!(read_trace(&path).unwrap().len(), read.len());
}
