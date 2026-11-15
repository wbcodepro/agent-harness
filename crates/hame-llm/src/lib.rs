//! The model seam: one streaming call, expressed in the session vocabulary.
//!
//! An adapter turns a [`Request`] into provider wire format, and the
//! provider's stream back into [`Delta`]s pushed at a [`DeltaSink`]. The
//! caller owns the sink, which is how the agent journals a response while it
//! is still arriving.

mod anthropic;
mod chat_completions;
mod responses;
#[cfg(test)]
mod testing;

use std::time::Duration;

pub use anthropic::Anthropic;
pub use chat_completions::ChatCompletions;
pub use hame_kernel::BoxFuture;
use hame_session::{ArtifactSource, Delta, History, Message, StopReason, ToolSpec, Usage};
pub use responses::Responses;

/// Everything one model call is built from.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Request {
    pub system: Option<String>,
    pub tools: Vec<ToolSpec>,
    pub messages: Vec<Message>,
    pub max_tokens: Option<u32>,
    /// Names the conversation this request continues, for providers that
    /// route requests with the same key to the same prompt cache.
    pub cache_key: Option<String>,
    /// Where image and artifact blocks' bytes are read from when the
    /// request is encoded. Without one, such blocks are sent as text notes.
    pub artifacts: Option<ArtifactSource>,
}

impl From<History> for Request {
    fn from(history: History) -> Self {
        Self {
            system: history.system,
            tools: history.tools,
            messages: history.messages,
            max_tokens: None,
            cache_key: None,
            artifacts: None,
        }
    }
}

/// How a response ended, once its stream is complete.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Completion {
    pub stop: StopReason,
    pub usage: Usage,
    /// Measurements of the call itself, for telemetry. Fields an adapter
    /// cannot observe stay `None`: unknown, not zero.
    pub meta: CallMeta,
}

/// What one model call cost on the wire, as the client observed it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CallMeta {
    /// The model the provider says answered, when it says.
    pub model: Option<String>,
    /// Bytes of the HTTP request body actually sent.
    pub request_bytes: Option<u64>,
    /// Bytes of the response body received.
    pub response_bytes: Option<u64>,
    /// Images included in the request, and their encoded bytes before
    /// base64.
    pub images: Option<u32>,
    pub image_bytes: Option<u64>,
    /// Milliseconds from sending the request to the first streamed delta.
    pub ttft_ms: Option<u64>,
    /// The provider's usage object exactly as received, so normalized
    /// fields can be audited against the provider's own definitions.
    pub raw_usage: Option<serde_json::Value>,
}

pub type SinkError = Box<dyn std::error::Error + Send + Sync>;

/// Receives a response as it streams. Returning an error aborts the call.
pub trait DeltaSink: Send {
    fn delta(&mut self, delta: Delta) -> Result<(), SinkError>;
}

impl<F> DeltaSink for F
where
    F: FnMut(Delta) -> Result<(), SinkError> + Send,
{
    fn delta(&mut self, delta: Delta) -> Result<(), SinkError> {
        self(delta)
    }
}

pub trait Llm: Send + Sync {
    fn stream<'a>(
        &'a self,
        request: &'a Request,
        sink: &'a mut dyn DeltaSink,
    ) -> BoxFuture<'a, Result<Completion, LlmError>>;
}

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("the provider answered HTTP {status}: {body}{}", out_of_credit_hint(*status, body))]
    Http {
        status: u16,
        body: String,
        /// How long the provider asked to be left alone, from its
        /// `retry-after-ms` or `retry-after` header.
        retry_after: Option<Duration>,
    },
    #[error("could not reach the provider: {0}")]
    Transport(String),
    #[error("unexpected provider response: {0}")]
    Protocol(String),
    #[error("could not record the response: {0}")]
    Sink(SinkError),
}

/// What to do when the provider says the account cannot pay: without it,
/// an editor shows a bare HTTP error and the user does not know that hame
/// picked this provider only because its key was the first one found.
fn out_of_credit_hint(status: u16, body: &str) -> &'static str {
    let body = body.to_ascii_lowercase();
    let broke = status == 402
        || [
            "insufficient balance",
            "insufficient_quota",
            "credit balance",
            "billing",
        ]
        .iter()
        .any(|marker| body.contains(marker));
    if broke {
        " (the account has no credit left: top it up, or use another provider with \
         `[model] provider = \"anthropic\"` or `\"openai\"` in ~/.hame/config.toml, or \
         --provider)"
    } else {
        ""
    }
}

impl LlmError {
    /// Whether sending the same request again could succeed.
    pub fn is_retryable(&self) -> bool {
        match self {
            LlmError::Http { status, .. } => *status == 429 || *status >= 500,
            LlmError::Transport(_) => true,
            LlmError::Protocol(_) | LlmError::Sink(_) => false,
        }
    }

    /// Whether the provider refused the request because the prompt does not
    /// fit the model's context window. Compacting and sending it again can
    /// succeed where sending it unchanged never will.
    pub fn is_context_overflow(&self) -> bool {
        let LlmError::Http { status, body, .. } = self else {
            return false;
        };
        let body = body.to_ascii_lowercase();
        matches!(status, 400 | 413)
            && [
                "context_length_exceeded",
                "maximum context length",
                "context window",
                "prompt is too long",
                "input is too long",
                "too many tokens",
                "exceeds the context",
            ]
            .iter()
            .any(|marker| body.contains(marker))
    }
}

