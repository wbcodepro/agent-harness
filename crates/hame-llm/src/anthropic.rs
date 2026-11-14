use std::sync::atomic::{AtomicBool, Ordering};

use hame_session::{Block, Delta, Message, Role, StopReason, Usage};
use serde_json::{Value, json};

use crate::{BoxFuture, Completion, DeltaSink, Llm, LlmError, Request, Retry};

const API_VERSION: &str = "2026-10-21";
/// Room for a long answer; streaming keeps a large value from timing out.
const DEFAULT_MAX_TOKENS: u32 = 64_000;

/// Adapter for the Anthropic Messages API.
pub struct Anthropic {
    client: reqwest::Client,
    url: String,
    api_key: String,
    model: String,
    effort: Option<String>,
    /// Ask the API to continue on another model when this one declines.
    fallbacks: bool,
    retry: Retry,
    /// Request features this model turned out not to accept.
    unsupported: Unsupported,
    /// Set once the API refuses replayed thinking. It would refuse the
    /// same blocks on every later step, so they are no longer sent.
    thinking_rejected: AtomicBool,
}

/// Parts of a request that newer models take and older ones reject. Each
/// is sent until the API says the model does not support it, and left out
/// from then on, so an older model works without being listed anywhere.
#[derive(Default)]
struct Unsupported {
    effort: AtomicBool,
    eager_input: AtomicBool,
    request_cache: AtomicBool,
    fallbacks: AtomicBool,
}

impl Unsupported {
    /// Records what a 400 answer says the model does not take. Returns
    /// whether it named something that was being sent, which is the only
    /// case worth another attempt.
    fn learn(&self, error: &LlmError) -> bool {
        let LlmError::Http { status: 400, body } = error else {
            return false;
        };
        [
            ("effort", &self.effort),
            ("output_config", &self.effort),
            ("eager_input_streaming", &self.eager_input),
            ("cache_control", &self.request_cache),
            ("fallbacks", &self.fallbacks),
        ]
        .into_iter()
        .any(|(named, flag)| body.contains(named) && !flag.swap(true, Ordering::Relaxed))
    }
}

const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";

/// Models that accept `fallbacks: "default"`.
fn supports_fallbacks(model: &str) -> bool {
    [
        "claude-fable-5",
        "claude-mythos-5",
        "claude-opus-5",
        "claude-sonnet-5-5",
    ]
    .iter()
    .any(|family| model.starts_with(family))
}

impl Anthropic {
    /// `base_url` is the API root, normally `https://api.anthropic.com`.
    pub fn new(base_url: &str, api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            url: format!("{}/v1/messages", base_url.trim_end_matches('/')),
            api_key: api_key.into(),
            model: model.into(),
            effort: None,
            fallbacks: true,
            retry: Retry::default(),
            unsupported: Unsupported::default(),
            thinking_rejected: AtomicBool::new(false),
        }
    }

    /// How hard the model should work: `low`, `medium`, `high`, `xhigh` or
    /// `max`. `None` leaves the model's own default.
    pub fn with_effort(mut self, effort: Option<String>) -> Self {
        self.effort = effort.filter(|effort| !effort.is_empty());
        self
    }

    /// Whether a response the model declines may be continued by another
    /// model the provider recommends. On by default, for the models and
    /// endpoints that support it.
    pub fn with_fallbacks(mut self, fallbacks: bool) -> Self {
        self.fallbacks = fallbacks;
        self
    }

    fn uses_fallbacks(&self) -> bool {
        self.fallbacks && supports_fallbacks(&self.model)
    }

    pub fn with_client(mut self, client: reqwest::Client) -> Self {
        self.client = client;
        self
    }

    pub fn with_retry(mut self, retry: Retry) -> Self {
        self.retry = retry;
        self
    }

    async fn open(&self, body: &Value) -> Result<reqwest::Response, LlmError> {
        let mut request = self
            .client
            .post(&self.url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", API_VERSION)
            .header("content-type", "application/json");
        if self.uses_fallbacks() {
            request = request.header("anthropic-beta", FALLBACK_BETA);
        }
        let response = request
            .body(serde_json::to_vec(body).map_err(|e| LlmError::Protocol(e.to_string()))?)
            .send()
            .await
            .map_err(|e| LlmError::Transport(e.to_string()))?;
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let mut body = response.text().await.unwrap_or_default();
        if body.len() > 2000 {
            let mut end = 2000;
            while !body.is_char_boundary(end) {
                end -= 1;
            }
            body.truncate(end);
        }
        Err(LlmError::Http {
            status: status.as_u16(),
            body,
        })
    }

    async fn open_with_retry(&self, body: &Value) -> Result<reqwest::Response, LlmError> {
        let mut attempt = 0;
        loop {
            match self.open(body).await {
                Ok(response) => return Ok(response),
                Err(e) if e.is_retryable() && attempt < self.retry.max_retries => {
                    tokio::time::sleep(self.retry.base_delay * 2u32.pow(attempt)).await;
                    attempt += 1;
                }
                Err(e) => return Err(e),
            }
        }
    }
}

