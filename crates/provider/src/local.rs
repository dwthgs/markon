//! Route D: a model server the user already runs on this Mac (Ollama, LM Studio),
//! spoken to in the OpenAI chat shape. Experimental.
use crate::openrouter::to_openai;
use crate::*;
use serde_json::json;
use std::time::Duration;

/// Ollama's OpenAI-compatible endpoint. LM Studio's is `http://127.0.0.1:1234`.
pub const OLLAMA: &str = "http://127.0.0.1:11434";

/// Ollama tags that call tools, smallest first: (tag, download GB, RAM needed GB).
pub const MODELS: &[(&str, f64, f64)] = &[
    ("qwen3.5:0.8b", 1.0, 4.0),
    ("qwen3.5:2b", 2.7, 6.0),
    ("qwen3.5:4b", 3.4, 8.0),
    ("qwen3.5:9b", 6.6, 12.0),
    ("qwen3.5:27b", 17.0, 24.0),
    ("qwen3.5:35b", 24.0, 32.0),
    ("qwen3.5:122b", 81.0, 96.0),
];

/// The models that fit in `ram_gb`, smallest first (their `models_that_fit_in`).
pub fn models_that_fit_in(ram_gb: f64) -> Vec<&'static str> {
    MODELS
        .iter()
        .filter(|(_, _, need)| ram_gb >= *need)
        .map(|(t, ..)| *t)
        .collect()
}

/// The second-largest model that fits, leaving room for everything else (their `default_local_model`).
pub fn default_model_for(ram_gb: f64) -> &'static str {
    let fit = models_that_fit_in(ram_gb);
    match fit.len() {
        0 => MODELS[0].0,
        1 => fit[0],
        n => fit[n - 2],
    }
}

pub struct LocalServer {
    http: reqwest::Client,
    base_url: String,
    model: String,
}

impl LocalServer {
    pub fn new(base_url: &str, model: &str) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(3))
            // A local model can take a while to load into memory on the first request.
            .read_timeout(Duration::from_secs(180))
            .build()
            .expect("http client");
        Self {
            http,
            base_url: base_url
                .trim_end_matches('/')
                .trim_end_matches("/v1")
                .to_string(),
            model: model.to_string(),
        }
    }
}

impl ModelProvider for LocalServer {
    fn route(&self) -> &'static str {
        "local"
    }
    fn stream(&self, req: ChatRequest) -> DeltaStream {
        let (tx, rx) = mpsc::channel(64);
        let mut body = to_openai(&req);
        body["model"] = json!(self.model);
        let rb = self
            .http
            .post(format!("{}/v1/chat/completions", self.base_url))
            .json(&body);
        tokio::spawn(async move {
            if let Err(e) = crate::openrouter::run(rb, &tx).await {
                let _ = tx.send(Err(e)).await;
            }
        });
        rx
    }
    fn models(&self) -> Vec<ModelInfo> {
        vec![ModelInfo {
            id: self.model.clone(),
            name: format!("{} (on this Mac, experimental)", self.model),
        }]
    }
    fn default_model(&self) -> String {
        self.model.clone()
    }
    fn validate(&self) -> BoxFuture<'_, Result<Account, ProviderError>> {
        Box::pin(async move {
            let r = self
                .http
                .get(format!("{}/v1/models", self.base_url))
                .timeout(Duration::from_secs(3))
                .send()
                .await
                .map_err(|_| {
                    ProviderError::Network(format!("no model server at {}", self.base_url))
                })?;
            let v: Value = r
                .json()
                .await
                .map_err(|e| ProviderError::Parse(e.to_string()))?;
            let ids: Vec<&str> = v["data"]
                .as_array()
                .map(|a| a.iter().filter_map(|m| m["id"].as_str()).collect())
                .unwrap_or_default();
            if !ids
                .iter()
                .any(|i| *i == self.model || i.starts_with(&format!("{}:", self.model)))
            {
                return Err(ProviderError::ModelUnavailable(format!(
                    "{} is not on the server; run `ollama pull {}`",
                    self.model, self.model
                )));
            }
            Ok(Account {
                route: "local".into(),
                plan: Some(format!("{} models on this Mac", ids.len())),
                balance: None,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_a_model_that_leaves_room() {
        assert_eq!(
            models_that_fit_in(8.0),
            ["qwen3.5:0.8b", "qwen3.5:2b", "qwen3.5:4b"]
        );
        assert_eq!(default_model_for(8.0), "qwen3.5:2b");
        assert_eq!(default_model_for(32.0), "qwen3.5:27b");
        assert_eq!(default_model_for(4.0), "qwen3.5:0.8b");
        assert_eq!(default_model_for(2.0), "qwen3.5:0.8b");
    }
}
