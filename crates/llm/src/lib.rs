//! The built-in model: no server, the model file pinned by SHA-256 in
//! `models/manifest.json` and loaded with llama.cpp (`llama-cpp-2`), the way `whisper-rs` runs speech.
//! Qwen2.5 Instruct's own chat and tool format: tools in `<tools>`, calls in `<tool_call>`, results in
//! `<tool_response>`. Experimental.
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::LlamaModel;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::sampling::LlamaSampler;
use markon_provider::*;
use serde_json::{Value, json};
use std::num::NonZeroU32;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::mpsc;

pub const MODEL_ID: &str = "qwen2.5-1.5b-instruct";
const CONTEXT: u32 = 8192;
const BATCH: usize = 512;

/// The system and conversation in Qwen2.5's ChatML, ending with the assistant's open turn.
pub fn chatml(req: &ChatRequest) -> String {
    let mut s = format!("<|im_start|>system\n{}", req.system.trim());
    if !req.tools.is_empty() {
        s.push_str(
            "\n\n# Tools\n\nYou may call one or more functions to assist with the user query.\n\n\
             You are provided with function signatures within <tools></tools> XML tags:\n<tools>",
        );
        for t in &req.tools {
            s.push('\n');
            s.push_str(
                &json!({"type": "function", "function": {"name": t.name, "description": t.description, "parameters": t.input_schema}})
                    .to_string(),
            );
        }
        s.push_str(
            "\n</tools>\n\nFor each function call, return a json object with function name and arguments \
             within <tool_call></tool_call> XML tags:\n<tool_call>\n{\"name\": <function-name>, \"arguments\": \
             <args-json-object>}\n</tool_call>",
        );
    }
    s.push_str("<|im_end|>\n");
    for m in &req.messages {
        let mut text = String::new();
        let mut calls = Vec::new();
        let mut results = Vec::new();
        for b in &m.content {
            match b["type"].as_str().unwrap_or("") {
                "text" => text.push_str(b["text"].as_str().unwrap_or("")),
                "tool_use" => {
                    calls.push(json!({"name": b["name"], "arguments": b["input"]}).to_string())
                }
                "tool_result" => results.push(match &b["content"] {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                }),
                _ => {}
            }
        }
        match m.role {
            Role::User if !results.is_empty() => {
                s.push_str("<|im_start|>user");
                for r in results {
                    s.push_str(&format!("\n<tool_response>\n{r}\n</tool_response>"));
                }
                if !text.is_empty() {
                    s.push_str(&format!("\n{text}"));
                }
                s.push_str("<|im_end|>\n");
            }
            Role::User => s.push_str(&format!("<|im_start|>user\n{text}<|im_end|>\n")),
            Role::Assistant => {
                s.push_str("<|im_start|>assistant\n");
                s.push_str(&text);
                for c in calls {
                    s.push_str(&format!("\n<tool_call>\n{c}\n</tool_call>"));
                }
                s.push_str("<|im_end|>\n");
            }
        }
    }
    s.push_str("<|im_start|>assistant\n");
    s
}

/// Split what the model wrote into spoken text and tool calls.
pub fn parse_calls(out: &str) -> (String, Vec<(String, Value)>) {
    let mut text = String::new();
    let mut calls = Vec::new();
    let mut rest = out;
    while let Some(start) = rest.find("<tool_call>") {
        text.push_str(&rest[..start]);
        let after = &rest[start + "<tool_call>".len()..];
        let end = after.find("</tool_call>").unwrap_or(after.len());
        if let Ok(v) = serde_json::from_str::<Value>(after[..end].trim())
            && let Some(name) = v["name"].as_str()
        {
            let args = match &v["arguments"] {
                Value::String(s) => serde_json::from_str(s).unwrap_or(json!({})),
                Value::Null => json!({}),
                a => a.clone(),
            };
            calls.push((name.to_string(), args));
        }
        rest = after.get(end + "</tool_call>".len()..).unwrap_or("");
    }
    text.push_str(rest);
    (text.trim().to_string(), calls)
}

/// How much of `buf` is safe to speak now: everything except a tail that could be the start of a
/// `<tool_call>` tag, and nothing once a tag has begun.
pub fn speakable(buf: &str) -> usize {
    if let Some(i) = buf.find("<tool_call>") {
        return i;
    }
    let tag = "<tool_call>";
    (1..tag.len())
        .rev()
        .find(|n| buf.ends_with(&tag[..*n]))
        .map(|n| buf.len() - n)
        .unwrap_or(buf.len())
}

fn backend() -> Result<&'static LlamaBackend, ProviderError> {
    static B: OnceLock<Result<LlamaBackend, String>> = OnceLock::new();
    B.get_or_init(|| {
        LlamaBackend::init()
            .map(|mut b| {
                b.void_logs();
                b
            })
            .map_err(|e| e.to_string())
    })
    .as_ref()
    .map_err(|e| ProviderError::Network(format!("llama.cpp did not start: {e}")))
}