/// Turns a failed HTTP response into an error, keeping the start of its body
/// and any wait the provider asked for.
pub(crate) async fn http_error(response: reqwest::Response) -> LlmError {
    let status = response.status().as_u16();
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<f64>().ok())
            .filter(|value| value.is_finite() && *value >= 0.0)
    };
    // An HTTP date in `retry-after` is rare from model APIs and is left to
    // the usual backoff.
    let retry_after = header("retry-after-ms")
        .map(|ms| Duration::from_secs_f64(ms / 1000.0))
        .or_else(|| header("retry-after").map(Duration::from_secs_f64));
    let mut body = response.text().await.unwrap_or_default();
    if body.len() > 2000 {
        let mut end = 2000;
        while !body.is_char_boundary(end) {
            end -= 1;
        }
        body.truncate(end);
    }
    LlmError::Http {
        status,
        body,
        retry_after,
    }
}

/// The longest wait a provider may ask for before a request is given up
/// instead: a limit that resets in an hour is better reported now than
/// waited out in silence.
pub const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

/// Retry policy for failures that happen before any of the response has
/// streamed. A stream that breaks midway is never retried here: part of it
/// has already been delivered to the sink.
#[derive(Debug, Clone, Copy)]
pub struct Retry {
    pub max_retries: u32,
    pub base_delay: Duration,
}

impl Default for Retry {
    fn default() -> Self {
        Self {
            // Five tries spread over a quarter of a minute: three over
            // three seconds lost whole runs to a network that was away
            // for ten.
            max_retries: 5,
            base_delay: Duration::from_millis(500),
        }
    }
}

impl Retry {
    /// How long to wait before trying again after the `attempt`-th retry
    /// (counting from zero) failed with `error`, or `None` to give up.
    ///
    /// A wait the provider asked for is honoured as given, up to
    /// [`MAX_RETRY_AFTER`]; otherwise the delay doubles each time, with up
    /// to a tenth added at random so agents that were limited together do
    /// not all come back in the same instant.
    pub fn delay(&self, attempt: u32, error: &LlmError) -> Option<Duration> {
        if !error.is_retryable() || attempt >= self.max_retries {
            return None;
        }
        if let LlmError::Http {
            retry_after: Some(asked),
            ..
        } = error
        {
            return (*asked <= MAX_RETRY_AFTER).then_some(*asked);
        }
        let backoff = self.base_delay.saturating_mul(2u32.saturating_pow(attempt));
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |now| now.subsec_nanos());
        Some(backoff + backoff.mul_f64(f64::from(nanos % 1000) / 10_000.0))
    }
}

#[cfg(test)]
mod credit_tests {
    use super::*;

    #[test]
    fn an_account_out_of_credit_says_what_to_do() {
        let error = LlmError::Http {
            status: 402,
            body: r#"{"error":{"message":"Insufficient Balance"}}"#.into(),
            retry_after: None,
        };
        assert!(error.to_string().contains("[model] provider"));
        let error = LlmError::Http {
            status: 400,
            body: "bad request".into(),
            retry_after: None,
        };
        assert!(!error.to_string().contains("[model] provider"));
    }

    fn http(status: u16, body: &str, retry_after: Option<Duration>) -> LlmError {
        LlmError::Http {
            status,
            body: body.into(),
            retry_after,
        }
    }

    #[test]
    fn a_requested_wait_is_honoured_up_to_a_ceiling() {
        let retry = Retry {
            max_retries: 3,
            base_delay: Duration::from_millis(100),
        };
        let asked = http(429, "slow down", Some(Duration::from_secs(7)));
        assert_eq!(retry.delay(0, &asked), Some(Duration::from_secs(7)));
        let too_long = http(429, "daily limit", Some(Duration::from_secs(3600)));
        assert_eq!(retry.delay(0, &too_long), None);
        assert_eq!(retry.delay(3, &asked), None, "out of retries");
        assert_eq!(retry.delay(0, &http(400, "bad", None)), None);

        let plain = http(503, "busy", None);
        let first = retry.delay(0, &plain).unwrap();
        let third = retry.delay(2, &plain).unwrap();
        assert!(first >= Duration::from_millis(100) && first <= Duration::from_millis(110));
        assert!(third >= Duration::from_millis(400) && third <= Duration::from_millis(440));
    }

    #[test]
    fn a_prompt_too_long_for_the_window_is_recognized() {
        for body in [
            r#"{"error":{"message":"prompt is too long: 210000 tokens > 200000 maximum"}}"#,
            r#"{"error":{"code":"context_length_exceeded"}}"#,
            "This model's maximum context length is 65536 tokens. However, you requested 70000",
        ] {
            assert!(http(400, body, None).is_context_overflow(), "{body}");
        }
        assert!(!http(400, "invalid tool schema", None).is_context_overflow());
        assert!(!http(500, "maximum context length", None).is_context_overflow());
    }
}