/// Whether an error says a replayed thinking block was not accepted.
fn rejects_thinking(error: &LlmError) -> bool {
    matches!(error, LlmError::Http { status: 400, body }
        if body.contains("thinking") && body.contains("signature"))
}

impl Llm for Anthropic {
    fn stream<'a>(
        &'a self,
        request: &'a Request,
        sink: &'a mut dyn DeltaSink,
    ) -> BoxFuture<'a, Result<Completion, LlmError>> {
        Box::pin(async move {
            let mut response = loop {
                let off = |flag: &AtomicBool| flag.load(Ordering::Relaxed);
                let with_thinking = !off(&self.thinking_rejected);
                let body = build_body(
                    &self.model,
                    self.effort
                        .as_deref()
                        .filter(|_| !off(&self.unsupported.effort)),
                    self.uses_fallbacks() && !off(&self.unsupported.fallbacks),
                    request,
                    with_thinking,
                    !off(&self.unsupported.eager_input),
                    !off(&self.unsupported.request_cache),
                );
                match self.open_with_retry(&body).await {
                    Ok(response) => break response,
                    // The history carries reasoning this conversation can
                    // no longer vouch for. Send it once more without any,
                    // and leave it out for the rest of the session; the
                    // model answers without that reasoning.
                    Err(e) if with_thinking && rejects_thinking(&e) => {
                        self.thinking_rejected.store(true, Ordering::Relaxed);
                    }
                    // An older model refused part of the request: leave
                    // that part out, now and for the rest of the session.
                    Err(e) if self.unsupported.learn(&e) => {}
                    Err(e) => return Err(e),
                }
            };

            let mut lines = SseData::default();
            let mut decoder = Decoder::default();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|e| LlmError::Transport(e.to_string()))?
            {
                for data in lines.push(&chunk) {
                    if decoder.feed(&data, sink)? {
                        return decoder.finish();
                    }
                }
            }
            decoder.finish()
        })
    }
}

fn text_of(blocks: &[Block]) -> String {
    let mut text = String::new();
    for block in blocks {
        if let Block::Text { text: part } = block {
            text.push_str(part);
        }
    }
    text
}

fn wire_blocks(message: &Message, with_thinking: bool) -> Vec<Value> {
    let mut out = Vec::new();
    for block in &message.content {
        match block {
            Block::Text { text } if !text.is_empty() => {
                out.push(json!({"type": "text", "text": text}));
            }
            Block::Text { .. } => {}
            // Only reasoning this API signed can be sent back to it.
            Block::Thinking {
                text,
                signature: Some(signature),
            } if with_thinking
                && message.role == Role::Assistant
                && !signature.starts_with(crate::responses::SIGNATURE_PREFIX) =>
            {
                out.push(json!({"type": "thinking", "thinking": text, "signature": signature}));
            }
            Block::Thinking { .. } => {}
            Block::ToolUse { id, name, input } => {
                // The API requires an object. A call whose arguments never
                // parsed is kept as a string in the log.
                let input = match input {
                    Value::Object(_) => input.clone(),
                    other => json!({"_unparsed_arguments": other}),
                };
                out.push(json!({"type": "tool_use", "id": id, "name": name, "input": input}));
            }
            // Goes back exactly where it appeared.
            Block::Fallback { from, to } if message.role == Role::Assistant => {
                out.push(json!({"type": "fallback", "from": from, "to": to}));
            }
            Block::Fallback { .. } => {}
            Block::ToolResult {
                call_id,
                content,
                is_error,
            } => {
                let mut result = json!({
                    "type": "tool_result",
                    "tool_use_id": call_id,
                    "content": text_of(content),
                });
                if *is_error {
                    result["is_error"] = json!(true);
                }
                out.push(result);
            }
        }
    }
    out
}

