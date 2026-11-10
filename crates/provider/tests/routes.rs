//! OpenRouter and Relay routes against local mock servers.
use markon_provider::openrouter::{OpenRouter, curated, to_openai};
use markon_provider::relay::Relay;
use markon_provider::*;
use serde_json::json;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

async fn mock(responses: Vec<String>) -> (String, Arc<Mutex<Vec<String>>>) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s = seen.clone();
    tokio::spawn(async move {
        for r in responses {
            let (mut c, _) = l.accept().await.unwrap();
            let mut buf = vec![0u8; 65536];
            let n = c.read(&mut buf).await.unwrap();
            s.lock()
                .unwrap()
                .push(String::from_utf8_lossy(&buf[..n]).to_string());
            c.write_all(r.as_bytes()).await.unwrap();
            c.shutdown().await.ok();
        }
    });
    (url, seen)
}

fn ok(ct: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: {ct}\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    )
}
fn status(code: u16, body: &str) -> String {
    format!(
        "HTTP/1.1 {code} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    )
}
async fn collect(mut rx: DeltaStream) -> Vec<Result<ChatDelta, ProviderError>> {
    let mut v = Vec::new();
    while let Some(d) = rx.recv().await {
        v.push(d);
    }
    v
}
fn req(model: &str) -> ChatRequest {
    ChatRequest {
        model: model.into(),
        system: "sys".into(),
        messages: vec![Message::user_text("hi")],
        tools: vec![],
        max_tokens: 100,
    }
}

#[test]
fn openrouter_offers_only_the_curated_list() {
    let p = OpenRouter::new(Arc::new(|| None));
    let (default, list) = curated();
    assert_eq!(p.models(), list);
    assert!(list.iter().any(|m| m.id == default));
    assert!(list.len() >= 3);
}

#[test]
fn history_converts_to_openai_messages() {
    let mut r = req("m");
    r.messages.push(Message {
        role: Role::Assistant,
        content: vec![
            json!({"type": "thinking", "thinking": "", "signature": "s"}),
            json!({"type": "text", "text": "Checking."}),
            json!({"type": "tool_use", "id": "t1", "name": "read_file", "input": {"path": "/a"}}),
        ],
    });
    r.messages.push(Message::tool_results(vec![ToolResult {
        id: "t1".into(),
        content: "data".into(),
        is_error: false,
    }]));
    r.tools.push(ToolSpec {
        name: "read_file".into(),
        description: "d".into(),
        input_schema: json!({"type": "object"}),
    });
    let b = to_openai(&r);
    let m = b["messages"].as_array().unwrap();
    assert_eq!(m[0]["role"], "system");
    assert_eq!(
        m[2]["tool_calls"][0]["function"]["arguments"],
        "{\"path\":\"/a\"}"
    );
    assert_eq!(
        m[3],
        json!({"role": "tool", "tool_call_id": "t1", "content": "data"})
    );
    assert_eq!(b["tools"][0]["function"]["name"], "read_file");
}

#[tokio::test]
async fn openrouter_streams_text_and_tool_calls_in_the_shared_shape() {
    let body = [
        r#"{"choices":[{"delta":{"content":"Let me "}}]}"#,
        r#"{"choices":[{"delta":{"content":"look."}}]}"#,
        r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"read_file","arguments":"{\"pa"}}]}}]}"#,
        r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"th\":\"/a\"}"}}]},"finish_reason":"tool_calls"}]}"#,
        r#"{"choices":[],"usage":{"prompt_tokens":11,"completion_tokens":7}}"#,
        "[DONE]",
    ]
    .iter()
    .map(|e| format!(": OPENROUTER PROCESSING\n\ndata: {e}\n\n"))
    .collect::<String>();
    let (url, seen) = mock(vec![ok("text/event-stream", &body)]).await;
    let p = OpenRouter::with_base_url(&url, Arc::new(|| Some("or-test-key".into())));
    let out: Vec<_> = collect(p.stream(req("anthropic/claude-opus-5.5")))
        .await
        .into_iter()
        .map(|d| d.unwrap())
        .collect();
    assert_eq!(out[0], ChatDelta::Text("Let me ".into()));
    assert!(out.contains(&ChatDelta::ToolCall {
        id: "c1".into(),
        name: "read_file".into(),
        input: json!({"path": "/a"})
    }));
    assert!(out.contains(&ChatDelta::Usage {
        input_tokens: 11,
        output_tokens: 7
    }));
    assert_eq!(out.last(), Some(&ChatDelta::Stop(StopReason::ToolUse)));
    let raw = seen.lock().unwrap()[0].to_lowercase();
    assert!(raw.contains("authorization: bearer or-test-key"));
    assert!(raw.starts_with("post /v1/chat/completions"));
}