/// The built-in model, loaded once. Generation is one at a time (llama.cpp contexts are not safe
/// to build in parallel).
pub struct BuiltIn {
    model: Arc<LlamaModel>,
    lock: Arc<Mutex<()>>,
}

impl BuiltIn {
    pub fn load(path: &Path) -> Result<Self, ProviderError> {
        let b = backend()?;
        let params = LlamaModelParams::default().with_n_gpu_layers(1000);
        let model = LlamaModel::load_from_file(b, path, &params)
            .map_err(|e| ProviderError::Network(format!("the built-in model did not load: {e}")))?;
        Ok(Self {
            model: Arc::new(model),
            lock: Arc::new(Mutex::new(())),
        })
    }
}

type Tx = mpsc::Sender<Result<ChatDelta, ProviderError>>;

fn generate(
    model: &LlamaModel,
    prompt: &str,
    max_tokens: u32,
    tx: &Tx,
) -> Result<(String, u64, u64), String> {
    let b = backend().map_err(|e| e.to_string())?;
    let mut ctx = model
        .new_context(
            b,
            LlamaContextParams::default()
                .with_n_ctx(NonZeroU32::new(CONTEXT))
                .with_n_batch(BATCH as u32),
        )
        .map_err(|e| e.to_string())?;
    let vocab = model.vocab();
    let tokens = vocab.tokenize(prompt.as_bytes(), false, true);
    if tokens.len() as u32 + 16 >= CONTEXT {
        return Err(format!(
            "the conversation is too long for the built-in model ({} tokens)",
            tokens.len()
        ));
    }
    let mut batch = LlamaBatch::new(BATCH, 1);
    for (chunk_i, chunk) in tokens.chunks(BATCH).enumerate() {
        batch.clear();
        let last_chunk = (chunk_i + 1) * BATCH >= tokens.len();
        for (j, t) in chunk.iter().enumerate() {
            let pos = (chunk_i * BATCH + j) as i32;
            batch
                .add(*t, pos, &[0], last_chunk && j + 1 == chunk.len())
                .map_err(|e| e.to_string())?;
        }
        ctx.decode(&mut batch).map_err(|e| e.to_string())?;
    }
    let mut sampler = LlamaSampler::chain_simple([
        LlamaSampler::top_k(20),
        LlamaSampler::top_p(0.8, 1),
        LlamaSampler::temp(0.7),
        LlamaSampler::dist(1234),
    ]);
    let mut out = String::new();
    let mut sent = 0;
    let mut pending: Vec<u8> = Vec::new();
    let limit = (CONTEXT - tokens.len() as u32 - 8).min(max_tokens.max(64));
    let mut n_out = 0u64;
    for pos in (tokens.len() as i32..).take(limit as usize) {
        let tok = sampler.sample(&ctx, batch.n_tokens() - 1);
        sampler.accept(tok);
        if vocab.is_eog(tok) {
            break;
        }
        n_out += 1;
        pending.extend(vocab.token_to_piece(tok, false, None));
        // A multi-byte character can span tokens: only whole characters are passed on.
        if let Ok(s) = std::str::from_utf8(&pending) {
            out.push_str(s);
            pending.clear();
        }
        let safe = speakable(&out);
        if safe > sent {
            if tx
                .blocking_send(Ok(ChatDelta::Text(out[sent..safe].to_string())))
                .is_err()
            {
                return Ok((out, tokens.len() as u64, n_out)); // the turn was cancelled
            }
            sent = safe;
        }
        batch.clear();
        batch.add(tok, pos, &[0], true).map_err(|e| e.to_string())?;
        ctx.decode(&mut batch).map_err(|e| e.to_string())?;
    }
    Ok((out, tokens.len() as u64, n_out))
}