fn build_body(
    model: &str,
    effort: Option<&str>,
    fallbacks: bool,
    request: &Request,
    with_thinking: bool,
    eager_input: bool,
    request_cache: bool,
) -> Value {
    // Tool results travel in user turns, and adjacent turns of one role
    // are merged, so every tool result for an assistant turn lands in the
    // single user message that follows it.
    let mut messages: Vec<(&str, Vec<Value>)> = Vec::new();
    for message in &request.messages {
        let role = match message.role {
            Role::Assistant => "assistant",
            Role::User | Role::Tool => "user",
        };
        let blocks = wire_blocks(message, with_thinking);
        if blocks.is_empty() {
            continue;
        }
        match messages.last_mut() {
            Some((last_role, last_blocks)) if *last_role == role => last_blocks.extend(blocks),
            _ => messages.push((role, blocks)),
        }
    }

    let mut body = json!({
        "model": model,
        "max_tokens": request.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
        "stream": true,
        "messages": messages
            .into_iter()
            .map(|(role, content)| json!({"role": role, "content": content}))
            .collect::<Vec<_>>(),
    });
    if request_cache {
        // Caches the whole prefix up to the last block. Each request reads
        // the entry the previous one wrote, because history only grows.
        body["cache_control"] = json!({"type": "ephemeral"});
    } else if let Some(last) = body["messages"]
        .as_array_mut()
        .and_then(|messages| messages.last_mut())
        .and_then(|message| message["content"].as_array_mut())
        .and_then(|blocks| blocks.last_mut())
        .filter(|block| block["type"] != "thinking")
    {
        // The same thing said the older way: a mark on the last block of
        // the conversation, which moves forward with every request.
        last["cache_control"] = json!({"type": "ephemeral"});
    }
    if let Some(system) = &request.system {
        body["system"] = json!([{"type": "text", "text": system}]);
        // A model without request-level caching still caches up to a
        // marked block: the tools and the system prompt, which never
        // change within a session.
        if !request_cache {
            body["system"][0]["cache_control"] = json!({"type": "ephemeral"});
        }
    }
    if !request.tools.is_empty() {
        body["tools"] = request
            .tools
            .iter()
            .map(|tool| {
                let mut declared = json!({
                    "name": tool.name,
                    "description": tool.description,
                    "input_schema": tool.input_schema,
                });
                if eager_input {
                    // Stream arguments as they are written; each tool
                    // validates its own input before acting on it.
                    declared["eager_input_streaming"] = json!(true);
                }
                declared
            })
            .collect();
    }
    if let Some(effort) = effort {
        body["output_config"] = json!({"effort": effort});
    }
    if fallbacks {
        body["fallbacks"] = json!("default");
    }
    body
}

/// Collects the `data:` payloads of a server-sent-event stream.
#[derive(Default)]
struct SseData {
    buffer: Vec<u8>,
}

impl SseData {
    fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buffer.extend_from_slice(chunk);
        let mut payloads = Vec::new();
        while let Some(end) = self.buffer.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.buffer.drain(..=end).collect();
            let line = String::from_utf8_lossy(&line);
            if let Some(data) = line.trim_end_matches(['\r', '\n']).strip_prefix("data:") {
                payloads.push(data.trim_start().to_string());
            }
        }
        payloads
    }
}

#[derive(Default)]
struct Decoder {
    stop: Option<StopReason>,
    usage: Usage,
    done: bool,
}

