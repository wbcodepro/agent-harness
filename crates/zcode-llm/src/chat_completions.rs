use std::collections::BTreeMap;

use serde_json::{Value, json};
use zcode_session::{Block, Delta, Message, Role, StopReason, Usage};

use crate::{BoxFuture, Completion, DeltaSink, Llm, LlmError, Request, Retry};

/// Adapter for the OpenAI chat-completions wire format, which DeepSeek and
/// many gateways also speak.
pub struct ChatCompletions {
    client: reqwest::Client,
    url: String,
    api_key: String,
    model: String,
    temperature: Option<f32>,
    retry: Retry,
    /// Whether the server is OpenAI's own, which takes fields that other
    /// servers speaking this format may reject.
    openai: bool,
}

/// Whether `base_url` points at OpenAI itself rather than a compatible server.
fn is_openai(base_url: &str) -> bool {
    reqwest::Url::parse(base_url).is_ok_and(|url| url.host_str() == Some("api.openai.com"))
}

impl ChatCompletions {
    /// `base_url` is the API root, e.g. `https://api.deepseek.com` or
    /// `https://api.openai.com/v1`.
    pub fn new(base_url: &str, api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            url: format!("{}/chat/completions", base_url.trim_end_matches('/')),
            api_key: api_key.into(),
            model: model.into(),
            temperature: None,
            retry: Retry::default(),
            openai: is_openai(base_url),
        }
    }

    /// Sampling temperature; `None` leaves the server's default.
    pub fn with_temperature(mut self, temperature: Option<f32>) -> Self {
        self.temperature = temperature;
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

    async fn open(&self, body: &[u8]) -> Result<reqwest::Response, LlmError> {
        let response = self
            .client
            .post(&self.url)
            .bearer_auth(&self.api_key)
            .header("content-type", "application/json")
            .header("accept", "text/event-stream")
            .body(body.to_vec())
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
}

impl Llm for ChatCompletions {
    fn stream<'a>(
        &'a self,
        request: &'a Request,
        sink: &'a mut dyn DeltaSink,
    ) -> BoxFuture<'a, Result<Completion, LlmError>> {
        Box::pin(async move {
            let mut body = build_body(&self.model, request, self.openai);
            if let Some(temperature) = self.temperature {
                body["temperature"] = json!(temperature);
            }
            let body = serde_json::to_vec(&body).map_err(|e| LlmError::Protocol(e.to_string()))?;

            let mut attempt = 0;
            let mut response = loop {
                match self.open(&body).await {
                    Ok(response) => break response,
                    Err(e) if e.is_retryable() && attempt < self.retry.max_retries => {
                        tokio::time::sleep(self.retry.base_delay * 2u32.pow(attempt)).await;
                        attempt += 1;
                    }
                    Err(e) => return Err(e),
                }
            };

            let mut lines = SseLines::default();
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

/// `cache_key` says whether the server takes `prompt_cache_key`.
fn build_body(model: &str, request: &Request, cache_key: bool) -> Value {
    let mut messages = Vec::new();
    if let Some(system) = &request.system {
        messages.push(json!({ "role": "system", "content": system }));
    }
    for message in &request.messages {
        push_message(&mut messages, message);
    }

    let mut body = json!({
        "model": model,
        "messages": messages,
        "stream": true,
        "stream_options": { "include_usage": true },
    });
    if !request.tools.is_empty() {
        body["tools"] = request
            .tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "function": {
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.input_schema,
                    }
                })
            })
            .collect();
    }
    if let Some(max_tokens) = request.max_tokens {
        body["max_tokens"] = json!(max_tokens);
    }
    if let Some(key) = request.cache_key.as_ref().filter(|_| cache_key) {
        body["prompt_cache_key"] = json!(key);
    }
    body
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

