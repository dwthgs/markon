//! Anthropic Messages API over SSE. Used directly with the user's key (`AnthropicDirect`) and,
//! with a bearer token, through our relay.
use crate::sse::SseParser;
use crate::*;
use serde_json::json;
use std::collections::BTreeMap;
use std::time::Duration;

pub const API_VERSION: &str = "2026-10-21";
pub const DEFAULT_MODEL: &str = "claude-opus-5-5";
/// Server-side refusal fallback, on by default for the 5.5 models (spec security review: tell the user).
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";

pub enum Auth {
    ApiKey(KeySource),
    Bearer(KeySource),
}

pub struct MessagesClient {
    pub(crate) http: reqwest::Client,
    pub(crate) base_url: String,
    pub(crate) auth: Auth,
    pub(crate) fallbacks: bool,
}

impl MessagesClient {
    pub fn new(base_url: impl Into<String>, auth: Auth, fallbacks: bool) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .expect("http client");
        Self { http, base_url: base_url.into(), auth, fallbacks }
    }

    fn authed(&self, rb: reqwest::RequestBuilder) -> Result<reqwest::RequestBuilder, ProviderError> {
        let rb = rb.header("anthropic-version", API_VERSION);
        Ok(match &self.auth {
            Auth::ApiKey(k) => rb.header("x-api-key", k().ok_or(ProviderError::NoKey)?),
            Auth::Bearer(k) => rb.bearer_auth(k().ok_or(ProviderError::NoKey)?),
        })
    }

    pub fn body(&self, req: &ChatRequest) -> Value {
        let mut b = json!({
            "model": req.model,
            "max_tokens": req.max_tokens,
            "system": req.system,
            "messages": req.messages,
            "stream": true,
        });
        if !req.tools.is_empty() {
            b["tools"] = json!(req.tools);
        }
        if self.fallbacks {
            b["fallbacks"] = json!("default");
        }
        b
    }

    pub fn stream(&self, req: ChatRequest) -> DeltaStream {
        let (tx, rx) = mpsc::channel(64);
        let url = format!("{}/v1/messages", self.base_url);
        let body = self.body(&req);
        let rb = self.authed(self.http.post(url).json(&body)).map(|rb| {
            if self.fallbacks { rb.header("anthropic-beta", FALLBACK_BETA) } else { rb }
        });
        tokio::spawn(async move {
            let rb = match rb {
                Ok(rb) => rb,
                Err(e) => {
                    let _ = tx.send(Err(e)).await;
                    return;
                }
            };
            if let Err(e) = run_stream(rb, &tx).await {
                let _ = tx.send(Err(e)).await;
            }
        });
        rx
    }

    pub async fn list_models(&self) -> Result<Value, ProviderError> {
        let rb = self.authed(self.http.get(format!("{}/v1/models", self.base_url)).timeout(Duration::from_secs(3)))?;
        let resp = rb.send().await.map_err(|e| ProviderError::Network(e.without_url().to_string()))?;
        let status = resp.status().as_u16();
        let ra = retry_after(resp.headers());
        let text = resp.text().await.unwrap_or_default();
        if status >= 400 {
            return Err(ProviderError::from_status(status, &text, ra));
        }
        serde_json::from_str(&text).map_err(|e| ProviderError::Parse(e.to_string()))
    }
}

#[derive(Default)]
struct Block {
    value: Value,
    text: String,
    json: String,
    signature: String,
}