impl Decoder {
    /// Handles one event. Returns true once the message is complete.
    fn feed(&mut self, data: &str, sink: &mut dyn DeltaSink) -> Result<bool, LlmError> {
        let event: Value = serde_json::from_str(data)
            .map_err(|e| LlmError::Protocol(format!("invalid stream event: {e}")))?;
        let mut emit = |delta: Delta| sink.delta(delta).map_err(LlmError::Sink);
        match event["type"].as_str() {
            Some("message_start") => self.read_usage(&event["message"]["usage"]),
            Some("content_block_start") => {
                let block = &event["content_block"];
                match block["type"].as_str() {
                    Some("tool_use") => emit(Delta::ToolUseStart {
                        id: block["id"].as_str().unwrap_or_default().into(),
                        name: block["name"].as_str().unwrap_or_default().into(),
                    })?,
                    Some("fallback") => emit(Delta::Fallback {
                        from: block["from"].clone(),
                        to: block["to"].clone(),
                    })?,
                    _ => {}
                }
            }
            Some("content_block_delta") => {
                let delta = &event["delta"];
                match delta["type"].as_str() {
                    Some("text_delta") => {
                        if let Some(text) = delta["text"].as_str().filter(|t| !t.is_empty()) {
                            emit(Delta::Text { text: text.into() })?;
                        }
                    }
                    Some("thinking_delta") => {
                        if let Some(text) = delta["thinking"].as_str().filter(|t| !t.is_empty()) {
                            emit(Delta::Thinking { text: text.into() })?;
                        }
                    }
                    Some("signature_delta") => {
                        if let Some(signature) = delta["signature"].as_str() {
                            emit(Delta::ThinkingSignature {
                                signature: signature.into(),
                            })?;
                        }
                    }
                    Some("input_json_delta") => {
                        if let Some(json) = delta["partial_json"].as_str().filter(|j| !j.is_empty())
                        {
                            emit(Delta::ToolInput { json: json.into() })?;
                        }
                    }
                    _ => {}
                }
            }
            Some("message_delta") => {
                self.read_usage(&event["usage"]);
                if let Some(reason) = event["delta"]["stop_reason"].as_str() {
                    self.stop = Some(match reason {
                        "tool_use" => StopReason::ToolUse,
                        "max_tokens" | "model_context_window_exceeded" => StopReason::MaxTokens,
                        "refusal" => StopReason::Refusal,
                        // end_turn, stop_sequence, pause_turn
                        _ => StopReason::EndTurn,
                    });
                }
            }
            Some("message_stop") => {
                self.done = true;
                return Ok(true);
            }
            Some("error") => {
                return Err(LlmError::Protocol(format!(
                    "the provider reported an error mid-stream: {}",
                    event["error"]
                )));
            }
            // ping, content_block_stop, and event types added later
            _ => {}
        }
        Ok(false)
    }

    /// `input_tokens` already excludes cached tokens in this API.
    fn read_usage(&mut self, usage: &Value) {
        let read = |name: &str| usage[name].as_u64();
        if let Some(n) = read("input_tokens") {
            self.usage.input_tokens = n;
        }
        if let Some(n) = read("output_tokens") {
            self.usage.output_tokens = n;
        }
        if let Some(n) = read("cache_read_input_tokens") {
            self.usage.cache_read_tokens = n;
        }
        if let Some(n) = read("cache_creation_input_tokens") {
            self.usage.cache_write_tokens = n;
        }
    }