fn push_message(out: &mut Vec<Value>, message: &Message) {
    match message.role {
        Role::User => out.push(json!({ "role": "user", "content": text_of(&message.content) })),
        Role::Assistant => {
            let tool_calls: Vec<Value> = message
                .content
                .iter()
                .filter_map(|block| match block {
                    Block::ToolUse { id, name, input } => Some(json!({
                        "id": id,
                        "type": "function",
                        "function": {
                            "name": name,
                            // A call whose arguments never parsed is stored
                            // as the raw string; send that back unchanged.
                            "arguments": match input {
                                Value::String(raw) => raw.clone(),
                                other => other.to_string(),
                            },
                        }
                    })),
                    _ => None,
                })
                .collect();
            let text = text_of(&message.content);
            let mut wire = json!({
                "role": "assistant",
                "content": if text.is_empty() { Value::Null } else { Value::String(text) },
            });
            if !tool_calls.is_empty() {
                wire["tool_calls"] = Value::Array(tool_calls);
            }
            out.push(wire);
        }
        // This format carries one tool result per message.
        Role::Tool => {
            for block in &message.content {
                if let Block::ToolResult {
                    call_id, content, ..
                } = block
                {
                    out.push(json!({
                        "role": "tool",
                        "tool_call_id": call_id,
                        "content": text_of(content),
                    }));
                }
            }
        }
    }
}

/// Splits a server-sent-event byte stream into `data:` payloads, across
/// arbitrary chunk boundaries.
#[derive(Default)]
struct SseLines {
    buffer: Vec<u8>,
}

impl SseLines {
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

/// Folds chat-completion chunks into deltas and a final [`Completion`].
#[derive(Default)]
struct Decoder {
    /// The call whose arguments go to the sink as they arrive. Deltas carry
    /// no index, so only one call can stream this way: the first, and only
    /// when it is index 0, which keeps the calls in index order.
    live_tool: Option<u64>,
    /// Every other call, held until the response ends, because the format
    /// lets a provider interleave the arguments of parallel calls.
    held_tools: BTreeMap<u64, HeldTool>,
    saw_tool: bool,
    stop: Option<StopReason>,
    usage: Usage,
}

struct HeldTool {
    id: String,
    name: String,
    arguments: String,
}

impl Decoder {
    /// Handles one `data:` payload. Returns true at the end-of-stream marker.
    fn feed(&mut self, data: &str, sink: &mut dyn DeltaSink) -> Result<bool, LlmError> {
        if data == "[DONE]" {
            self.release_held(sink)?;
            return Ok(true);
        }
        let chunk: Value = serde_json::from_str(data)
            .map_err(|e| LlmError::Protocol(format!("invalid stream chunk: {e}")))?;
        if let Some(error) = chunk.get("error") {
            return Err(LlmError::Protocol(format!("provider error: {error}")));
        }
        if let Some(usage) = chunk.get("usage").filter(|usage| usage.is_object()) {
            self.usage = read_usage(usage);
        }
        let Some(choice) = chunk.pointer("/choices/0") else {
            return Ok(false);
        };
        let mut emit = |delta: Delta| sink.delta(delta).map_err(LlmError::Sink);

        let delta = &choice["delta"];
        if let Some(text) = delta["reasoning_content"]
            .as_str()
            .filter(|t| !t.is_empty())
        {
            emit(Delta::Thinking { text: text.into() })?;
        }
        if let Some(text) = delta["content"].as_str().filter(|t| !t.is_empty()) {
            emit(Delta::Text { text: text.into() })?;
        }
        for call in delta["tool_calls"].as_array().into_iter().flatten() {
            let index = call["index"].as_u64().unwrap_or(0);
            let arguments = call
                .pointer("/function/arguments")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if self.live_tool != Some(index) && !self.held_tools.contains_key(&index) {
                let name = call
                    .pointer("/function/name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        LlmError::Protocol(format!("tool call {index} continued before it started"))
                    })?;
                let id = call["id"]
                    .as_str()
                    .map_or_else(|| format!("call_{index}"), str::to_string);
                if index == 0 && !self.saw_tool {
                    emit(Delta::ToolUseStart {
                        id,
                        name: name.into(),
                    })?;
                    self.live_tool = Some(index);
                } else {
                    let held = HeldTool {
                        id,
                        name: name.into(),
                        arguments: String::new(),
                    };
                    self.held_tools.insert(index, held);
                }
                self.saw_tool = true;
            }
            if let Some(held) = self.held_tools.get_mut(&index) {
                held.arguments.push_str(arguments);
            } else if !arguments.is_empty() {
                emit(Delta::ToolInput {
                    json: arguments.into(),
                })?;
            }
        }

