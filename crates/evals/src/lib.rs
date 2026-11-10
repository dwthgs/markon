//! Evals: saved scenarios replayed through the real
//! agent loop with stand-in tools, each scored on whether the right tool was called, the right words
//! were said, and how long it took. Run by hand only (`cargo run -p markon-evals`): tests never call
//! real models, so the suite only checks the scoring.
use markon_agent::{
    Agent, AgentConfig, AgentEvent, StaticSection, ToolCall, ToolRuntime, UserTurn,
};
use markon_provider::{BoxFuture, ModelProvider, ToolSpec};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Scenario {
    pub name: String,
    pub say: String,
    /// The tool that should be called first, or none.
    #[serde(default)]
    pub tool: Option<String>,
    /// Words the spoken answer must contain (any case).
    #[serde(default)]
    pub contains: Vec<String>,
    /// Words it must not contain.
    #[serde(default)]
    pub never: Vec<String>,
    /// What the stand-in tool returns.
    #[serde(default)]
    pub tool_result: String,
    #[serde(default = "default_limit")]
    pub limit_ms: u64,
}
fn default_limit() -> u64 {
    8000
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Score {
    pub name: String,
    pub passed: bool,
    pub tools: Vec<String>,
    pub answer: String,
    pub first_text_ms: Option<u64>,
    pub total_ms: u64,
    pub problems: Vec<String>,
}

pub const SCENARIOS: &str = include_str!("../scenarios.json");

/// Stand-in tools: the scenario's tool returns its canned result, every other tool is offered too.
struct StandIn {
    specs: Vec<ToolSpec>,
    result: String,
    called: Arc<Mutex<Vec<String>>>,
}
impl ToolRuntime for StandIn {
    fn specs(&self) -> Vec<ToolSpec> {
        self.specs.clone()
    }
    fn call(&self, c: ToolCall) -> BoxFuture<'static, Result<String, String>> {
        self.called.lock().expect("called").push(c.name);
        let r = self.result.clone();
        Box::pin(async move { Ok(r) })
    }
}

pub fn standard_tools() -> Vec<ToolSpec> {
    let t = |n: &str, d: &str, props: Value| ToolSpec {
        name: n.into(),
        description: d.into(),
        input_schema: json!({"type": "object", "properties": props}),
    };
    vec![
        t(
            "what_did_i_miss",
            "What happened in the user's messages, mail and calendar.",
            json!({"hours": {"type": "number"}}),
        ),
        t(
            "remind_me",
            "Set a reminder at a time.",
            json!({"text": {"type": "string"}, "when": {"type": "string"}}),
        ),
        t(
            "search_my_stuff",
            "Search the user's documents.",
            json!({"query": {"type": "string"}}),
        ),
        t(
            "send_imessage",
            "Send an iMessage; the user is asked first.",
            json!({"to": {"type": "string"}, "text": {"type": "string"}}),
        ),
        t(
            "fetch_page",
            "Fetch a web page.",
            json!({"url": {"type": "string"}}),
        ),
        t(
            "spotify",
            "Control Spotify.",
            json!({"action": {"type": "string"}}),
        ),
    ]
}

/// Score one finished run.
pub fn score(
    s: &Scenario,
    tools: &[String],
    answer: &str,
    first_text_ms: Option<u64>,
    total_ms: u64,
) -> Score {
    let mut problems = Vec::new();
    match (&s.tool, tools.first()) {
        (Some(want), Some(got)) if want != got => {
            problems.push(format!("called {got}, wanted {want}"))
        }
        (Some(want), None) => problems.push(format!("called no tool, wanted {want}")),
        (None, Some(got)) => problems.push(format!("called {got}, wanted no tool")),
        _ => {}
    }
    let lower = answer.to_lowercase();
    for w in &s.contains {
        if !lower.contains(&w.to_lowercase()) {
            problems.push(format!("did not say \"{w}\""));
        }
    }
    for w in &s.never {
        if lower.contains(&w.to_lowercase()) {
            problems.push(format!("said \"{w}\""));
        }
    }
    if total_ms > s.limit_ms {
        problems.push(format!("took {total_ms} ms, limit {} ms", s.limit_ms));
    }
    Score {
        name: s.name.clone(),
        passed: problems.is_empty(),
        tools: tools.to_vec(),
        answer: answer.into(),
        first_text_ms,
        total_ms,
        problems,
    }
}

/// Run one scenario through the agent loop with `provider`.
pub async fn run(provider: Arc<dyn ModelProvider>, s: &Scenario, persona: &str) -> Score {
    let called = Arc::new(Mutex::new(Vec::new()));
    let tools = Arc::new(StandIn {
        specs: standard_tools(),
        result: s.tool_result.clone(),
        called: called.clone(),
    });
    let (mut agent, mut rx) = Agent::new(
        provider,
        tools,
        AgentConfig {
            max_rounds: 4,
            ..AgentConfig::default()
        },
    );
    agent.add_section(Arc::new(StaticSection(persona.into())));
    let started = Instant::now();
    let agent = Arc::new(agent);
    let a = agent.clone();
    let say = s.say.clone();
    let turn = tokio::spawn(async move { a.submit(UserTurn::voice(&say)).await });
    let (mut first, mut answer) = (None, String::new());
    while let Ok(Some(e)) = tokio::time::timeout(Duration::from_secs(60), rx.recv()).await {
        match e {
            AgentEvent::TextDelta { .. } if first.is_none() => {
                first = Some(started.elapsed().as_millis() as u64)
            }
            AgentEvent::TurnFinished(t) => {
                answer = t.text;
                break;
            }
            _ => {}
        }
    }
    let _ = turn.await;
    let total = started.elapsed().as_millis() as u64;
    let tools = called.lock().expect("called").clone();
    score(s, &tools, &answer, first, total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use markon_provider::fake::FakeProvider;

    #[test]
    fn the_bundled_scenarios_parse() {
        let v: Vec<Scenario> = serde_json::from_str(SCENARIOS).unwrap();
        assert!(v.len() >= 8);
        assert!(v.iter().any(|s| s.tool.is_none()));
    }

    #[test]
    fn scoring_names_each_problem() {
        let s = Scenario {
            name: "x".into(),
            say: "catch me up".into(),
            tool: Some("what_did_i_miss".into()),
            contains: vec!["Sara".into()],
            never: vec!["As an AI".into()],
            tool_result: String::new(),
            limit_ms: 1000,
        };
        assert!(
            score(
                &s,
                &["what_did_i_miss".into()],
                "Sara wrote.",
                Some(300),
                900
            )
            .passed
        );
        let bad = score(&s, &["fetch_page".into()], "As an AI I can't.", None, 2000);
        assert_eq!(bad.problems.len(), 4, "{:?}", bad.problems);
    }

    #[tokio::test]
    async fn a_scenario_runs_through_the_real_agent_loop() {
        let fake = Arc::new(FakeProvider::new(vec![
            FakeProvider::tools(&[("1", "what_did_i_miss", json!({}))]),
            FakeProvider::text("Sara is running late."),
        ]));
        let s: Vec<Scenario> = serde_json::from_str(SCENARIOS).unwrap();
        let catch_up = s.iter().find(|s| s.name == "catch up").unwrap();
        let r = run(fake, catch_up, "You are Markon.").await;
        assert!(r.passed, "{:?}", r.problems);
        assert_eq!(r.tools, ["what_did_i_miss"]);
    }
}
