//! Acceptance tests, all against the fake provider.
use markon_agent::*;
use markon_provider::fake::FakeProvider;
use markon_provider::*;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedReceiver;

struct Tools;
impl ToolRuntime for Tools {
    fn specs(&self) -> Vec<ToolSpec> {
        ["slow", "fast", "boom", "big"]
            .iter()
            .map(|n| ToolSpec {
                name: (*n).into(),
                description: String::new(),
                input_schema: json!({"type": "object"}),
            })
            .collect()
    }
    fn call(&self, call: ToolCall) -> BoxFuture<'static, Result<String, String>> {
        Box::pin(async move {
            match call.name.as_str() {
                "slow" => {
                    tokio::time::sleep(Duration::from_millis(80)).await;
                    Ok("slow done".into())
                }
                "fast" => Ok("fast done".into()),
                "big" => Ok("x".repeat(100_000)),
                _ => Err("it broke".into()),
            }
        })
    }
}

fn agent(
    scripts: Vec<Vec<Result<ChatDelta, ProviderError>>>,
) -> (Agent, Arc<FakeProvider>, UnboundedReceiver<AgentEvent>) {
    let fake = Arc::new(FakeProvider::new(scripts));
    let (a, rx) = Agent::new(
        fake.clone(),
        Arc::new(Tools),
        AgentConfig {
            retry_base: Duration::from_millis(1),
            ..AgentConfig::default()
        },
    );
    (a, fake, rx)
}

async fn drain(rx: &mut UnboundedReceiver<AgentEvent>) -> Vec<AgentEvent> {
    let mut v = Vec::new();
    while let Ok(Some(e)) = tokio::time::timeout(Duration::from_millis(300), rx.recv()).await {
        v.push(e);
    }
    v
}

fn states(ev: &[AgentEvent]) -> Vec<AgentState> {
    ev.iter()
        .filter_map(|e| {
            if let AgentEvent::StateChanged(s) = e {
                Some(*s)
            } else {
                None
            }
        })
        .collect()
}

fn finished(ev: &[AgentEvent]) -> &TurnSummary {
    ev.iter()
        .find_map(|e| {
            if let AgentEvent::TurnFinished(t) = e {
                Some(t)
            } else {
                None
            }
        })
        .expect("turn finished")
}

#[tokio::test]
async fn states_are_emitted_once_and_in_order() {
    let (a, _, mut rx) = agent(vec![FakeProvider::text("Hello there.")]);
    a.set_state(AgentState::Listening);
    a.set_state(AgentState::Listening); // duplicate: not emitted again
    a.submit(UserTurn::voice("hi")).await;
    a.set_state(AgentState::Speaking);
    a.set_state(AgentState::Idle);
    let ev = drain(&mut rx).await;
    assert_eq!(
        states(&ev),
        vec![
            AgentState::Listening,
            AgentState::Thinking,
            AgentState::Speaking,
            AgentState::Idle
        ]
    );
}

#[tokio::test]
async fn speaking_is_refused_before_thinking() {
    let (a, _, mut rx) = agent(vec![]);
    a.set_state(AgentState::Listening);
    assert!(!a.set_state(AgentState::Speaking));
    let ev = drain(&mut rx).await;
    assert_eq!(states(&ev), vec![AgentState::Listening]);
}

#[tokio::test]
async fn parallel_tool_calls_return_results_in_call_order() {
    let (a, fake, mut rx) = agent(vec![
        FakeProvider::tools(&[("t1", "slow", json!({})), ("t2", "fast", json!({}))]),
        FakeProvider::text("Both done."),
    ]);
    let t = std::time::Instant::now();
    a.submit(UserTurn::text("go")).await;
    let ev = drain(&mut rx).await;
    assert!(
        t.elapsed() < Duration::from_millis(400),
        "tools ran in parallel"
    );
    let second = &fake.requests.lock().unwrap()[1];
    let results = &second.messages.last().unwrap().content;
    assert_eq!(results[0]["tool_use_id"], "t1");
    assert_eq!(results[1]["tool_use_id"], "t2");
    assert_eq!(results[0]["content"], "slow done");
    assert_eq!(finished(&ev).text, "Both done.");
}

#[tokio::test]
async fn a_failing_or_unknown_tool_is_a_model_visible_error_and_the_loop_continues() {
    let (a, fake, mut rx) = agent(vec![
        FakeProvider::tools(&[("t1", "boom", json!({})), ("t2", "nope", json!({}))]),
        FakeProvider::text("Recovered."),
    ]);
    a.submit(UserTurn::text("go")).await;
    let ev = drain(&mut rx).await;
    let r = fake.requests.lock().unwrap()[1]
        .messages
        .last()
        .unwrap()
        .content
        .clone();
    assert_eq!(r[0]["is_error"], true);
    assert_eq!(r[0]["content"], "it broke");
    assert_eq!(r[1]["is_error"], true);
    let listed = r[1]["content"].as_str().unwrap();
    assert!(
        listed.contains("slow") && listed.contains("fast") && listed.contains("boom"),
        "{listed}"
    );
    assert_eq!(finished(&ev).text, "Recovered.");
}

