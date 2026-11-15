use std::collections::BTreeMap;
use std::time::Instant;

use hame_session::{Block, Delta, Message, Role, StopReason, Usage};
use serde_json::{Value, json};

use crate::media::{Inliner, Media, Piece, has_media, joined, pieces};
use crate::{
    BoxFuture, Completion, DeltaSink, Llm, LlmError, Measure, ModelProfile, Provider, Request,
    Retry, Timed,
};

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
    /// What the model takes besides text.
    profile: ModelProfile,
}

/// Whether `base_url` points at OpenAI itself rather than a compatible server.
fn is_openai(base_url: &str) -> bool {
    reqwest::Url::parse(base_url).is_ok_and(|url| url.host_str() == Some("api.openai.com"))
}

impl ChatCompletions {
    /// `base_url` is the API root, e.g. `https://api.deepseek.com` or
    /// `https://api.openai.com/v1`.
    ///
    /// What the model can be shown besides text is resolved from its id,
    /// read as an OpenAI model at api.openai.com and as a model behind a
    /// compatible server elsewhere ([`ModelProfile::resolve`]); replace it
    /// with [`ChatCompletions::with_profile`].
    pub fn new(base_url: &str, api_key: impl Into<String>, model: impl Into<String>) -> Self {
        let model = model.into();
        let openai = is_openai(base_url);
        let provider = if openai {
            Provider::OpenAi
        } else {
            Provider::Compatible
        };
        Self {
            client: reqwest::Client::new(),
            url: format!("{}/chat/completions", base_url.trim_end_matches('/')),
            api_key: api_key.into(),
            profile: ModelProfile::resolve(provider, &model),
            model,
            temperature: None,
            retry: Retry::default(),
            openai,
        }
    }

    /// Overrides what the model is taken to accept besides text.
    pub fn with_profile(mut self, profile: ModelProfile) -> Self {
        self.profile = profile;
        self
    }

    pub fn profile(&self) -> ModelProfile {
        self.profile
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
        if response.status().is_success() {
            return Ok(response);
        }
        Err(crate::http_error(response).await)
    }
}

impl Llm for ChatCompletions {
    fn stream<'a>(
        &'a self,
        request: &'a Request,
        sink: &'a mut dyn DeltaSink,
    ) -> BoxFuture<'a, Result<Completion, LlmError>> {
        Box::pin(async move {
            let (mut body, images, image_bytes) =
                build_body(&self.model, self.profile, request, self.openai);
            if let Some(temperature) = self.temperature {
                body["temperature"] = json!(temperature);
            }
            let body = serde_json::to_vec(&body).map_err(|e| LlmError::Protocol(e.to_string()))?;

            let mut attempt = 0;
            let (mut response, mut measure) = loop {
                let sent = Instant::now();
                match self.open(&body).await {
                    Ok(response) => {
                        break (
                            response,
                            Measure::new(body.len(), images, image_bytes, sent),
                        );
                    }
                    Err(e) => match self.retry.delay(attempt, &e) {
                        Some(wait) => {
                            tokio::time::sleep(wait).await;
                            attempt += 1;
                        }
                        None => return Err(e),
                    },
                }
            };

            let mut lines = SseLines::default();
            let mut decoder = Decoder::default();
            let mut timed = Timed {
                inner: sink,
                sent: measure.sent,
                first: None,
            };
            'stream: while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|e| LlmError::Transport(e.to_string()))?
            {
                measure.response_bytes += chunk.len() as u64;
                for data in lines.push(&chunk) {
                    if decoder.feed(&data, &mut timed)? {
                        break 'stream;
                    }
                }
            }
            measure.first_delta = timed.first;
            decoder.finish().map(|completion| measure.apply(completion))
        })
    }
}

