//! Route B: the user's own OpenRouter key, OpenAI-compatible chat completions over SSE.
//! History stays in the Anthropic block shape inside Markon and is converted here.
use crate::sse::SseParser;
use crate::*;
use serde_json::json;
use std::collections::BTreeMap;
use std::time::Duration;

const CURATED: &str = include_str!("../data/openrouter-models.json");

pub fn curated() -> (String, Vec<ModelInfo>) {
    let v: Value = serde_json::from_str(CURATED).expect("curated list");
    let models = v["models"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|m| ModelInfo {
                    id: m["id"].as_str().unwrap_or("").into(),
                    name: m["name"].as_str().unwrap_or("").into(),
                })
                .collect()
        })
        .unwrap_or_default();
    (v["default"].as_str().unwrap_or("").into(), models)
}

pub struct OpenRouter {
    http: reqwest::Client,
    base_url: String,
    key: KeySource,
}

impl OpenRouter {
    pub fn new(key: KeySource) -> Self {
        Self::with_base_url("https://openrouter.ai/api", key)
    }
    pub fn with_base_url(base_url: &str, key: KeySource) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .expect("http client");
        Self {
            http,
            base_url: base_url.into(),
            key,
        }
    }
}

/// Anthropic-shaped history → OpenAI chat messages. Thinking blocks are Anthropic-only and dropped.
pub fn to_openai(req: &ChatRequest) -> Value {
    let mut msgs = vec![json!({"role": "system", "content": req.system})];
    for m in &req.messages {
        let mut text = String::new();
        let mut calls = Vec::new();
        for b in &m.content {
            match b["type"].as_str().unwrap_or_default() {
                "text" => text.push_str(b["text"].as_str().unwrap_or_default()),
                "tool_use" => calls.push(json!({
                    "id": b["id"], "type": "function",
                    "function": {"name": b["name"], "arguments": b["input"].to_string()}
                })),
                "tool_result" => msgs.push(json!({"role": "tool", "tool_call_id": b["tool_use_id"], "content": b["content"]})),
                _ => {}
            }
        }
        match m.role {
            Role::User if !text.is_empty() => msgs.push(json!({"role": "user", "content": text})),
            Role::Assistant if !text.is_empty() || !calls.is_empty() => {
                let mut a = json!({"role": "assistant", "content": if text.is_empty() { Value::Null } else { json!(text) }});
                if !calls.is_empty() {
                    a["tool_calls"] = json!(calls);
                }
                msgs.push(a);
            }
            _ => {}
        }
    }
    let mut body = json!({
        "model": req.model,
        "messages": msgs,
        "max_tokens": req.max_tokens,
        "stream": true,
        "stream_options": {"include_usage": true},
    });
    if !req.tools.is_empty() {
        body["tools"] = json!(req.tools.iter().map(|t| json!({"type": "function", "function": {"name": t.name, "description": t.description, "parameters": t.input_schema}})).collect::<Vec<_>>());
    }
    body
}

#[derive(Default)]
struct Call {
    id: String,
    name: String,
    args: String,
}