async fn run_stream(
    rb: reqwest::RequestBuilder,
    tx: &mpsc::Sender<Result<ChatDelta, ProviderError>>,
) -> Result<(), ProviderError> {
    // without_url keeps request URLs (and anything in them) out of error text.
    let mut resp = rb.send().await.map_err(|e| ProviderError::Network(e.without_url().to_string()))?;
    let status = resp.status().as_u16();
    if status >= 400 {
        let ra = retry_after(resp.headers());
        let text = resp.text().await.unwrap_or_default();
        return Err(ProviderError::from_status(status, &text, ra));
    }
    let mut parser = SseParser::default();
    let mut blocks: BTreeMap<u64, Block> = BTreeMap::new();
    let (mut input_tokens, mut output_tokens) = (0u64, 0u64);
    let mut stop = None;
    let send = |d| tx.send(Ok(d));
    while let Some(chunk) = resp.chunk().await.map_err(|e| ProviderError::Network(e.without_url().to_string()))? {
        for ev in parser.push(&chunk) {
            let v: Value = serde_json::from_str(&ev.data).map_err(|e| ProviderError::Parse(e.to_string()))?;
            match v["type"].as_str().unwrap_or(&ev.event) {
                "message_start" => input_tokens = v["message"]["usage"]["input_tokens"].as_u64().unwrap_or(0),
                "content_block_start" => {
                    let i = v["index"].as_u64().unwrap_or(0);
                    blocks.insert(i, Block { value: v["content_block"].clone(), ..Default::default() });
                }
                "content_block_delta" => {
                    let i = v["index"].as_u64().unwrap_or(0);
                    let b = blocks.entry(i).or_default();
                    let d = &v["delta"];
                    match d["type"].as_str().unwrap_or_default() {
                        "text_delta" => {
                            let t = d["text"].as_str().unwrap_or_default();
                            b.text.push_str(t);
                            if send(ChatDelta::Text(t.into())).await.is_err() {
                                return Ok(()); // receiver dropped: the turn was cancelled
                            }
                        }
                        "input_json_delta" => b.json.push_str(d["partial_json"].as_str().unwrap_or_default()),
                        "thinking_delta" => b.text.push_str(d["thinking"].as_str().unwrap_or_default()),
                        "signature_delta" => b.signature.push_str(d["signature"].as_str().unwrap_or_default()),
                        _ => {}
                    }
                }
                "content_block_stop" => {
                    let i = v["index"].as_u64().unwrap_or(0);
                    let Some(mut b) = blocks.remove(&i) else { continue };
                    match b.value["type"].as_str().unwrap_or_default() {
                        "text" => b.value["text"] = json!(b.text),
                        "thinking" => {
                            b.value["thinking"] = json!(b.text);
                            if !b.signature.is_empty() {
                                b.value["signature"] = json!(b.signature);
                            }
                        }
                        "tool_use" => {
                            let input: Value = if b.json.trim().is_empty() {
                                json!({})
                            } else {
                                serde_json::from_str(&b.json).unwrap_or(Value::Null)
                            };
                            b.value["input"] = input.clone();
                            let id = b.value["id"].as_str().unwrap_or_default().to_string();
                            let name = b.value["name"].as_str().unwrap_or_default().to_string();
                            send(ChatDelta::Block(b.value)).await.ok();
                            send(ChatDelta::ToolCall { id, name, input }).await.ok();
                            continue;
                        }
                        _ => {}
                    }
                    send(ChatDelta::Block(b.value)).await.ok();
                }
                "message_delta" => {
                    if let Some(s) = v["delta"]["stop_reason"].as_str() {
                        stop = Some(StopReason::parse(s));
                    }
                    output_tokens = v["usage"]["output_tokens"].as_u64().unwrap_or(output_tokens);
                }
                "message_stop" => {
                    send(ChatDelta::Usage { input_tokens, output_tokens }).await.ok();
                    send(ChatDelta::Stop(stop.take().unwrap_or(StopReason::EndTurn))).await.ok();
                    return Ok(());
                }
                "error" => {
                    let kind = v["error"]["type"].as_str().unwrap_or_default();
                    return Err(match kind {
                        "overloaded_error" => ProviderError::Overloaded,
                        "rate_limit_error" => ProviderError::RateLimited { retry_after_s: None },
                        _ => ProviderError::Http { status: 500, message: v["error"]["message"].as_str().unwrap_or(kind).into() },
                    });
                }
                _ => {}
            }
        }
    }
    Err(ProviderError::Network("stream ended before the reply finished".into()))
}

/// Route A: the user's own Anthropic key from the keychain.
pub struct AnthropicDirect {
    client: MessagesClient,
}

impl AnthropicDirect {
    pub fn new(key: KeySource) -> Self {
        Self::with_base_url("https://api.anthropic.com", key)
    }
    pub fn with_base_url(base_url: &str, key: KeySource) -> Self {
        Self { client: MessagesClient::new(base_url, Auth::ApiKey(key), true) }
    }
}

impl ModelProvider for AnthropicDirect {
    fn route(&self) -> &'static str {
        "anthropic"
    }
    fn stream(&self, req: ChatRequest) -> DeltaStream {
        self.client.stream(req)
    }
    fn models(&self) -> Vec<ModelInfo> {
        [("claude-opus-5-5", "Claude Opus 5.5"), ("claude-sonnet-5-5", "Claude Sonnet 5.5"), ("claude-haiku-5-5", "Claude Haiku 5.5")]
            .into_iter()
            .map(|(id, name)| ModelInfo { id: id.into(), name: name.into() })
            .collect()
    }
    fn default_model(&self) -> String {
        DEFAULT_MODEL.into()
    }
    fn validate(&self) -> BoxFuture<'_, Result<Account, ProviderError>> {
        Box::pin(async move {
            self.client.list_models().await?;
            // The Anthropic API does not expose plan or balance.
            Ok(Account { route: "anthropic".into(), plan: None, balance: None })
        })
    }
}