    fn finish(self) -> Result<Completion, LlmError> {
        match (self.done, self.stop) {
            (true, Some(stop)) => Ok(Completion {
                stop,
                usage: self.usage,
            }),
            _ => Err(LlmError::Protocol(
                "the stream ended before the message was complete".into(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::serve;
    use hame_session::{ToolSpec, assemble};
    use std::time::Duration;

    fn adapter(base_url: &str) -> Anthropic {
        Anthropic::new(base_url, "test-key", "test-model")
            .with_effort(Some("high".into()))
            .with_client(reqwest::Client::builder().no_proxy().build().unwrap())
            .with_retry(Retry {
                max_retries: 1,
                base_delay: Duration::from_millis(1),
            })
    }

    fn sse(events: &[Value]) -> String {
        events
            .iter()
            .map(|event| {
                format!(
                    "event: {}\ndata: {event}\n\n",
                    event["type"].as_str().unwrap()
                )
            })
            .collect()
    }

    fn request() -> Request {
        Request {
            system: Some("You are hame.".into()),
            tools: vec![ToolSpec {
                name: "read".into(),
                description: "Read a file.".into(),
                input_schema: json!({"type": "object"}),
            }],
            messages: vec![
                Message {
                    role: Role::User,
                    content: vec![Block::text("Show a.rs")],
                },
                Message {
                    role: Role::Assistant,
                    content: vec![
                        Block::Thinking {
                            text: String::new(),
                            signature: Some("sig-a".into()),
                        },
                        // Reasoning from another provider has no signature
                        // and must not be sent.
                        Block::Thinking {
                            text: "foreign".into(),
                            signature: None,
                        },
                        Block::ToolUse {
                            id: "t1".into(),
                            name: "read".into(),
                            input: json!({"path": "a.rs"}),
                        },
                        Block::ToolUse {
                            id: "t2".into(),
                            name: "read".into(),
                            input: json!("{\"path\": \"b"),
                        },
                    ],
                },
                Message {
                    role: Role::Tool,
                    content: vec![
                        Block::ToolResult {
                            call_id: "t1".into(),
                            content: vec![Block::text("fn a() {}")],
                            is_error: false,
                        },
                        Block::ToolResult {
                            call_id: "t2".into(),
                            content: vec![Block::text("Invalid input")],
                            is_error: true,
                        },
                    ],
                },
                Message {
                    role: Role::User,
                    content: vec![Block::text("[hame verify] checks fail")],
                },
            ],
            max_tokens: None,
            cache_key: None,
        }
    }

    async fn collect(
        adapter: &Anthropic,
        request: &Request,
    ) -> (Vec<Delta>, Result<Completion, LlmError>) {
        let mut deltas = Vec::new();
        let mut sink = |delta: Delta| {
            deltas.push(delta);
            Ok(())
        };
        let result = adapter.stream(request, &mut sink).await;
        (deltas, result)
    }

    fn answer() -> String {
        let delta = |index: u64, delta: Value| json!({"type": "content_block_delta", "index": index, "delta": delta});
        sse(&[
            json!({"type": "message_start", "message": {"usage": {
                "input_tokens": 12, "cache_read_input_tokens": 900,
                "cache_creation_input_tokens": 40, "output_tokens": 1
            }}}),
            json!({"type": "ping"}),
            json!({"type": "content_block_start", "index": 0,
                "content_block": {"type": "thinking", "thinking": "", "signature": ""}}),
            delta(
                0,
                json!({"type": "signature_delta", "signature": "sig-new"}),
            ),
            json!({"type": "content_block_stop", "index": 0}),
            json!({"type": "content_block_start", "index": 1,
                "content_block": {"type": "text", "text": ""}}),
            delta(1, json!({"type": "text_delta", "text": "Reading "})),
            delta(1, json!({"type": "text_delta", "text": "b.rs."})),
            json!({"type": "content_block_stop", "index": 1}),
            json!({"type": "content_block_start", "index": 2,
                "content_block": {"type": "tool_use", "id": "t3", "name": "read", "input": {}}}),
            delta(2, json!({"type": "input_json_delta", "partial_json": ""})),
            delta(
                2,
                json!({"type": "input_json_delta", "partial_json": "{\"path\": \"b"}),
            ),
            delta(
                2,
                json!({"type": "input_json_delta", "partial_json": ".rs\"}"}),
            ),
            json!({"type": "content_block_stop", "index": 2}),
            json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"},
                "usage": {"output_tokens": 57}}),
            json!({"type": "message_stop"}),
        ])
    }

    #[tokio::test]
    async fn streams_a_response_and_sends_history_in_the_wire_shape() {
        let (base_url, bodies) = serve(vec![(200, answer())]).await;
        let (deltas, result) = collect(&adapter(&base_url), &request()).await;

        assert_eq!(
            result.unwrap(),
            Completion {
                stop: StopReason::ToolUse,
                usage: Usage {
                    input_tokens: 12,
                    output_tokens: 57,
                    cache_read_tokens: 900,
                    cache_write_tokens: 40,
                },
            }
        );
        assert_eq!(
            assemble(&deltas),
            vec![
                Block::Thinking {
                    text: String::new(),
                    signature: Some("sig-new".into()),
                },
                Block::text("Reading b.rs."),
                Block::ToolUse {
                    id: "t3".into(),
                    name: "read".into(),
                    input: json!({"path": "b.rs"}),
                },
            ]
        );

        let sent = bodies.lock().unwrap()[0].clone();
        assert_eq!(
            sent,
            json!({
                "model": "test-model",
                "max_tokens": 64000,
                "stream": true,
                "cache_control": {"type": "ephemeral"},
                "output_config": {"effort": "high"},
                "system": [{"type": "text", "text": "You are hame."}],
                "tools": [{"name": "read", "description": "Read a file.",
                    "input_schema": {"type": "object"}, "eager_input_streaming": true}],
                "messages": [
                    {"role": "user", "content": [{"type": "text", "text": "Show a.rs"}]},
                    {"role": "assistant", "content": [
                        {"type": "thinking", "thinking": "", "signature": "sig-a"},
                        {"type": "tool_use", "id": "t1", "name": "read", "input": {"path": "a.rs"}},
                        {"type": "tool_use", "id": "t2", "name": "read",
                            "input": {"_unparsed_arguments": "{\"path\": \"b"}},
                    ]},
                    // Both results and the follow-up text share one user turn.
                    {"role": "user", "content": [
                        {"type": "tool_result", "tool_use_id": "t1", "content": "fn a() {}"},
                        {"type": "tool_result", "tool_use_id": "t2", "content": "Invalid input",
                            "is_error": true},
                        {"type": "text", "text": "[hame verify] checks fail"},
                    ]},
                ],
            })
        );
    }

    #[tokio::test]
    async fn a_declined_response_continues_on_the_fallback_model() {
        let stream = sse(&[
            json!({"type": "message_start", "message": {"usage": {"input_tokens": 9}}}),
            json!({"type": "content_block_start", "index": 0,
                "content_block": {"type": "tool_use", "id": "void", "name": "read", "input": {}}}),
            json!({"type": "content_block_stop", "index": 0}),
            json!({"type": "content_block_start", "index": 1, "content_block": {
                "type": "fallback", "from": {"model": "claude-opus-5-5"}, "to": {"model": "claude-opus-4-8"}}}),
            json!({"type": "content_block_stop", "index": 1}),
            json!({"type": "content_block_start", "index": 2, "content_block": {"type": "text", "text": ""}}),
            json!({"type": "content_block_delta", "index": 2, "delta": {"type": "text_delta", "text": "Here."}}),
            json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 3}}),
            json!({"type": "message_stop"}),
        ]);
        let (base_url, bodies) = serve(vec![(200, stream.clone()), (200, stream)]).await;
        let adapter = Anthropic::new(&base_url, "k", "claude-opus-5-5")
            .with_client(reqwest::Client::builder().no_proxy().build().unwrap());

        let (deltas, result) = collect(&adapter, &request()).await;
        assert_eq!(result.unwrap().stop, StopReason::EndTurn);
        let handover = Block::Fallback {
            from: json!({"model": "claude-opus-5-5"}),
            to: json!({"model": "claude-opus-4-8"}),
        };
        // The declined model's tool call is gone; nothing will run it.
        let content = assemble(&deltas);
        assert_eq!(content, vec![handover, Block::text("Here.")]);
        assert_eq!(bodies.lock().unwrap()[0]["fallbacks"], "default");

        // On the next request the handover goes back in place.
        let mut next = request();
        next.messages.push(Message {
            role: Role::Assistant,
            content,
        });
        next.messages.push(Message {
            role: Role::User,
            content: vec![Block::text("Thanks")],
        });
        collect(&adapter, &next).await.1.unwrap();
        let sent = bodies.lock().unwrap()[1].clone();
        let echoed = &sent["messages"][3]["content"];
        assert_eq!(
            echoed[0],
            json!({"type": "fallback", "from": {"model": "claude-opus-5-5"}, "to": {"model": "claude-opus-4-8"}})
        );
        assert_eq!(echoed[1]["text"], "Here.");

        // Models without server-side fallback are not sent the parameter.
        assert!(!supports_fallbacks("claude-haiku-5-5") && !supports_fallbacks("claude-sonnet-5"));
        assert!(supports_fallbacks("claude-fable-5-2") && supports_fallbacks("claude-sonnet-5-5"));
    }

