//! A look at the screen reaches a model that can see pictures as an
//! image, a text-only model is told it cannot see it, and the picture never stays in the history.
use markon_agent::*;
use markon_provider::fake::FakeProvider;
use markon_provider::*;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

struct Screen(String);
impl ToolRuntime for Screen {
    fn specs(&self) -> Vec<ToolSpec> {
        vec![ToolSpec {
            name: SCREEN_TOOL.into(),
            description: String::new(),
            input_schema: json!({"type": "object"}),
        }]
    }
    fn call(&self, _: ToolCall) -> BoxFuture<'static, Result<String, String>> {
        let r = self.0.clone();
        Box::pin(async move { Ok(r) })
    }
}

/// The fake, presented as another route.
struct As(&'static str, Arc<FakeProvider>);
impl ModelProvider for As {
    fn route(&self) -> &'static str {
        self.0
    }
    fn stream(&self, req: ChatRequest) -> DeltaStream {
        self.1.stream(req)
    }
    fn models(&self) -> Vec<ModelInfo> {
        self.1.models()
    }
    fn default_model(&self) -> String {
        self.1.default_model()
    }
    fn validate(&self) -> BoxFuture<'_, Result<Account, ProviderError>> {
        self.1.validate()
    }
}

fn shot() -> String {
    let p = std::env::temp_dir().join(format!("markon-agent-shot-{}.jpg", std::process::id()));
    std::fs::write(&p, [0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3]).unwrap();
    format!(
        "<screenshot path=\"{}\"/>\nThis is the user's screen. The app in front is Keynote.",
        p.display()
    )
}

async fn run(route: &'static str) -> (Arc<FakeProvider>, Agent) {
    let fake = Arc::new(FakeProvider::new(vec![
        FakeProvider::tools(&[("1", SCREEN_TOOL, json!({}))]),
        FakeProvider::text("A slide about Lalibela."),
    ]));
    let (agent, mut rx) = Agent::new(
        Arc::new(As(route, fake.clone())),
        Arc::new(Screen(shot())),
        AgentConfig::default(),
    );
    agent.submit(UserTurn::voice("What's on my screen?")).await;
    while tokio::time::timeout(Duration::from_millis(100), rx.recv())
        .await
        .is_ok_and(|e| e.is_some())
    {}
    (fake, agent)
}

fn tool_result(fake: &FakeProvider) -> Value {
    let reqs = fake.requests.lock().unwrap();
    reqs[1]
        .messages
        .iter()
        .flat_map(|m| m.content.iter())
        .find(|b| b["type"] == "tool_result")
        .cloned()
        .expect("a tool result")
}

#[tokio::test]
async fn a_model_that_sees_pictures_gets_the_screen_as_an_image() {
    let (fake, agent) = run("anthropic").await;
    let r = tool_result(&fake);
    let parts = r["content"].as_array().expect("text and image");
    assert_eq!(parts[0]["type"], "text");
    assert!(parts[0]["text"].as_str().unwrap().contains("Keynote"));
    assert!(!parts[0]["text"].as_str().unwrap().contains("<screenshot"));
    assert_eq!(parts[1]["type"], "image");
    assert_eq!(parts[1]["source"]["media_type"], "image/jpeg");
    assert_eq!(parts[1]["source"]["data"], "/9j/4AECAw==");
    let kept = serde_json::to_string(
        &agent
            .history()
            .iter()
            .map(|m| &m.content)
            .collect::<Vec<_>>(),
    )
    .unwrap();
    assert!(
        !kept.contains("\"image\""),
        "pictures never stay in the history: {kept}"
    );
}

#[tokio::test]
async fn a_text_only_model_is_told_it_cannot_see_the_screen() {
    let (fake, _) = run("claudecode").await;
    let r = tool_result(&fake);
    let text = r["content"].as_str().expect("plain text");
    assert!(!text.contains("<screenshot"), "{text}");
    assert!(
        text.contains("cannot see pictures") && text.contains("Keynote"),
        "{text}"
    );
}

#[test]
fn only_a_screenshot_result_can_attach_a_file() {
    assert!(screenshot_path("<screenshot path=\"/tmp/a.jpg\"/>").is_some());
    assert!(screenshot_path("text <screenshot path=\"/tmp/a.jpg\"/>").is_none());
    assert!(screenshot_path("<screenshot path=\"/Users/x/.ssh/id_rsa\"/>").is_none());
    assert!(screenshot_path("<screenshot path=\"a.jpg\"/>").is_none());
}