        if let Some(reason) = choice["finish_reason"].as_str() {
            self.stop = Some(match reason {
                "stop" => StopReason::EndTurn,
                "tool_calls" => StopReason::ToolUse,
                "length" => StopReason::MaxTokens,
                "content_filter" => StopReason::Refusal,
                other => {
                    return Err(LlmError::Protocol(format!(
                        "unsupported finish_reason {other:?}"
                    )));
                }
            });
            self.release_held(sink)?;
        }
        Ok(false)
    }

    /// Sends the held calls, complete and in index order. Nothing more can
    /// be added to them once the response has ended.
    fn release_held(&mut self, sink: &mut dyn DeltaSink) -> Result<(), LlmError> {
        let mut emit = |delta: Delta| sink.delta(delta).map_err(LlmError::Sink);
        for held in std::mem::take(&mut self.held_tools).into_values() {
            emit(Delta::ToolUseStart {
                id: held.id,
                name: held.name,
            })?;
            if !held.arguments.is_empty() {
                emit(Delta::ToolInput {
                    json: held.arguments,
                })?;
            }
        }
        Ok(())
    }

    fn finish(self) -> Result<Completion, LlmError> {
        let stop = self
            .stop
            .ok_or_else(|| LlmError::Protocol("the stream ended without a finish_reason".into()))?;
        // Some providers report "stop" even though the response calls tools.
        let stop = match stop {
            StopReason::EndTurn if self.saw_tool => StopReason::ToolUse,
            other => other,
        };
        Ok(Completion {
            stop,
            usage: self.usage,
        })
    }
}