async fn run(
    rb: reqwest::RequestBuilder,
    tx: &mpsc::Sender<Result<ChatDelta, ProviderError>>,
) -> Result<(), ProviderError> {
    let mut resp = rb
        .send()
        .await
        .map_err(|e| ProviderError::Network(e.without_url().to_string()))?;
    let status = resp.status().as_u16();
    if status >= 400 {
        let ra = retry_after(resp.headers());
        let text = resp.text().await.unwrap_or_default();
        return Err(ProviderError::from_status(status, &text, ra));
    }
    let mut parser = SseParser::default();
    let mut text = String::new();
    let mut calls: BTreeMap<u64, Call> = BTreeMap::new();
    let mut stop = None;
    let mut usage = (0, 0);
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| ProviderError::Network(e.without_url().to_string()))?
    {
        for ev in parser.push(&chunk) {
            if ev.data.trim() == "[DONE]" {
                if !text.is_empty() {
                    tx.send(Ok(ChatDelta::Block(json!({"type": "text", "text": text}))))
                        .await
                        .ok();
                }
                for c in calls.values() {
                    let input: Value = if c.args.trim().is_empty() {
                        json!({})
                    } else {
                        serde_json::from_str(&c.args).unwrap_or(Value::Null)
                    };
                    tx.send(Ok(ChatDelta::Block(
                        json!({"type": "tool_use", "id": c.id, "name": c.name, "input": input}),
                    )))
                    .await
                    .ok();
                    tx.send(Ok(ChatDelta::ToolCall {
                        id: c.id.clone(),
                        name: c.name.clone(),
                        input,
                    }))
                    .await
                    .ok();
                }
                tx.send(Ok(ChatDelta::Usage {
                    input_tokens: usage.0,
                    output_tokens: usage.1,
                }))
                .await
                .ok();
                tx.send(Ok(ChatDelta::Stop(
                    stop.take().unwrap_or(StopReason::EndTurn),
                )))
                .await
                .ok();
                return Ok(());
            }
            let v: Value =
                serde_json::from_str(&ev.data).map_err(|e| ProviderError::Parse(e.to_string()))?;
            if let Some(err) = v.get("error") {
                return Err(ProviderError::Http {
                    status: err["code"].as_u64().unwrap_or(500) as u16,
                    message: err["message"].as_str().unwrap_or("error").into(),
                });
            }
            if let Some(u) = v.get("usage").filter(|u| !u.is_null()) {
                usage = (
                    u["prompt_tokens"].as_u64().unwrap_or(0),
                    u["completion_tokens"].as_u64().unwrap_or(0),
                );
            }
            let Some(choice) = v["choices"].get(0) else {
                continue;
            };
            let d = &choice["delta"];
            if let Some(t) = d["content"].as_str().filter(|t| !t.is_empty()) {
                text.push_str(t);
                if tx.send(Ok(ChatDelta::Text(t.into()))).await.is_err() {
                    return Ok(());
                }
            }
            for tc in d["tool_calls"].as_array().into_iter().flatten() {
                let c = calls.entry(tc["index"].as_u64().unwrap_or(0)).or_default();
                if let Some(id) = tc["id"].as_str() {
                    c.id = id.into();
                }
                if let Some(n) = tc["function"]["name"].as_str() {
                    c.name.push_str(n);
                }
                c.args
                    .push_str(tc["function"]["arguments"].as_str().unwrap_or_default());
            }
            if let Some(r) = choice["finish_reason"].as_str() {
                stop = Some(StopReason::parse(r));
            }
        }
    }
    Err(ProviderError::Network(
        "stream ended before the reply finished".into(),
    ))
}

impl ModelProvider for OpenRouter {
    fn route(&self) -> &'static str {
        "openrouter"
    }
    fn stream(&self, req: ChatRequest) -> DeltaStream {
        let (tx, rx) = mpsc::channel(64);
        let rb = (self.key)().ok_or(ProviderError::NoKey).map(|k| {
            self.http
                .post(format!("{}/v1/chat/completions", self.base_url))
                .bearer_auth(k)
                .header("X-Title", "Markon")
                .json(&to_openai(&req))
        });
        tokio::spawn(async move {
            let r = match rb {
                Ok(rb) => run(rb, &tx).await,
                Err(e) => Err(e),
            };
            if let Err(e) = r {
                let _ = tx.send(Err(e)).await;
            }
        });
        rx
    }
    fn models(&self) -> Vec<ModelInfo> {
        curated().1
    }
    fn default_model(&self) -> String {
        curated().0
    }
    fn validate(&self) -> BoxFuture<'_, Result<Account, ProviderError>> {
        Box::pin(async move {
            let key = (self.key)().ok_or(ProviderError::NoKey)?;
            let resp = self
                .http
                .get(format!("{}/v1/key", self.base_url))
                .bearer_auth(key)
                .timeout(Duration::from_secs(3))
                .send()
                .await
                .map_err(|e| ProviderError::Network(e.without_url().to_string()))?;
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            if status >= 400 {
                return Err(ProviderError::from_status(status, &text, None));
            }
            let v: Value =
                serde_json::from_str(&text).map_err(|e| ProviderError::Parse(e.to_string()))?;
            let d = &v["data"];
            let balance = d["limit_remaining"]
                .as_f64()
                .map(|r| format!("${r:.2} left"))
                .or_else(|| d["usage"].as_f64().map(|u| format!("${u:.2} used")));
            Ok(Account {
                route: "openrouter".into(),
                plan: d["is_free_tier"]
                    .as_bool()
                    .map(|f| if f { "free tier".into() } else { "paid".into() }),
                balance,
            })
        })
    }
}
