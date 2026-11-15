use hame_agent::ToolRequest;
use hame_kernel::{BoxFuture, Layer, Next};
use hame_session::Block;
use hame_tools::{Effect, ToolOutput};

use crate::schema::{SpanKind, SpanStatus};
use crate::tracer::Tracer;

/// One `tool` span per call through the tool pipeline.
///
/// Push it first, so it is the outermost layer and its span covers policy,
/// approval and hooks as well as the tool. A script or batch tool sends its
/// inner calls through the same pipeline from inside its own call, so they
/// appear as children of its span.
///
/// The span records the tool name, call id, effect and the sizes of input
/// and output, never their content: inputs can carry secrets.
pub struct TraceLayer {
    tracer: Tracer,
}

pub fn trace_layer(tracer: &Tracer) -> TraceLayer {
    TraceLayer {
        tracer: tracer.clone(),
    }
}

impl Layer<ToolRequest, ToolOutput> for TraceLayer {
    fn call<'a>(
        &'a self,
        request: ToolRequest,
        next: Next<'a, ToolRequest, ToolOutput>,
    ) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            let mut span = self.tracer.span(SpanKind::Tool, request.name.clone());
            span.set("call_id", request.call_id.clone());
            span.set(
                "effect",
                match request.effect {
                    Effect::ReadOnly => "read_only",
                    Effect::Write => "write",
                    Effect::Execute => "execute",
                    Effect::Control => "control",
                },
            );
            span.set(
                "input_bytes",
                serde_json::to_vec(&request.input).map_or(0, |bytes| bytes.len()) as u64,
            );
            let output = span.scope(next.run(request)).await;
            span.set("output_bytes", output.text.len() as u64);
            if !output.parts.is_empty() {
                span.set("parts", output.parts.len() as u64);
                let images = output
                    .parts
                    .iter()
                    .filter(|part| matches!(part, Block::Image { .. }))
                    .count();
                if images > 0 {
                    span.set("images", images as u64);
                }
            }
            span.set("is_error", output.is_error);
            span.end(if output.is_error {
                SpanStatus::Error
            } else {
                SpanStatus::Ok
            });
            output
        })
    }
}
