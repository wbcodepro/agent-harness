use std::sync::atomic::{AtomicBool, Ordering};

use hame_session::{Block, Delta, Message, Role, StopReason, Usage};
use serde_json::{Value, json};

use crate::{BoxFuture, Completion, DeltaSink, Llm, LlmError, Request, Retry};

/// Marks a thinking signature as an OpenAI reasoning item, so it is only
/// ever sent back to this API.
pub(crate) const SIGNATURE_PREFIX: &str = "openai:";

/// Adapter for the OpenAI Responses API, which current OpenAI models need
/// for tool use together with reasoning.
///
/// Requests are stateless (`store: false`): the whole conversation is sent
/// each time, including the encrypted reasoning items of earlier steps, so
/// the model keeps its train of thought across tool calls.
pub struct Responses {
    client: reqwest::Client,
    url: String,
    api_key: String,
    model: String,
    effort: Option<String>,
    retry: Retry,
    /// Set once the API refuses replayed reasoning. It would refuse the
    /// same items on every later step, so they are no longer sent.
    reasoning_rejected: AtomicBool,
}

impl Responses {
    /// `base_url` is the API root, normally `https://api.openai.com/v1`.
    pub fn new(base_url: &str, api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            url: format!("{}/responses", base_url.trim_end_matches('/')),
            api_key: api_key.into(),
            model: model.into(),
            effort: None,
            retry: Retry::default(),
            reasoning_rejected: AtomicBool::new(false),
        }
    }

    /// Reasoning effort; `None` leaves the model's default.
    pub fn with_effort(mut self, effort: Option<String>) -> Self {
        self.effort = effort.filter(|effort| !effort.is_empty());
        self
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
        let mut attempt = 0;
        loop {
            let sent = self
                .client
                .post(&self.url)
                .bearer_auth(&self.api_key)
                .header("content-type", "application/json")
                .header("accept", "text/event-stream")
                .body(serde_json::to_vec(body).map_err(|e| LlmError::Protocol(e.to_string()))?)
                .send()
                .await
                .map_err(|e| LlmError::Transport(e.to_string()));
            let error = match sent {
                Ok(response) if response.status().is_success() => return Ok(response),
                Ok(response) => {
                    let status = response.status().as_u16();
                    let mut body = response.text().await.unwrap_or_default();
                    if body.len() > 2000 {
                        let mut end = 2000;
                        while !body.is_char_boundary(end) {
                            end -= 1;
                        }
                        body.truncate(end);
                    }
                    LlmError::Http { status, body }
                }
                Err(e) => e,
            };
            if !error.is_retryable() || attempt >= self.retry.max_retries {
                return Err(error);
            }
            tokio::time::sleep(self.retry.base_delay * 2u32.pow(attempt)).await;
            attempt += 1;
        }
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

fn input_items(message: &Message, with_reasoning: bool, out: &mut Vec<Value>) {
    match message.role {
        Role::User => {
            let text = text_of(&message.content);
            out.push(json!({"role": "user", "content": [{"type": "input_text", "text": text}]}));
        }
        // Each block is its own item, in the order the model produced them.
        Role::Assistant => {
            for block in &message.content {
                match block {
                    Block::Text { text } if !text.is_empty() => out.push(json!({
                        "role": "assistant",
                        "content": [{"type": "output_text", "text": text}],
                    })),
                    Block::Thinking {
                        signature: Some(signature),
                        ..
                    } if with_reasoning => {
                        // The signature is the reasoning item itself.
                        if let Some(item) = signature
                            .strip_prefix(SIGNATURE_PREFIX)
                            .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
                        {
                            out.push(item);
                        }
                    }
                    Block::ToolUse { id, name, input } => out.push(json!({
                        "type": "function_call",
                        "call_id": id,
                        "name": name,
                        "arguments": match input {
                            Value::String(raw) => raw.clone(),
                            other => other.to_string(),
                        },
                    })),
                    _ => {}
                }
            }
        }
        Role::Tool => {
            for block in &message.content {
                if let Block::ToolResult {
                    call_id, content, ..
                } = block
                {
                    out.push(json!({
                        "type": "function_call_output",
                        "call_id": call_id,
                        "output": text_of(content),
                    }));
                }
            }
        }
    }
}

fn build_body(model: &str, effort: Option<&str>, request: &Request, with_reasoning: bool) -> Value {
    let mut input = Vec::new();
    for message in &request.messages {
        input_items(message, with_reasoning, &mut input);
    }
    let mut body = json!({
        "model": model,
        "input": input,
        "stream": true,
        "store": false,
        "include": ["reasoning.encrypted_content"],
    });
    if let Some(system) = &request.system {
        body["instructions"] = json!(system);
    }
    if !request.tools.is_empty() {
        body["tools"] = request
            .tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.input_schema,
                })
            })
            .collect();
    }
    if let Some(effort) = effort {
        body["reasoning"] = json!({ "effort": effort });
    }
    if let Some(key) = &request.cache_key {
        body["prompt_cache_key"] = json!(key);
    }
    if let Some(max_tokens) = request.max_tokens {
        body["max_output_tokens"] = json!(max_tokens);
    }
    body
}