/// `input_tokens` counts only the uncached part of the prompt; cached prompt
/// tokens are reported separately as `cache_read_tokens`.
fn read_usage(usage: &Value) -> Usage {
    let prompt = usage["prompt_tokens"].as_u64().unwrap_or(0);
    let cached = usage["prompt_cache_hit_tokens"]
        .as_u64()
        .or_else(|| {
            usage
                .pointer("/prompt_tokens_details/cached_tokens")
                .and_then(Value::as_u64)
        })
        .unwrap_or(0);
    Usage {
        input_tokens: prompt.saturating_sub(cached),
        output_tokens: usage["completion_tokens"].as_u64().unwrap_or(0),
        cache_read_tokens: cached,
        cache_write_tokens: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::serve;
    use std::time::Duration;
    use zcode_session::{ToolSpec, assemble};

    fn adapter(base_url: &str) -> ChatCompletions {
        ChatCompletions::new(base_url, "test-key", "test-model")
            .with_temperature(Some(0.0))
            .with_client(reqwest::Client::builder().no_proxy().build().unwrap())
            .with_retry(Retry {
                max_retries: 2,
                base_delay: Duration::from_millis(1),
            })
    }

    fn sse(chunks: &[Value]) -> String {
        let mut body = String::new();
        for chunk in chunks {
            body.push_str(&format!("data: {chunk}\r\n\r\n"));
        }
        body.push_str("data: [DONE]\n\n");
        body
    }

    fn request() -> Request {
        Request {
            system: Some("You are zcode.".into()),
            tools: vec![ToolSpec {
                name: "read".into(),
                description: "Read a file.".into(),
                input_schema: json!({"type": "object"}),
            }],
            messages: vec![
                Message {
                    role: Role::User,
                    content: vec![Block::text("Show a.rs and b.rs")],
                },
                Message {
                    role: Role::Assistant,
                    content: vec![
                        Block::Thinking {
                            text: "private".into(),
                            signature: None,
                        },
                        Block::text("Reading."),
                        Block::ToolUse {
                            id: "c0".into(),
                            name: "read".into(),
                            input: json!({"path": "a.rs"}),
                        },
                    ],
                },
                Message {
                    role: Role::Tool,
                    content: vec![Block::ToolResult {
                        call_id: "c0".into(),
                        content: vec![Block::text("fn a() {}")],
                        is_error: false,
                    }],
                },
            ],
            max_tokens: Some(256),
            cache_key: None,
        }
    }

    async fn collect(
        adapter: &ChatCompletions,
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

    #[tokio::test]
    async fn streams_text_thinking_and_parallel_tool_calls() {
        let tool = |index: u64, id: Option<&str>, name: Option<&str>, arguments: &str| {
            json!({"choices": [{"delta": {"tool_calls": [{
                "index": index, "id": id,
                "function": {"name": name, "arguments": arguments}
            }]}}]})
        };
        let body = sse(&[
            json!({"choices": [{"delta": {"role": "assistant", "content": ""}}]}),
            json!({"choices": [{"delta": {"reasoning_content": "Need b.rs."}}]}),
            json!({"choices": [{"delta": {"content": "Now "}}]}),
            json!({"choices": [{"delta": {"content": "b.rs — xong."}}]}),
            tool(0, Some("c1"), Some("read"), ""),
            tool(0, None, None, "{\"path\":"),
            tool(0, None, None, "\"b.rs\"}"),
            tool(1, Some("c2"), Some("read"), "{\"path\":\"c.rs\"}"),
            json!({"choices": [{"delta": {}, "finish_reason": "tool_calls"}]}),
            json!({"choices": [], "usage": {
                "prompt_tokens": 100, "completion_tokens": 20, "prompt_cache_hit_tokens": 60
            }}),
        ]);
        let (base_url, bodies) = serve(vec![(200, body)]).await;

        let (deltas, result) = collect(&adapter(&base_url), &request()).await;

        assert_eq!(
            result.unwrap(),
            Completion {
                stop: StopReason::ToolUse,
                usage: Usage {
                    input_tokens: 40,
                    output_tokens: 20,
                    cache_read_tokens: 60,
                    cache_write_tokens: 0,
                },
            }
        );
        assert_eq!(
            assemble(&deltas),
            vec![
                Block::Thinking {
                    text: "Need b.rs.".into(),
                    signature: None,
                },
                Block::text("Now b.rs — xong."),
                Block::ToolUse {
                    id: "c1".into(),
                    name: "read".into(),
                    input: json!({"path": "b.rs"}),
                },
                Block::ToolUse {
                    id: "c2".into(),
                    name: "read".into(),
                    input: json!({"path": "c.rs"}),
                },
            ]
        );

        let sent = bodies.lock().unwrap()[0].clone();
        assert_eq!(
            sent,
            json!({
                "model": "test-model",
                "stream": true,
                "stream_options": {"include_usage": true},
                "max_tokens": 256,
                "temperature": 0.0,
                "tools": [{"type": "function", "function": {
                    "name": "read", "description": "Read a file.",
                    "parameters": {"type": "object"}
                }}],
                "messages": [
                    {"role": "system", "content": "You are zcode."},
                    {"role": "user", "content": "Show a.rs and b.rs"},
                    {"role": "assistant", "content": "Reading.", "tool_calls": [{
                        "id": "c0", "type": "function",
                        "function": {"name": "read", "arguments": "{\"path\":\"a.rs\"}"}
                    }]},
                    {"role": "tool", "tool_call_id": "c0", "content": "fn a() {}"},
                ],
            })
        );
    }

    #[tokio::test]
    async fn interleaved_tool_calls_come_out_whole_and_in_index_order() {
        let tool = |index: u64, id: Option<&str>, name: Option<&str>, arguments: &str| json!({"index": index, "id": id, "function": {"name": name, "arguments": arguments}});
        let chunk = |calls: Vec<Value>| json!({"choices": [{"delta": {"tool_calls": calls}}]});
        let body = sse(&[
            // Index 2 starts before index 1, and one chunk carries two calls.
            chunk(vec![tool(0, Some("c0"), Some("read"), "{\"path\":")]),
            chunk(vec![tool(2, Some("c2"), Some("list"), "")]),
            chunk(vec![tool(1, Some("c1"), Some("read"), "{\"pa")]),
            chunk(vec![
                tool(0, None, None, "\"a.rs\"}"),
                tool(1, None, None, "th\":\"b"),
            ]),
            chunk(vec![tool(2, None, None, "{}")]),
            chunk(vec![tool(1, None, None, ".rs\"}")]),
            json!({"choices": [{"delta": {}, "finish_reason": "tool_calls"}]}),
        ]);
        let (base_url, _) = serve(vec![(200, body)]).await;
        let (deltas, result) = collect(&adapter(&base_url), &request()).await;

        assert_eq!(result.unwrap().stop, StopReason::ToolUse);
        let call = |id: &str, name: &str, input: Value| Block::ToolUse {
            id: id.into(),
            name: name.into(),
            input,
        };
        assert_eq!(
            assemble(&deltas),
            vec![
                call("c0", "read", json!({"path": "a.rs"})),
                call("c1", "read", json!({"path": "b.rs"})),
                call("c2", "list", json!({})),
            ]
        );
        // The first call still streams as it arrives.
        assert_eq!(
            deltas[..2],
            [
                Delta::ToolUseStart {
                    id: "c0".into(),
                    name: "read".into(),
                },
                Delta::ToolInput {
                    json: "{\"path\":".into(),
                },
            ]
        );
    }

    #[tokio::test]
    async fn a_tool_call_that_never_started_is_a_protocol_error() {
        let body = sse(&[json!({"choices": [{"delta": {"tool_calls": [{
            "index": 1, "function": {"arguments": "{}"}
        }]}}]})]);
        let (base_url, _) = serve(vec![(200, body)]).await;
        let (_, result) = collect(&adapter(&base_url), &request()).await;
        assert!(matches!(result, Err(LlmError::Protocol(_))));
    }

    #[test]
    fn the_cache_key_is_sent_only_to_openai_itself() {
        let mut request = request();
        assert!(
            build_body("m", &request, true)
                .get("prompt_cache_key")
                .is_none()
        );
        request.cache_key = Some("s-1".into());
        assert_eq!(build_body("m", &request, true)["prompt_cache_key"], "s-1");
        assert!(
            build_body("m", &request, false)
                .get("prompt_cache_key")
                .is_none()
        );

        assert!(is_openai("https://api.openai.com/v1"));
        assert!(is_openai("https://api.openai.com/v1/"));
        for other in [
            "https://api.deepseek.com",
            "http://localhost:11434/v1",
            "https://api.openai.com.example.net/v1",
            "https://gateway.example/api.openai.com/v1",
            "not a url",
        ] {
            assert!(!is_openai(other), "{other}");
        }
        assert!(ChatCompletions::new("https://api.openai.com/v1", "k", "m").openai);
        assert!(!ChatCompletions::new("https://api.deepseek.com", "k", "m").openai);
    }

    #[tokio::test]
    async fn retries_before_the_stream_starts_but_not_client_errors() {
        let ok = sse(&[
            json!({"choices": [{"delta": {"content": "hi"}}]}),
            json!({"choices": [{"delta": {}, "finish_reason": "stop"}]}),
        ]);
        let (base_url, bodies) = serve(vec![
            (503, "busy".into()),
            (429, "slow down".into()),
            (200, ok),
        ])
        .await;
        let (deltas, result) = collect(&adapter(&base_url), &request()).await;
        assert_eq!(result.unwrap().stop, StopReason::EndTurn);
        assert_eq!(deltas, vec![Delta::Text { text: "hi".into() }]);
        assert_eq!(bodies.lock().unwrap().len(), 3);

        let (base_url, bodies) = serve(vec![(401, "bad key".into())]).await;
        let (_, result) = collect(&adapter(&base_url), &request()).await;
        assert!(matches!(
            result,
            Err(LlmError::Http { status: 401, body }) if body == "bad key"
        ));
        assert_eq!(bodies.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_stream_that_stops_early_is_an_error_after_its_deltas() {
        let body = "data: {\"choices\":[{\"delta\":{\"content\":\"par\"}}]}\n\n".to_string();
        let (base_url, _) = serve(vec![(200, body)]).await;
        let (deltas, result) = collect(&adapter(&base_url), &request()).await;
        assert_eq!(deltas, vec![Delta::Text { text: "par".into() }]);
        assert!(matches!(result, Err(LlmError::Protocol(_))));
    }
}