    #[tokio::test]
    async fn rejected_thinking_is_retried_without_it_and_not_sent_again() {
        let rejection = json!({"type": "error", "error": {"type": "invalid_request_error",
            "message": "messages.1.content.0: Invalid `signature` in `thinking` block. The block is bound to a different conversation."}})
        .to_string();
        let (base_url, bodies) =
            serve(vec![(400, rejection), (200, answer()), (200, answer())]).await;
        let model = adapter(&base_url);
        let (_, result) = collect(&model, &request()).await;
        assert_eq!(result.unwrap().stop, StopReason::ToolUse);
        // The next step does not offer the thinking again.
        let (_, result) = collect(&model, &request()).await;
        assert_eq!(result.unwrap().stop, StopReason::ToolUse);

        let bodies = bodies.lock().unwrap();
        assert_eq!(bodies.len(), 3);
        let has_thinking = |body: &Value| body.to_string().contains("\"type\":\"thinking\"");
        assert!(bodies.iter().map(has_thinking).eq([true, false, false]));
    }

    #[tokio::test]
    async fn what_an_older_model_does_not_accept_is_left_out_from_then_on() {
        let refusal = |message: &str| {
            json!({"type": "error", "error": {"type": "invalid_request_error", "message": message}})
                .to_string()
        };
        let (base_url, bodies) = serve(vec![
            (
                400,
                refusal("This model does not support the effort parameter."),
            ),
            (
                400,
                refusal("tools.0.custom.eager_input_streaming: Extra inputs are not permitted"),
            ),
            (
                400,
                refusal("cache_control: Extra inputs are not permitted"),
            ),
            (200, answer()),
            (200, answer()),
        ])
        .await;
        let model = adapter(&base_url).with_effort(Some("high".into()));
        let (_, result) = collect(&model, &request()).await;
        assert_eq!(result.unwrap().stop, StopReason::ToolUse);
        // The next request does not try them again.
        let (_, result) = collect(&model, &request()).await;
        assert!(result.is_ok());

        let bodies = bodies.lock().unwrap();
        assert_eq!(bodies.len(), 5);
        assert_eq!(bodies[0]["output_config"]["effort"], "high");
        assert!(bodies[1].get("output_config").is_none());
        assert_eq!(bodies[1]["tools"][0]["eager_input_streaming"], true);
        assert!(bodies[2]["tools"][0].get("eager_input_streaming").is_none());
        assert!(bodies[2].get("cache_control").is_some());
        for sent in &bodies[3..] {
            assert!(sent.get("output_config").is_none());
            assert!(sent.get("cache_control").is_none());
            assert!(sent["tools"][0].get("eager_input_streaming").is_none());
            // Caching is asked for the older way: on the fixed part of the
            // prompt and on the last block of the conversation.
            assert_eq!(sent["system"][0]["cache_control"]["type"], "ephemeral");
            let last = sent["messages"].as_array().unwrap().last().unwrap();
            let block = last["content"].as_array().unwrap().last().unwrap();
            assert_eq!(block["cache_control"]["type"], "ephemeral");
        }
    }