#[tokio::test]
async fn openrouter_validate_reports_the_balance() {
    let (url, _) = mock(vec![ok("application/json", r#"{"data":{"label":"x","usage":1.5,"limit":10,"limit_remaining":8.5,"is_free_tier":false}}"#)]).await;
    let a = OpenRouter::with_base_url(&url, Arc::new(|| Some("k".into())))
        .validate()
        .await
        .unwrap();
    assert_eq!(a.balance.as_deref(), Some("$8.50 left"));
}

#[tokio::test]
async fn relay_registers_then_streams_and_stores_the_token() {
    let sse = "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hi\"}}\n\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\ndata: {\"type\":\"message_stop\"}\n\n";
    let (url, seen) = mock(vec![
        ok("application/json", r#"{"token":"jwt-1","expires_in":3600}"#),
        ok("text/event-stream", sse),
    ])
    .await;
    let stored = Arc::new(Mutex::new(None::<String>));
    let s = stored.clone();
    let p = Relay::new(
        &url,
        "device-123",
        Arc::new(|| None),
        Arc::new(move |t| *s.lock().unwrap() = t.map(String::from)),
    );
    let out = collect(p.stream(req("claude-opus-5-5"))).await;
    assert_eq!(out[0], Ok(ChatDelta::Text("Hi".into())));
    assert_eq!(stored.lock().unwrap().as_deref(), Some("jwt-1"));
    let seen = seen.lock().unwrap();
    assert!(seen[0].contains("device-123"));
    assert!(
        seen[1]
            .to_lowercase()
            .contains("authorization: bearer jwt-1")
    );
    assert!(
        !seen[1].to_lowercase().contains("x-api-key"),
        "no Anthropic key ever leaves the app on this route"
    );
}

#[tokio::test]
async fn relay_quota_exhausted_is_a_machine_readable_error() {
    let (url, _) = mock(vec![status(429, r#"{"type":"error","error":{"type":"quota_exhausted","code":"quota_exhausted","message":"Free trial used up for today"}}"#)]).await;
    let p = Relay::new(&url, "d", Arc::new(|| Some("jwt".into())), Arc::new(|_| {}));
    let out = collect(p.stream(req("m"))).await;
    assert_eq!(
        out,
        vec![Err(ProviderError::QuotaExhausted {
            code: "quota_exhausted".into()
        })]
    );
}

#[tokio::test]
async fn relay_unreachable_says_so() {
    let p = Relay::new(
        "http://127.0.0.1:9",
        "d",
        Arc::new(|| None),
        Arc::new(|_| {}),
    );
    let out = collect(p.stream(req("m"))).await;
    assert!(matches!(&out[0], Err(ProviderError::Network(m)) if m.contains("trial service")));
}

#[tokio::test]
async fn a_local_server_gets_the_chosen_model_and_no_key() {
    use markon_provider::local::LocalServer;
    let body = [
        r#"{"choices":[{"delta":{"content":"Hi"},"finish_reason":"stop"}]}"#,
        "[DONE]",
    ]
    .iter()
    .map(|e| format!("data: {e}\n\n"))
    .collect::<String>();
    let (url, seen) = mock(vec![ok("text/event-stream", &body)]).await;
    let p = LocalServer::new(&format!("{url}/v1"), "qwen3.5:4b");
    let out: Vec<_> = collect(p.stream(req("ignored")))
        .await
        .into_iter()
        .map(|d| d.unwrap())
        .collect();
    assert_eq!(out[0], ChatDelta::Text("Hi".into()));
    let raw = seen.lock().unwrap()[0].clone();
    assert!(raw.starts_with("POST /v1/chat/completions"));
    assert!(raw.contains("\"model\":\"qwen3.5:4b\""));
    assert!(
        !raw.to_lowercase().contains("authorization"),
        "nothing to authorise on this Mac"
    );
    assert_eq!(p.route(), "local");
}

#[tokio::test]
async fn a_local_server_without_the_model_says_how_to_get_it() {
    use markon_provider::local::LocalServer;
    let (url, _) = mock(vec![ok(
        "application/json",
        r#"{"data":[{"id":"llama3.2:3b"}]}"#,
    )])
    .await;
    let err = LocalServer::new(&url, "qwen3.5:4b")
        .validate()
        .await
        .unwrap_err();
    assert!(err.to_string().contains("ollama pull qwen3.5:4b"), "{err}");
    let (url, _) = mock(vec![ok(
        "application/json",
        r#"{"data":[{"id":"qwen3.5:4b"}]}"#,
    )])
    .await;
    assert!(
        LocalServer::new(&url, "qwen3.5:4b")
            .validate()
            .await
            .is_ok()
    );
}
