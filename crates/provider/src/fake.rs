//! A scripted provider for tests: never touches the network.
use crate::*;
use std::sync::Mutex;

/// Each call to `stream` pops the next script. A script is a list of deltas or errors.
pub struct FakeProvider {
    scripts: Mutex<Vec<Vec<Result<ChatDelta, ProviderError>>>>,
    pub requests: Mutex<Vec<ChatRequest>>,
    pub delay: std::time::Duration,
}

impl FakeProvider {
    pub fn new(mut scripts: Vec<Vec<Result<ChatDelta, ProviderError>>>) -> Self {
        scripts.reverse();
        Self {
            scripts: Mutex::new(scripts),
            requests: Mutex::new(Vec::new()),
            delay: std::time::Duration::ZERO,
        }
    }

    /// A reply that is plain text, split into word deltas.
    pub fn text(reply: &str) -> Vec<Result<ChatDelta, ProviderError>> {
        let mut v: Vec<_> = reply
            .split_inclusive(' ')
            .map(|w| Ok(ChatDelta::Text(w.into())))
            .collect();
        v.push(Ok(ChatDelta::Block(
            serde_json::json!({"type": "text", "text": reply}),
        )));
        v.push(Ok(ChatDelta::Usage {
            input_tokens: 10,
            output_tokens: 5,
        }));
        v.push(Ok(ChatDelta::Stop(StopReason::EndTurn)));
        v
    }

    /// A reply that calls tools.
    pub fn tools(calls: &[(&str, &str, Value)]) -> Vec<Result<ChatDelta, ProviderError>> {
        let mut v = Vec::new();
        for (id, name, input) in calls {
            v.push(Ok(ChatDelta::Block(
                serde_json::json!({"type": "tool_use", "id": id, "name": name, "input": input}),
            )));
            v.push(Ok(ChatDelta::ToolCall {
                id: (*id).into(),
                name: (*name).into(),
                input: input.clone(),
            }));
        }
        v.push(Ok(ChatDelta::Stop(StopReason::ToolUse)));
        v
    }
}

impl ModelProvider for FakeProvider {
    fn route(&self) -> &'static str {
        "fake"
    }
    fn stream(&self, req: ChatRequest) -> DeltaStream {
        self.requests.lock().expect("lock").push(req);
        let script = self
            .scripts
            .lock()
            .expect("lock")
            .pop()
            .unwrap_or_else(|| FakeProvider::text("(no more scripts)"));
        let delay = self.delay;
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            for d in script {
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
                if tx.send(d).await.is_err() {
                    return;
                }
            }
        });
        rx
    }
    fn models(&self) -> Vec<ModelInfo> {
        vec![ModelInfo {
            id: "fake".into(),
            name: "Fake".into(),
        }]
    }
    fn default_model(&self) -> String {
        "fake".into()
    }
    fn validate(&self) -> BoxFuture<'_, Result<Account, ProviderError>> {
        Box::pin(async {
            Ok(Account {
                route: "fake".into(),
                plan: None,
                balance: None,
            })
        })
    }
}
