//! The model seam: one streaming call, expressed in the session vocabulary.
//!
//! An adapter turns a [`Request`] into provider wire format, and the
//! provider's stream back into [`Delta`]s pushed at a [`DeltaSink`]. The
//! caller owns the sink, which is how the agent journals a response while it
//! is still arriving.

mod anthropic;
mod chat_completions;
#[cfg(test)]
mod testing;

use std::time::Duration;

pub use anthropic::Anthropic;
pub use chat_completions::ChatCompletions;
pub use hx_kernel::BoxFuture;
use hx_session::{Delta, History, Message, StopReason, ToolSpec, Usage};

/// Everything one model call is built from.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Request {
    pub system: Option<String>,
    pub tools: Vec<ToolSpec>,
    pub messages: Vec<Message>,
    pub max_tokens: Option<u32>,
}

impl From<History> for Request {
    fn from(history: History) -> Self {
        Self {
            system: history.system,
            tools: history.tools,
            messages: history.messages,
            max_tokens: None,
        }
    }
}

/// How a response ended, once its stream is complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Completion {
    pub stop: StopReason,
    pub usage: Usage,
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
    #[error("the provider answered HTTP {status}: {body}")]
    Http { status: u16, body: String },
    #[error("could not reach the provider: {0}")]
    Transport(String),
    #[error("unexpected provider response: {0}")]
    Protocol(String),
    #[error("could not record the response: {0}")]
    Sink(SinkError),
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
}

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
            max_retries: 3,
            base_delay: Duration::from_millis(500),
        }
    }
}
