//! Acceptance tests for AnthropicDirect against a local mock server (no real API calls).
use markon_provider::anthropic::AnthropicDirect;
use markon_provider::*;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const KEY: &str = "sk-test-not-a-real-key";

/// Serves one canned HTTP response per connection and records each raw request.
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

fn sse(events: &[&str]) -> String {
    let body: String = events.iter().map(|e| format!("data: {e}\n\n")).collect();
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    )
}

fn status(code: u16, body: &str) -> String {
    format!(
        "HTTP/1.1 {code} X\r\ncontent-type: application/json\r\nretry-after: 2\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    )
}

fn key() -> KeySource {
    Arc::new(|| Some(KEY.to_string()))
}

async fn collect(mut rx: DeltaStream) -> Vec<Result<ChatDelta, ProviderError>> {
    let mut v = Vec::new();
    while let Some(d) = rx.recv().await {
        v.push(d);
    }
    v
}

fn req() -> ChatRequest {
    ChatRequest {
        model: "claude-opus-5-5".into(),
        system: "persona".into(),
        messages: vec![Message::user_text("hi")],
        tools: vec![],
        max_tokens: 1024,
    }
}

#[tokio::test]
async fn streams_text_tool_calls_usage_and_stop_in_one_shape() {
    let (url, seen) = mock(vec![sse(&[
        r#"{"type":"message_start","message":{"usage":{"input_tokens":42}}}"#,
        r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig"}}"#,
        r#"{"type":"content_block_stop","index":0}"#,
        r#"{"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}"#,
        r#"{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Opening "}}"#,
        r#"{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"it."}}"#,
        r#"{"type":"content_block_stop","index":1}"#,
        r#"{"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"tu_1","name":"open_url","input":{}}}"#,
        r#"{"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"url\":\"https://exa"}}"#,
        r#"{"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"mple.com\"}"}}"#,
        r#"{"type":"content_block_stop","index":2}"#,
        r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":17}}"#,
        r#"{"type":"message_stop"}"#,
    ])])
    .await;
    let p = AnthropicDirect::with_base_url(&url, key());
    let out: Vec<_> = collect(p.stream(req()))
        .await
        .into_iter()
        .map(|d| d.unwrap())
        .collect();
    assert!(out.contains(&ChatDelta::Text("Opening ".into())));
    assert!(out.contains(&ChatDelta::Block(
        serde_json::json!({"type":"thinking","thinking":"","signature":"sig"})
    )));
    assert!(out.contains(&ChatDelta::ToolCall {
        id: "tu_1".into(),
        name: "open_url".into(),
        input: serde_json::json!({"url":"https://example.com"})
    }));
    assert_eq!(
        out[out.len() - 2],
        ChatDelta::Usage {
            input_tokens: 42,
            output_tokens: 17
        }
    );
    assert_eq!(out[out.len() - 1], ChatDelta::Stop(StopReason::ToolUse));

    let raw = seen.lock().unwrap()[0].to_lowercase();
    assert!(raw.contains("anthropic-version: 2026-10-21"));
    assert!(raw.contains(&format!("x-api-key: {KEY}")));
    assert!(raw.contains("\"stream\":true"));
}

#[tokio::test]
async fn maps_429_and_529_to_retryable_errors() {
    let (url, _) = mock(vec![
        status(
            429,
            r#"{"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#,
        ),
        status(
            529,
            r#"{"type":"error","error":{"type":"overloaded_error","message":"busy"}}"#,
        ),
        status(
            401,
            r#"{"type":"error","error":{"type":"authentication_error","message":"bad key"}}"#,
        ),
    ])
    .await;
    let p = AnthropicDirect::with_base_url(&url, key());
    let e1 = collect(p.stream(req())).await.pop().unwrap().unwrap_err();
    assert_eq!(
        e1,
        ProviderError::RateLimited {
            retry_after_s: Some(2)
        }
    );
    assert!(e1.retryable());
    let e2 = collect(p.stream(req())).await.pop().unwrap().unwrap_err();
    assert_eq!(e2, ProviderError::Overloaded);
    let e3 = collect(p.stream(req())).await.pop().unwrap().unwrap_err();
    assert_eq!(e3, ProviderError::Auth);
    assert!(!e3.retryable());
}

#[tokio::test]
async fn a_dropped_stream_keeps_the_partial_text_and_reports_it() {
    let body = "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Half\"}}\n\n";
    let (url, _) = mock(vec![format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n{body}"
    )])
    .await;
    let p = AnthropicDirect::with_base_url(&url, key());
    let out = collect(p.stream(req())).await;
    assert_eq!(out[0], Ok(ChatDelta::Text("Half".into())));
    assert!(matches!(out.last(), Some(Err(ProviderError::Network(_)))));
}

#[tokio::test]
async fn validate_confirms_a_key_in_under_3_s_and_errors_never_contain_it() {
    let (url, _) = mock(vec![
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 11\r\n\r\n{\"data\":[]}".into(),
        status(401, r#"{"error":{"type":"authentication_error","message":"invalid x-api-key"}}"#),
    ])
    .await;
    let p = AnthropicDirect::with_base_url(&url, key());
    let t = std::time::Instant::now();
    assert_eq!(p.validate().await.unwrap().route, "anthropic");
    assert!(t.elapsed().as_secs_f32() < 3.0);
    let err = p.validate().await.unwrap_err();
    assert!(!format!("{err:?} {err}").contains(KEY));
}

#[tokio::test]
async fn no_key_fails_before_any_request() {
    let p = AnthropicDirect::with_base_url("http://127.0.0.1:9", Arc::new(|| None));
    assert_eq!(
        collect(p.stream(req())).await,
        vec![Err(ProviderError::NoKey)]
    );
}