    #[tokio::test]
    async fn a_refusal_that_names_nothing_we_sent_is_not_retried() {
        let unrelated = json!({"type": "error", "error": {"type": "invalid_request_error",
            "message": "max_tokens: must be at most 64000"}})
        .to_string();
        let (base_url, bodies) = serve(vec![(400, unrelated), (200, answer())]).await;
        let (_, result) = collect(&adapter(&base_url), &request()).await;
        assert!(matches!(result, Err(LlmError::Http { status: 400, .. })));
        assert_eq!(bodies.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn refusals_overload_and_cut_streams_are_distinguished() {
        let refusal = sse(&[
            json!({"type": "message_start", "message": {"usage": {"input_tokens": 5}}}),
            json!({"type": "message_delta", "delta": {"stop_reason": "refusal"}, "usage": {"output_tokens": 0}}),
            json!({"type": "message_stop"}),
        ]);
        let (base_url, _) = serve(vec![(529, "overloaded".into()), (200, refusal)]).await;
        let (deltas, result) = collect(&adapter(&base_url), &request()).await;
        assert!(deltas.is_empty());
        assert_eq!(result.unwrap().stop, StopReason::Refusal);

        let cut = sse(&[
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "par"}}),
        ]);
        let (base_url, _) = serve(vec![(200, cut)]).await;
        let (deltas, result) = collect(&adapter(&base_url), &request()).await;
        assert_eq!(deltas, vec![Delta::Text { text: "par".into() }]);
        assert!(matches!(result, Err(LlmError::Protocol(_))));

        let failed = sse(&[
            json!({"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}}),
        ]);
        let (base_url, _) = serve(vec![(200, failed)]).await;
        let (_, result) = collect(&adapter(&base_url), &request()).await;
        assert!(matches!(result, Err(LlmError::Protocol(m)) if m.contains("Overloaded")));

        let (base_url, bodies) = serve(vec![(401, "bad key".into())]).await;
        let (_, result) = collect(&adapter(&base_url), &request()).await;
        assert!(matches!(result, Err(LlmError::Http { status: 401, .. })));
        assert_eq!(bodies.lock().unwrap().len(), 1);
    }
}