/// Whether an error says replayed reasoning was not accepted.
fn rejects_reasoning(error: &LlmError) -> bool {
    matches!(error, LlmError::Http { status: 400, body }
        if body.contains("reasoning") || body.contains("encrypted_content"))
}

impl Llm for Responses {
    fn stream<'a>(
        &'a self,
        request: &'a Request,
        sink: &'a mut dyn DeltaSink,
    ) -> BoxFuture<'a, Result<Completion, LlmError>> {
        Box::pin(async move {
            let with_reasoning = !self.reasoning_rejected.load(Ordering::Relaxed);
            let body = build_body(&self.model, self.effort.as_deref(), request, with_reasoning);
            let has_reasoning = body.to_string().contains("\"type\":\"reasoning\"");
            let mut response = match self.open(&body).await {
                // Reasoning from an earlier step is no longer accepted:
                // send the conversation once more without any, and leave
                // it out from now on.
                Err(e) if has_reasoning && rejects_reasoning(&e) => {
                    self.reasoning_rejected.store(true, Ordering::Relaxed);
                    let body = build_body(&self.model, self.effort.as_deref(), request, false);
                    self.open(&body).await?
                }
                other => other?,
            };

            let mut buffer: Vec<u8> = Vec::new();
            let mut decoder = Decoder::default();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|e| LlmError::Transport(e.to_string()))?
            {
                buffer.extend_from_slice(&chunk);
                while let Some(end) = buffer.iter().position(|b| *b == b'\n') {
                    let line: Vec<u8> = buffer.drain(..=end).collect();
                    let line = String::from_utf8_lossy(&line);
                    if let Some(data) = line.trim_end_matches(['\r', '\n']).strip_prefix("data:")
                        && decoder.feed(data.trim_start(), sink)?
                    {
                        return decoder.finish();
                    }
                }
            }
            decoder.finish()
        })
    }
}

#[derive(Default)]
struct Decoder {
    called_tools: bool,
    outcome: Option<(StopReason, Usage)>,
}