/// The request body, and how many images it carries and their raw bytes.
/// `cache_key` says whether the server takes `prompt_cache_key`.
fn build_body(
    model: &str,
    profile: ModelProfile,
    request: &Request,
    cache_key: bool,
) -> (Value, u32, u64) {
    let mut inliner = Inliner::new(profile, request);
    let mut messages = Vec::new();
    if let Some(system) = &request.system {
        messages.push(json!({ "role": "system", "content": system }));
    }
    for message in &request.messages {
        push_message(&mut messages, message, &mut inliner);
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
    (body, inliner.images, inliner.image_bytes)
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

/// A user content part for a piece. Shapes from the chat-completions
/// reference (developers.openai.com, "Create chat completion"): `text`,
/// `image_url` with the base64 data URL in `image_url.url`, and `file`
/// with `filename` and `file_data` for a PDF.
fn user_part(piece: Piece) -> Value {
    match piece {
        Piece::Text(text) => json!({"type": "text", "text": text}),
        Piece::Media(Media::Image { mime, data }, _) => json!({
            "type": "image_url",
            "image_url": {"url": Media::data_url(&mime, &data)},
        }),
        Piece::Media(Media::Pdf { data, name }, _) => json!({
            "type": "file",
            "file": {
                "filename": name.unwrap_or_else(|| "document.pdf".into()),
                "file_data": Media::data_url("application/pdf", &data),
            },
        }),
        Piece::Media(Media::Note(text), _) => json!({"type": "text", "text": text}),
    }
}

fn push_message(out: &mut Vec<Value>, message: &Message, inliner: &mut Inliner) {
    match message.role {
        Role::User => {
            let pieces = pieces(&message.content, inliner);
            let content = if has_media(&pieces) {
                Value::Array(pieces.into_iter().map(user_part).collect())
            } else {
                json!(joined(pieces))
            };
            out.push(json!({ "role": "user", "content": content }));
        }
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
        // This format carries one tool result per message, and a tool
        // message holds text only ("For tool messages, only type `text` is
        // supported"). Images a result carries follow all the tool messages
        // of the turn in one user message, labelled with their call, and
        // the tool message says where they went.
        Role::Tool => {
            let mut attached: Vec<Value> = Vec::new();
            for block in &message.content {
                let Block::ToolResult {
                    call_id, content, ..
                } = block
                else {
                    continue;
                };
                let mut text = Vec::new();
                let mut media = Vec::new();
                for piece in pieces(content, inliner) {
                    match piece {
                        Piece::Text(part) => text.push(part),
                        Piece::Media(sent, label) => {
                            text.push(format!(
                                "{}: attached in the next user message]",
                                label.trim_end_matches(']')
                            ));
                            media.push(user_part(Piece::Media(sent, label)));
                        }
                    }
                }
                out.push(json!({
                    "role": "tool",
                    "tool_call_id": call_id,
                    "content": text.join("\n"),
                }));
                if !media.is_empty() {
                    attached.push(json!({
                        "type": "text",
                        "text": format!("Attachments from tool call {call_id}:"),
                    }));
                    attached.extend(media);
                }
            }
            if !attached.is_empty() {
                out.push(json!({"role": "user", "content": attached}));
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
    model: Option<String>,
    raw_usage: Option<Value>,
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
        if let Some(model) = chunk["model"].as_str() {
            self.model = Some(model.to_string());
        }
        if let Some(usage) = chunk.get("usage").filter(|usage| usage.is_object()) {
            self.usage = read_usage(usage);
            self.raw_usage = Some(usage.clone());
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
            meta: crate::CallMeta {
                model: self.model,
                raw_usage: self.raw_usage,
                ..crate::CallMeta::default()
            },
        })
    }
}

/// `input_tokens` counts only the uncached part of the prompt; cached prompt
/// tokens are reported separately as `cache_read_tokens`. Both DeepSeek
/// (`prompt_cache_hit_tokens`, with `prompt_cache_miss_tokens` the rest) and
/// OpenAI (`prompt_tokens_details.cached_tokens`) include the cached tokens
/// in `prompt_tokens`, so they are subtracted, never added.
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
    use hame_session::{ToolSpec, assemble};
    use std::time::Duration;

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
            system: Some("You are hame.".into()),
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
            artifacts: None,
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
            json!({"choices": [], "model": "deepseek-test", "usage": {
                "prompt_tokens": 100, "completion_tokens": 20, "prompt_cache_hit_tokens": 60
            }}),
        ]);
        let response_len = body.len();
        let (base_url, bodies) = serve(vec![(200, body)]).await;

        let (deltas, result) = collect(&adapter(&base_url), &request()).await;

        let completion = result.unwrap();
        let meta = completion.meta.clone();
        assert_eq!(
            completion,
            Completion {
                meta: meta.clone(),
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
            meta.request_bytes,
            Some(serde_json::to_vec(&sent).unwrap().len() as u64)
        );
        assert_eq!(meta.response_bytes, Some(response_len as u64));
        assert_eq!((meta.images, meta.image_bytes), (Some(0), Some(0)));
        assert!(meta.ttft_ms.is_some());
        assert_eq!(meta.model.as_deref(), Some("deepseek-test"));
        assert_eq!(meta.raw_usage.unwrap()["prompt_cache_hit_tokens"], 60);
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
                    {"role": "system", "content": "You are hame."},
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
            build_body("m", ModelProfile::TEXT_ONLY, &request, true)
                .0
                .get("prompt_cache_key")
                .is_none()
        );
        request.cache_key = Some("s-1".into());
        assert_eq!(
            build_body("m", ModelProfile::TEXT_ONLY, &request, true).0["prompt_cache_key"],
            "s-1"
        );
        assert!(
            build_body("m", ModelProfile::TEXT_ONLY, &request, false)
                .0
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
            Err(LlmError::Http { status: 401, body, .. }) if body == "bad key"
        ));
        assert_eq!(bodies.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_wait_asked_for_by_the_provider_is_kept_or_reported() {
        let ok = sse(&[
            json!({"choices": [{"delta": {"content": "hi"}}]}),
            json!({"choices": [{"delta": {}, "finish_reason": "stop"}]}),
        ]);
        let (base_url, bodies) = crate::testing::serve_with_headers(vec![
            (429, "slow down".into(), "retry-after-ms: 150\r\n"),
            (200, ok, ""),
        ])
        .await;
        let started = std::time::Instant::now();
        let (_, result) = collect(&adapter(&base_url), &request()).await;
        assert_eq!(result.unwrap().stop, StopReason::EndTurn);
        // The adapter's own backoff is 1 ms; only the header explains this.
        assert!(started.elapsed() >= Duration::from_millis(150));
        assert_eq!(bodies.lock().unwrap().len(), 2);

        // A limit that lifts in an hour is reported at once.
        let (base_url, bodies) = crate::testing::serve_with_headers(vec![(
            429,
            "daily limit".into(),
            "retry-after: 3600\r\n",
        )])
        .await;
        let (_, result) = collect(&adapter(&base_url), &request()).await;
        assert!(matches!(
            result,
            Err(LlmError::Http { status: 429, retry_after: Some(wait), .. })
                if wait == Duration::from_secs(3600)
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

    #[test]
    fn tool_images_follow_the_tool_messages_for_a_vision_model_only() {
        use crate::media::tests::{b64, fixture, png};
        let fx = fixture();
        let profile = ModelProfile::resolve(Provider::OpenAi, "gpt-4o-mini");
        let (body, images, image_bytes) = build_body("m", profile, &fx.request, false);
        assert_eq!((images, image_bytes), (2, fx.red.bytes + fx.blue.bytes));
        let url = |bytes: &[u8]| format!("data:image/png;base64,{}", b64(bytes));
        let missing = format!(
            "[image image/png 8x8 {} bytes, artifact {}: not sent, it is missing from the \
             artifact store]",
            fx.gone.bytes,
            fx.gone.short()
        );
        let attached = format!(
            "[image image/png 8x8 {} bytes, artifact {}: attached in the next user message]",
            fx.blue.bytes,
            fx.blue.short()
        );
        assert_eq!(
            body["messages"],
            json!([
                {"role": "user", "content": [
                    {"type": "text", "text": "Compare these."},
                    {"type": "image_url", "image_url": {"url": url(&png(8, 8, [255, 0, 0], [255, 0, 0]))}},
                    {"type": "file", "file": {"filename": "spec.pdf",
                        "file_data": format!("data:application/pdf;base64,{}", b64(b"%PDF-1.4 x"))}},
                ]},
                {"role": "assistant", "content": null, "tool_calls": [
                    {"id": "t1", "type": "function", "function": {"name": "shot", "arguments": "{}"}},
                ]},
                {"role": "tool", "tool_call_id": "t1", "content": format!(
                    "captured\n{attached}\n{missing}\n{{\"ok\":true}}\n[audio content not carried: audio/wav, 10 bytes]")},
                {"role": "user", "content": [
                    {"type": "text", "text": "Attachments from tool call t1:"},
                    {"type": "image_url", "image_url": {"url": url(&png(8, 8, [0, 0, 255], [0, 0, 255]))}},
                ]},
            ])
        );

        // DeepSeek takes no images: notes in place, no extra user message,
        // plain string content.
        let deepseek = ChatCompletions::new("https://api.deepseek.com", "k", "deepseek-chat");
        assert_eq!(deepseek.profile(), ModelProfile::TEXT_ONLY);
        let (body, images, _) = build_body("m", deepseek.profile(), &fx.request, false);
        assert_eq!(images, 0);
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 3);
        assert!(messages[0]["content"].is_string());
        let tool = messages[2]["content"].as_str().unwrap();
        assert!(
            tool.contains(&format!(
                "artifact {}: not sent, this model does not take images",
                fx.blue.short()
            )),
            "{tool}"
        );
        assert!(!body.to_string().contains("base64,"));
    }

    /// Usage in the shapes DeepSeek and OpenAI report it: both include the
    /// cached tokens in `prompt_tokens`. DeepSeek's fields are from its API
    /// documentation (`prompt_cache_hit_tokens` + `prompt_cache_miss_tokens`
    /// = `prompt_tokens`); OpenAI's from its chat-completions reference.
    #[test]
    fn cached_prompt_tokens_are_counted_once_for_each_provider() {
        let deepseek = json!({"prompt_tokens": 5120, "completion_tokens": 410,
            "total_tokens": 5530, "prompt_cache_hit_tokens": 4864,
            "prompt_cache_miss_tokens": 256,
            "prompt_tokens_details": {"cached_tokens": 4864},
            "completion_tokens_details": {"reasoning_tokens": 300}});
        let openai = json!({"prompt_tokens": 2048, "completion_tokens": 64,
            "total_tokens": 2112,
            "prompt_tokens_details": {"cached_tokens": 1536, "audio_tokens": 0},
            "completion_tokens_details": {"reasoning_tokens": 0}});
        for (raw, uncached, cached) in [(deepseek, 256, 4864), (openai, 512, 1536)] {
            let usage = read_usage(&raw);
            assert_eq!(usage.input_tokens, uncached, "{raw}");
            assert_eq!(usage.cache_read_tokens, cached, "{raw}");
            assert_eq!(usage.cache_write_tokens, 0);
            assert_eq!(
                usage.input_tokens + usage.cache_read_tokens + usage.output_tokens,
                raw["total_tokens"].as_u64().unwrap(),
                "{raw}"
            );
        }
    }
}
