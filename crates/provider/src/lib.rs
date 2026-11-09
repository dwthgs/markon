//! One trait, several model routes. Deltas from every route are normalised to [`ChatDelta`].
//! Keys are read from a [`KeySource`] per request and never logged.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::mpsc;

pub mod anthropic;
pub mod fake;
pub mod sse;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

/// A message is a role plus raw content blocks in the Messages API shape. Assistant blocks
/// (including thinking blocks) are replayed exactly as the model produced them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<Value>,
}

impl Message {
    pub fn user_text(text: &str) -> Self {
        Self { role: Role::User, content: vec![serde_json::json!({"type": "text", "text": text})] }
    }
    pub fn tool_results(results: Vec<ToolResult>) -> Self {
        Self {
            role: Role::User,
            content: results
                .into_iter()
                .map(|r| serde_json::json!({"type": "tool_result", "tool_use_id": r.id, "content": r.content, "is_error": r.is_error}))
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolResult {
    pub id: String,
    pub content: String,
    pub is_error: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Debug, Clone, Default)]
pub struct ChatRequest {
    pub model: String,
    pub system: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    pub max_tokens: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    Refusal,
    Other(String),
}

impl StopReason {
    fn parse(s: &str) -> Self {
        match s {
            "end_turn" | "stop" => Self::EndTurn,
            "tool_use" | "tool_calls" => Self::ToolUse,
            "max_tokens" | "length" => Self::MaxTokens,
            "refusal" | "content_filter" => Self::Refusal,
            o => Self::Other(o.into()),
        }
    }
}

/// The one internal shape every provider streams.
#[derive(Debug, Clone, PartialEq)]
pub enum ChatDelta {
    Text(String),
    ToolCall { id: String, name: String, input: Value },
    /// A finished assistant content block, to append to the history verbatim.
    Block(Value),
    Usage { input_tokens: u64, output_tokens: u64 },
    Stop(StopReason),
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Account {
    pub route: String,
    pub plan: Option<String>,
    pub balance: Option<String>,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ProviderError {
    #[error("no API key is set for this route")]
    NoKey,
    #[error("the key was rejected")]
    Auth,
    #[error("rate limited")]
    RateLimited { retry_after_s: Option<u64> },
    #[error("the model service is overloaded")]
    Overloaded,
    #[error("free trial used up for today")]
    QuotaExhausted { code: String },
    #[error("model {0} is not available")]
    ModelUnavailable(String),
    #[error("http {status}: {message}")]
    Http { status: u16, message: String },
    #[error("network: {0}")]
    Network(String),
    #[error("bad response: {0}")]
    Parse(String),
}

impl ProviderError {
    /// 429 and 529 are retried with back-off by the agent loop.
    pub fn retryable(&self) -> bool {
        matches!(self, Self::RateLimited { .. } | Self::Overloaded)
    }

    fn from_status(status: u16, body: &str, retry_after_s: Option<u64>) -> Self {
        let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
        let message = v["error"]["message"].as_str().unwrap_or(body).chars().take(300).collect::<String>();
        let code = v["error"]["code"].as_str().or(v["code"].as_str()).unwrap_or_default();
        match status {
            401 | 403 => Self::Auth,
            402 if code.starts_with("quota") => Self::QuotaExhausted { code: code.into() },
            429 if code.starts_with("quota") => Self::QuotaExhausted { code: code.into() },
            429 => Self::RateLimited { retry_after_s },
            529 | 503 => Self::Overloaded,
            404 => Self::ModelUnavailable(message),
            _ => Self::Http { status, message },
        }
    }
}

pub type DeltaStream = mpsc::Receiver<Result<ChatDelta, ProviderError>>;

/// Where a route gets its key: the keychain in the app, a fixed value in tests.
pub type KeySource = Arc<dyn Fn() -> Option<String> + Send + Sync>;

pub trait ModelProvider: Send + Sync {
    /// Short route id: "anthropic", "openrouter", "relay", "fake".
    fn route(&self) -> &'static str;
    fn stream(&self, req: ChatRequest) -> DeltaStream;
    fn models(&self) -> Vec<ModelInfo>;
    fn default_model(&self) -> String;
    fn validate(&self) -> BoxFuture<'_, Result<Account, ProviderError>>;
}

pub(crate) fn retry_after(h: &reqwest::header::HeaderMap) -> Option<u64> {
    h.get("retry-after")?.to_str().ok()?.parse().ok()
}