impl ModelProvider for BuiltIn {
    fn route(&self) -> &'static str {
        "builtin"
    }
    fn stream(&self, req: ChatRequest) -> DeltaStream {
        let (tx, rx) = mpsc::channel(256);
        let (model, lock) = (self.model.clone(), self.lock.clone());
        let prompt = chatml(&req);
        tokio::task::spawn_blocking(move || {
            let _one_at_a_time = lock.lock().unwrap_or_else(|e| e.into_inner());
            match generate(&model, &prompt, req.max_tokens, &tx) {
                Ok((out, n_in, n_out)) => {
                    let (text, calls) = parse_calls(&out);
                    if !text.is_empty() {
                        let _ = tx.blocking_send(Ok(ChatDelta::Block(
                            json!({"type": "text", "text": text}),
                        )));
                    }
                    for (i, (name, input)) in calls.iter().enumerate() {
                        let id = format!("local-{}-{i}", n_in + n_out);
                        let _ = tx.blocking_send(Ok(ChatDelta::Block(
                            json!({"type": "tool_use", "id": id, "name": name, "input": input}),
                        )));
                        let _ = tx.blocking_send(Ok(ChatDelta::ToolCall {
                            id,
                            name: name.clone(),
                            input: input.clone(),
                        }));
                    }
                    let _ = tx.blocking_send(Ok(ChatDelta::Usage {
                        input_tokens: n_in,
                        output_tokens: n_out,
                    }));
                    let stop = if calls.is_empty() {
                        StopReason::EndTurn
                    } else {
                        StopReason::ToolUse
                    };
                    let _ = tx.blocking_send(Ok(ChatDelta::Stop(stop)));
                }
                Err(e) => {
                    let _ = tx.blocking_send(Err(ProviderError::Network(e)));
                }
            }
        });
        rx
    }
    fn models(&self) -> Vec<ModelInfo> {
        vec![ModelInfo {
            id: MODEL_ID.into(),
            name: "Qwen2.5 1.5B, built in (experimental)".into(),
        }]
    }
    fn default_model(&self) -> String {
        MODEL_ID.into()
    }
    fn validate(&self) -> BoxFuture<'_, Result<Account, ProviderError>> {
        Box::pin(async {
            Ok(Account {
                route: "builtin".into(),
                plan: Some("on this Mac".into()),
                balance: None,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req() -> ChatRequest {
        ChatRequest {
            model: MODEL_ID.into(),
            system: "You are Markon.".into(),
            messages: vec![
                Message::user_text("weather?"),
                Message {
                    role: Role::Assistant,
                    content: vec![
                        json!({"type": "tool_use", "id": "a", "name": "fetch_page", "input": {"url": "https://wttr.in"}}),
                    ],
                },
                Message::tool_results(vec![ToolResult {
                    id: "a".into(),
                    content: "Sunny 21C".into(),
                    is_error: false,
                }]),
            ],
            tools: vec![ToolSpec {
                name: "fetch_page".into(),
                description: "Fetch".into(),
                input_schema: json!({"type": "object"}),
            }],
            max_tokens: 100,
        }
    }

    #[test]
    fn the_prompt_is_qwen_chatml_with_tools_calls_and_results() {
        let p = chatml(&req());
        assert!(p.starts_with("<|im_start|>system\nYou are Markon.\n\n# Tools"));
        assert!(p.contains("\"name\":\"fetch_page\""));
        assert!(p.contains("<|im_start|>assistant\n\n<tool_call>\n{\"arguments\":{\"url\":\"https://wttr.in\"},\"name\":\"fetch_page\"}\n</tool_call><|im_end|>"));
        assert!(
            p.contains("<|im_start|>user\n<tool_response>\nSunny 21C\n</tool_response><|im_end|>")
        );
        assert!(p.ends_with("<|im_start|>assistant\n"));
    }

    #[test]
    fn tool_calls_are_read_and_never_spoken() {
        let (text, calls) = parse_calls(
            "Let me check.\n<tool_call>\n{\"name\": \"fetch_page\", \"arguments\": {\"url\": \"https://x\"}}\n</tool_call>",
        );
        assert_eq!(text, "Let me check.");
        assert_eq!(
            calls,
            [("fetch_page".to_string(), json!({"url": "https://x"}))]
        );
        let (_, c) =
            parse_calls("<tool_call>{\"name\":\"a\",\"arguments\":\"{\\\"k\\\":1}\"}</tool_call>");
        assert_eq!(c[0].1, json!({"k": 1}), "arguments given as a string");
        assert_eq!(parse_calls("<tool_call>not json</tool_call>").1.len(), 0);
        assert_eq!(speakable("Hello <tool"), 6);
        assert_eq!(speakable("Hello <tool_call>{"), 6);
        assert_eq!(speakable("1 < 2"), 5);
    }

    /// Runs the real model when MARKON_LLM_MODEL points at the GGUF file; `cargo test -p markon-llm -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn the_real_model_answers_and_calls_a_tool() {
        let path = std::env::var("MARKON_LLM_MODEL").expect("MARKON_LLM_MODEL");
        let m = BuiltIn::load(Path::new(&path)).unwrap();
        let mut r = req();
        r.messages.truncate(1);
        r.messages[0] =
            Message::user_text("Fetch https://wttr.in/?format=3 and tell me the weather.");
        let mut rx = m.stream(r);
        let mut got = Vec::new();
        while let Some(d) = rx.recv().await {
            got.push(d.unwrap());
        }
        assert!(
            got.iter()
                .any(|d| matches!(d, ChatDelta::ToolCall { name, .. } if name == "fetch_page")),
            "{got:?}"
        );
    }
}
