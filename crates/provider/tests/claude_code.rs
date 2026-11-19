//! The Claude Code route against a fake `claude` script: no network, no real model.
use markon_provider::claude_code::ClaudeCode;
use markon_provider::*;
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

/// A fake `claude` that saves its stdin and args next to itself and prints `lines`.
fn fake(name: &str, lines: &[serde_json::Value], exit: i32) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("markon-cc-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out: String = lines.iter().map(|l| format!("{l}\n")).collect();
    std::fs::write(dir.join("out.jsonl"), out).unwrap();
    let bin = dir.join("claude");
    std::fs::write(
        &bin,
        format!(
            "#!/bin/sh\ncat > '{d}/stdin.txt'\necho \"$@\" > '{d}/args.txt'\nenv > '{d}/env.txt'\ncat '{d}/out.jsonl'\nexit {exit}\n",
            d = dir.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    bin
}

fn delta(text: &str) -> serde_json::Value {
    json!({"type": "stream_event", "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": text}}})
}

fn result(text: &str, is_error: bool) -> serde_json::Value {
    json!({"type": "result", "is_error": is_error, "result": text, "usage": {"input_tokens": 12, "output_tokens": 7}})
}

async fn collect(p: &ClaudeCode, req: ChatRequest) -> Vec<Result<ChatDelta, ProviderError>> {
    let mut rx = p.stream(req);
    let mut out = Vec::new();
    while let Some(d) = rx.recv().await {
        out.push(d);
    }
    out
}

fn req() -> ChatRequest {
    ChatRequest {
        model: String::new(),
        system: "You are Markon.".into(),
        messages: vec![Message::user_text("what's the weather")],
        tools: vec![ToolSpec {
            name: "fetch_page".into(),
            description: "Fetch a page".into(),
            input_schema: json!({"type": "object"}),
        }],
        max_tokens: 1024,
    }
}

#[tokio::test]
async fn streams_text_and_hides_tool_calls_from_speech() {
    let bin = fake(
        "tools",
        &[
            json!({"type": "system", "subtype": "init"}),
            delta("Let me check. <tool"),
            delta(
                "_call>\n{\"name\": \"fetch_page\", \"arguments\": {\"url\": \"https://wttr.in\"}}\n</tool_call>",
            ),
            delta(
                "\n<tool_call>{\"name\": \"fetch_page\", \"arguments\": {\"url\": \"https://wttr.in\"}}</tool_call>",
            ),
            result("ignored", false),
        ],
        0,
    );
    let p = ClaudeCode::new(bin.clone());
    let out = collect(&p, req()).await;
    let spoken: String = out
        .iter()
        .filter_map(|d| match d {
            Ok(ChatDelta::Text(t)) => Some(t.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(spoken, "Let me check. ");
    let calls = out
        .iter()
        .filter(|d| matches!(d, Ok(ChatDelta::ToolCall { name, input, .. }) if name == "fetch_page" && input["url"] == "https://wttr.in"))
        .count();
    assert_eq!(calls, 1, "a call written twice runs once");
    assert!(out.contains(&Ok(ChatDelta::Usage {
        input_tokens: 12,
        output_tokens: 7
    })));
    assert_eq!(out.last(), Some(&Ok(ChatDelta::Stop(StopReason::ToolUse))));

    let dir = bin.parent().unwrap();
    let stdin = std::fs::read_to_string(dir.join("stdin.txt")).unwrap();
    assert!(stdin.contains("what's the weather"));
    let args = std::fs::read_to_string(dir.join("args.txt")).unwrap();
    assert!(
        args.contains("--tools  ") || args.contains("--tools \n"),
        "built-in tools are off: {args}"
    );
    assert!(
        args.contains("fetch_page"),
        "tools are described in the system prompt"
    );
    assert!(args.contains("--model sonnet"));
    let env = std::fs::read_to_string(dir.join("env.txt")).unwrap();
    assert!(
        !env.contains("ANTHROPIC_API_KEY"),
        "the subscription login is used, never a key"
    );
}

#[tokio::test]
async fn plain_reply_ends_the_turn() {
    let bin = fake(
        "plain",
        &[
            delta("Hello "),
            delta("there."),
            result("Hello there.", false),
        ],
        0,
    );
    let out = collect(&ClaudeCode::new(bin), req()).await;
    assert!(out.contains(&Ok(ChatDelta::Block(
        json!({"type": "text", "text": "Hello there."})
    ))));
    assert_eq!(out.last(), Some(&Ok(ChatDelta::Stop(StopReason::EndTurn))));
}

#[tokio::test]
async fn not_logged_in_is_an_auth_error() {
    let bin = fake(
        "auth",
        &[result("Not logged in · Please run /login", true)],
        1,
    );
    let out = collect(&ClaudeCode::new(bin), req()).await;
    assert_eq!(out.last(), Some(&Err(ProviderError::Auth)));
}

#[tokio::test]
async fn a_crash_is_reported() {
    let bin = fake("crash", &[], 3);
    let out = collect(&ClaudeCode::new(bin), req()).await;
    assert!(matches!(out.last(), Some(Err(ProviderError::Network(_)))));
}

/// Runs the real Claude Code on this Mac (your Claude plan, no key):
/// `cargo test -p markon-provider --test claude_code -- --ignored`.
#[tokio::test]
#[ignore]
async fn the_real_claude_code_answers_and_calls_a_tool() {
    let p = ClaudeCode::new(markon_provider::claude_code::find().expect("claude installed"));
    let acct = p.validate().await.expect("signed in");
    eprintln!("account: {:?}", acct.plan);
    let mut r = req();
    r.messages[0] =
        Message::user_text("Fetch https://wttr.in/London?format=3 and tell me the weather.");
    let t0 = std::time::Instant::now();
    let mut rx = p.stream(r);
    let mut first = None;
    let mut got = Vec::new();
    while let Some(d) = rx.recv().await {
        if first.is_none() {
            first = Some(t0.elapsed());
        }
        got.push(d);
    }
    eprintln!("first delta {:?}, done {:?}: {got:?}", first, t0.elapsed());
    assert!(
        got.iter()
            .any(|d| matches!(d, Ok(ChatDelta::ToolCall { name, .. }) if name == "fetch_page"))
    );
    assert_eq!(got.last(), Some(&Ok(ChatDelta::Stop(StopReason::ToolUse))));
}
