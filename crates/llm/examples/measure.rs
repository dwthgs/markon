//! Measures load time, time to the first word, and how often the
//! built-in model calls the right tool. Run by hand, never in the test suite:
//! `MARKON_LLM_MODEL=/path/model.gguf cargo run --release -p markon-llm --example measure`
use markon_llm::BuiltIn;
use markon_provider::*;
use serde_json::json;
use std::time::Instant;

fn tool(name: &str, description: &str, props: serde_json::Value) -> ToolSpec {
    ToolSpec {
        name: name.into(),
        description: description.into(),
        input_schema: json!({"type": "object", "properties": props}),
    }
}

#[tokio::main]
async fn main() {
    let path = std::env::var("MARKON_LLM_MODEL").expect("set MARKON_LLM_MODEL");
    let t = Instant::now();
    let m = BuiltIn::load(std::path::Path::new(&path)).expect("load");
    println!("load_ms {}", t.elapsed().as_millis());
    let tools = vec![
        tool(
            "fetch_page",
            "Fetch a web page and return its text.",
            json!({"url": {"type": "string"}}),
        ),
        tool(
            "what_did_i_miss",
            "What happened in the user's messages, mail and calendar.",
            json!({"hours": {"type": "number"}}),
        ),
        tool(
            "remind_me",
            "Set a reminder at a time.",
            json!({"text": {"type": "string"}, "when": {"type": "string"}}),
        ),
        tool(
            "spotify",
            "Control Spotify: play, pause, next.",
            json!({"action": {"type": "string"}}),
        ),
    ];
    let cases: &[(&str, Option<&str>)] = &[
        ("What did I miss today?", Some("what_did_i_miss")),
        ("Remind me to call Sara at 3pm.", Some("remind_me")),
        ("Pause the music.", Some("spotify")),
        ("Read https://example.com for me.", Some("fetch_page")),
        ("Catch me up on my messages.", Some("what_did_i_miss")),
        (
            "Set a reminder in 20 minutes to stretch.",
            Some("remind_me"),
        ),
        ("Skip this song.", Some("spotify")),
        (
            "What's on https://news.ycombinator.com right now?",
            Some("fetch_page"),
        ),
        ("What is 12 times 7?", None),
        ("Say good morning in Amharic.", None),
        ("What's the capital of Kenya?", None),
        ("Tell me a one-line joke.", None),
    ];
    let (mut right, mut first) = (0, Vec::new());
    for (prompt, want) in cases {
        let req = ChatRequest {
            model: markon_llm::MODEL_ID.into(),
            system: "You are Markon, a voice assistant. Answer in one short spoken sentence. Use a tool when one fits.".into(),
            messages: vec![Message::user_text(prompt)],
            tools: tools.clone(),
            max_tokens: 200,
        };
        let t = Instant::now();
        let mut rx = m.stream(req);
        let (mut called, mut ttf) = (None, None);
        while let Some(Ok(d)) = rx.recv().await {
            match d {
                ChatDelta::Text(_) if ttf.is_none() => ttf = Some(t.elapsed().as_millis()),
                ChatDelta::ToolCall { name, .. } => called = Some(name),
                _ => {}
            }
        }
        let ok = called.as_deref() == *want;
        right += usize::from(ok);
        if let Some(ms) = ttf {
            first.push(ms);
        }
        println!(
            "{ok:5} want={want:?} got={called:?} first_text_ms={ttf:?} total_ms={} :: {prompt}",
            t.elapsed().as_millis()
        );
    }
    first.sort_unstable();
    println!("tool_choice {right}/{}", cases.len());
    if !first.is_empty() {
        println!(
            "first_text_ms median {} max {}",
            first[first.len() / 2],
            first.last().unwrap()
        );
    }
}