impl Decoder {
    /// Handles one event. Returns true once the response is complete.
    fn feed(&mut self, data: &str, sink: &mut dyn DeltaSink) -> Result<bool, LlmError> {
        if data == "[DONE]" {
            return Ok(true);
        }
        let event: Value = serde_json::from_str(data)
            .map_err(|e| LlmError::Protocol(format!("invalid stream event: {e}")))?;
        let mut emit = |delta: Delta| sink.delta(delta).map_err(LlmError::Sink);
        match event["type"].as_str() {
            Some("response.output_text.delta" | "response.refusal.delta") => {
                if let Some(text) = event["delta"].as_str().filter(|t| !t.is_empty()) {
                    emit(Delta::Text { text: text.into() })?;
                }
            }
            Some("response.output_item.added") if event["item"]["type"] == "function_call" => {
                self.called_tools = true;
                emit(Delta::ToolUseStart {
                    id: event["item"]["call_id"].as_str().unwrap_or_default().into(),
                    name: event["item"]["name"].as_str().unwrap_or_default().into(),
                })?;
            }
            Some("response.function_call_arguments.delta") => {
                if let Some(json) = event["delta"].as_str().filter(|j| !j.is_empty()) {
                    emit(Delta::ToolInput { json: json.into() })?;
                }
            }
            // A finished reasoning item: keep it whole so it can be sent
            // back, and show its summary if it has one.
            Some("response.output_item.done") if event["item"]["type"] == "reasoning" => {
                let item = &event["item"];
                let summary: String = item["summary"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|part| part["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                if !summary.is_empty() {
                    emit(Delta::Thinking { text: summary })?;
                }
                if item["encrypted_content"].is_string() {
                    let kept = json!({
                        "type": "reasoning",
                        "id": item["id"],
                        "summary": item["summary"],
                        "encrypted_content": item["encrypted_content"],
                    });
                    emit(Delta::ThinkingSignature {
                        signature: format!("{SIGNATURE_PREFIX}{kept}"),
                    })?;
                }
            }
            Some(kind @ ("response.completed" | "response.incomplete")) => {
                let response = &event["response"];
                let usage = &response["usage"];
                let cached = usage["input_tokens_details"]["cached_tokens"]
                    .as_u64()
                    .unwrap_or(0);
                let usage = Usage {
                    input_tokens: usage["input_tokens"]
                        .as_u64()
                        .unwrap_or(0)
                        .saturating_sub(cached),
                    output_tokens: usage["output_tokens"].as_u64().unwrap_or(0),
                    cache_read_tokens: cached,
                    cache_write_tokens: 0,
                };
                let reason = response["incomplete_details"]["reason"].as_str();
                let stop = match (kind, reason) {
                    ("response.incomplete", Some("content_filter")) => StopReason::Refusal,
                    ("response.incomplete", _) => StopReason::MaxTokens,
                    _ if self.called_tools => StopReason::ToolUse,
                    _ => StopReason::EndTurn,
                };
                self.outcome = Some((stop, usage));
                return Ok(true);
            }
            Some("response.failed") => {
                return Err(LlmError::Protocol(format!(
                    "the provider failed the response: {}",
                    event["response"]["error"]
                )));
            }
            Some("error") => {
                return Err(LlmError::Protocol(format!(
                    "the provider reported an error mid-stream: {event}"
                )));
            }
            _ => {}
        }
        Ok(false)
    }

    fn finish(self) -> Result<Completion, LlmError> {
        self.outcome
            .map(|(stop, usage)| Completion { stop, usage })
            .ok_or_else(|| {
                LlmError::Protocol("the stream ended before the response was complete".into())
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::serve;
    use hame_session::{ToolSpec, assemble};
    use std::time::Duration;

    fn adapter(base_url: &str) -> Responses {
        Responses::new(base_url, "test-key", "test-model")
            .with_effort(Some("low".into()))
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

    fn reasoning_item() -> Value {
        json!({"type": "reasoning", "id": "rs_1", "summary": [{"type": "summary_text", "text": "Need the file."}],
            "encrypted_content": "ENC"})
    }

    fn answer() -> String {
        sse(&[
            json!({"type": "response.created", "response": {}}),
            json!({"type": "response.output_item.added", "item": {"type": "reasoning", "id": "rs_1", "summary": []}}),
            json!({"type": "response.output_item.done", "item": reasoning_item()}),
            json!({"type": "response.output_item.added", "item": {"type": "message", "role": "assistant", "content": []}}),
            json!({"type": "response.output_text.delta", "delta": "Reading "}),
            json!({"type": "response.output_text.delta", "delta": "it."}),
            json!({"type": "response.output_item.added", "item": {"type": "function_call", "id": "fc_1",
                "call_id": "call_9", "name": "read", "arguments": ""}}),
            json!({"type": "response.function_call_arguments.delta", "delta": "{\"path\":"}),
            json!({"type": "response.function_call_arguments.delta", "delta": "\"a.rs\"}"}),
            json!({"type": "response.completed", "response": {"status": "completed", "usage": {
                "input_tokens": 120, "input_tokens_details": {"cached_tokens": 100}, "output_tokens": 30}}}),
        ])
    }

    fn request(history: Vec<Message>) -> Request {
        let mut messages = vec![Message {
            role: Role::User,
            content: vec![Block::text("Show a.rs")],
        }];
        messages.extend(history);
        Request {
            system: Some("You are hame.".into()),
            tools: vec![ToolSpec {
                name: "read".into(),
                description: "Read a file.".into(),
                input_schema: json!({"type": "object"}),
            }],
            messages,
            max_tokens: None,
            cache_key: None,
        }
    }

    async fn collect(
        adapter: &Responses,
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

    #[test]
    fn the_conversation_is_named_so_its_requests_share_a_cache() {
        let mut request = request(Vec::new());
        assert!(
            build_body("m", None, &request, true)
                .get("prompt_cache_key")
                .is_none()
        );
        request.cache_key = Some("s-1".into());
        assert_eq!(
            build_body("m", None, &request, true)["prompt_cache_key"],
            "s-1"
        );
    }

    #[tokio::test]
    async fn a_response_streams_and_its_reasoning_goes_back_on_the_next_step() {
        let (base_url, bodies) = serve(vec![(200, answer()), (200, answer())]).await;
        let adapter = adapter(&base_url);

        let (deltas, result) = collect(&adapter, &request(vec![])).await;
        assert_eq!(
            result.unwrap(),
            Completion {
                stop: StopReason::ToolUse,
                usage: Usage {
                    input_tokens: 20,
                    output_tokens: 30,
                    cache_read_tokens: 100,
                    cache_write_tokens: 0,
                },
            }
        );
        let content = assemble(&deltas);
        assert_eq!(content.len(), 3);
        assert!(
            matches!(&content[0], Block::Thinking { text, signature: Some(s) }
            if text == "Need the file." && s.starts_with("openai:"))
        );
        assert_eq!(content[1], Block::text("Reading it."));
        assert_eq!(
            content[2],
            Block::ToolUse {
                id: "call_9".into(),
                name: "read".into(),
                input: json!({"path": "a.rs"}),
            }
        );

        // Next step: the assistant turn and the tool result go back as items.
        let next = request(vec![
            Message {
                role: Role::Assistant,
                content,
            },
            Message {
                role: Role::Tool,
                content: vec![Block::ToolResult {
                    call_id: "call_9".into(),
                    content: vec![Block::text("fn a() {}")],
                    is_error: false,
                }],
            },
        ]);
        collect(&adapter, &next).await.1.unwrap();
        let sent = bodies.lock().unwrap()[1].clone();
        assert_eq!(
            sent,
            json!({
                "model": "test-model",
                "stream": true,
                "store": false,
                "include": ["reasoning.encrypted_content"],
                "instructions": "You are hame.",
                "reasoning": {"effort": "low"},
                "tools": [{"type": "function", "name": "read", "description": "Read a file.",
                    "parameters": {"type": "object"}}],
                "input": [
                    {"role": "user", "content": [{"type": "input_text", "text": "Show a.rs"}]},
                    reasoning_item(),
                    {"role": "assistant", "content": [{"type": "output_text", "text": "Reading it."}]},
                    {"type": "function_call", "call_id": "call_9", "name": "read", "arguments": "{\"path\":\"a.rs\"}"},
                    {"type": "function_call_output", "call_id": "call_9", "output": "fn a() {}"},
                ],
            })
        );
    }

    #[tokio::test]
    async fn rejected_reasoning_is_dropped_and_failures_are_reported() {
        let history = vec![Message {
            role: Role::Assistant,
            content: vec![
                Block::Thinking {
                    text: String::new(),
                    signature: Some(format!("openai:{}", reasoning_item())),
                },
                // Another provider's signature is never sent here.
                Block::Thinking {
                    text: String::new(),
                    signature: Some("anthropic-signature".into()),
                },
                Block::text("Done."),
            ],
        }];
        let rejection = json!({"error": {"message": "Invalid reasoning item: encrypted_content could not be verified."}}).to_string();
        let done = sse(&[
            json!({"type": "response.output_text.delta", "delta": "ok"}),
            json!({"type": "response.incomplete", "response": {"incomplete_details": {"reason": "max_output_tokens"}, "usage": {}}}),
        ]);
        let (base_url, bodies) =
            serve(vec![(400, rejection), (200, done.clone()), (200, done)]).await;
        let model = adapter(&base_url);
        let (_, result) = collect(&model, &request(history.clone())).await;
        assert_eq!(result.unwrap().stop, StopReason::MaxTokens);
        // The next step does not offer the reasoning again.
        let (_, result) = collect(&model, &request(history)).await;
        assert_eq!(result.unwrap().stop, StopReason::MaxTokens);
        {
            let bodies = bodies.lock().unwrap();
            let reasoning =
                |body: &Value| body.to_string().matches("\"type\":\"reasoning\"").count();
            assert_eq!(bodies.len(), 3);
            assert_eq!(
                [&bodies[0], &bodies[1], &bodies[2]].map(reasoning),
                [1, 0, 0]
            );
            assert!(!bodies[0].to_string().contains("anthropic-signature"));
        }

        let failed =
            sse(&[json!({"type": "response.failed", "response": {"error": {"message": "boom"}}})]);
        let (base_url, _) = serve(vec![(200, failed)]).await;
        let (_, result) = collect(&adapter(&base_url), &request(vec![])).await;
        assert!(matches!(result, Err(LlmError::Protocol(m)) if m.contains("boom")));

        let (base_url, _) = serve(vec![(
            200,
            sse(&[json!({"type": "response.created", "response": {}})]),
        )])
        .await;
        let (_, result) = collect(&adapter(&base_url), &request(vec![])).await;
        assert!(matches!(result, Err(LlmError::Protocol(_))));
    }
}