#[tokio::test]
async fn tool_output_is_capped_at_32_kb_for_the_model() {
    let (a, fake, _rx) = agent(vec![
        FakeProvider::tools(&[("t1", "big", json!({}))]),
        FakeProvider::text("ok"),
    ]);
    a.submit(UserTurn::text("go")).await;
    let r = fake.requests.lock().unwrap()[1]
        .messages
        .last()
        .unwrap()
        .content
        .clone();
    assert!(r[0]["content"].as_str().unwrap().len() <= 32 * 1024);
}

#[tokio::test]
async fn stops_after_12_tool_rounds_and_asks_the_model_to_wrap_up() {
    let mut scripts: Vec<_> = (0..12)
        .map(|i| FakeProvider::tools(&[(&format!("t{i}"), "fast", json!({}))]))
        .collect();
    scripts.push(FakeProvider::tools(&[("t12", "fast", json!({}))]));
    scripts.push(FakeProvider::text("Wrapping up."));
    let (a, fake, mut rx) = agent(scripts);
    a.submit(UserTurn::text("loop")).await;
    let ev = drain(&mut rx).await;
    let reqs = fake.requests.lock().unwrap();
    let ran = ev
        .iter()
        .filter(|e| matches!(e, AgentEvent::ToolCallFinished { .. }))
        .count();
    assert_eq!(ran, 12);
    let last = reqs
        .last()
        .unwrap()
        .messages
        .last()
        .unwrap()
        .content
        .clone();
    assert!(
        last.iter()
            .all(|b| b["is_error"] == true || b["type"] == "text"),
        "unrun calls are refused"
    );
    assert!(
        last.iter()
            .any(|b| b["text"].as_str().is_some_and(|t| t.contains("wrap up")))
    );
    assert_eq!(finished(&ev).text, "Wrapping up.");
}

#[tokio::test]
async fn retries_429_and_529_three_times_then_apologises() {
    let (a, fake, mut rx) = agent(vec![
        vec![Err(ProviderError::Overloaded)],
        vec![Err(ProviderError::RateLimited {
            retry_after_s: None,
        })],
        FakeProvider::text("Third time lucky."),
    ]);
    a.submit(UserTurn::text("hi")).await;
    assert_eq!(finished(&drain(&mut rx).await).text, "Third time lucky.");
    assert_eq!(fake.requests.lock().unwrap().len(), 3);

    let (a, _, mut rx) = agent(vec![vec![Err(ProviderError::Overloaded)]; 3]);
    a.submit(UserTurn::text("hi")).await;
    let ev = drain(&mut rx).await;
    let t = finished(&ev);
    assert!(t.text.to_lowercase().contains("sorry"), "{}", t.text);
    assert!(ev.iter().any(|e| matches!(e, AgentEvent::Error(_))));
}

#[tokio::test]
async fn a_dropped_stream_keeps_the_partial_answer_and_marks_it() {
    let (a, _, mut rx) = agent(vec![vec![
        Ok(ChatDelta::Text("The answer is ".into())),
        Err(ProviderError::Network("reset".into())),
    ]]);
    a.submit(UserTurn::text("hi")).await;
    let ev = drain(&mut rx).await;
    let t = finished(&ev);
    assert_eq!(t.text, "The answer is ");
    assert!(t.partial);
}

#[tokio::test]
async fn a_cancelled_turn_leaves_no_history_and_drops_its_cards() {
    let mut fake = FakeProvider::new(vec![
        FakeProvider::tools(&[("t1", "slow", json!({}))]),
        FakeProvider::text("late"),
    ]);
    fake.delay = Duration::from_millis(30);
    let fake = Arc::new(fake);
    let (a, mut rx) = Agent::new(fake, Arc::new(Tools), AgentConfig::default());
    let a = Arc::new(a);
    let a2 = a.clone();
    let h = tokio::spawn(async move { a2.submit(UserTurn::text("go")).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    a.cancel();
    h.await.unwrap();
    let ev = drain(&mut rx).await;
    assert!(
        ev.iter()
            .any(|e| matches!(e, AgentEvent::TurnCancelled { .. }))
    );
    assert!(!ev.iter().any(|e| matches!(e, AgentEvent::TurnFinished(_))));
    assert!(a.history().is_empty(), "no half-written history");
    assert_eq!(states(&ev).last(), Some(&AgentState::Idle));
}

#[tokio::test]
async fn prompt_sections_compose_the_system_prompt() {
    let (mut a, fake, _rx) = agent(vec![FakeProvider::text("ok")]);
    a.add_section(Arc::new(StaticSection("You are Markon.".into())));
    a.add_section(Arc::new(StaticSection("Be brief.".into())));
    a.submit(UserTurn::text("hi")).await;
    assert_eq!(
        fake.requests.lock().unwrap()[0].system,
        "You are Markon.\n\nBe brief."
    );
}

#[tokio::test]
async fn usage_is_reported_per_turn() {
    let (a, _, mut rx) = agent(vec![FakeProvider::text("ok")]);
    a.submit(UserTurn::text("hi")).await;
    let t = drain(&mut rx).await;
    assert_eq!(finished(&t).usage, (10, 5));
}

#[test]
fn events_serialise_for_the_ui() {
    let s = serde_json::to_value(AgentEvent::StateChanged(AgentState::Listening)).unwrap();
    assert_eq!(s, json!({"type": "state_changed", "data": "listening"}));
    let e = serde_json::to_value(AgentEvent::Error("x".into())).unwrap();
    assert_eq!(e, json!({"type": "error", "data": "x"}));
}
