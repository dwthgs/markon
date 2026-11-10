//! Route C: the free trial through our relay (services/relay). The device registers with a random id,
//! gets a short-lived token (kept in the keychain by the app), then talks the Messages API to the relay.
use crate::anthropic::{Auth, MessagesClient};
use crate::*;
use serde_json::json;
use std::time::Duration;

/// Stores the relay token (the keychain in the app).
pub type TokenStore = Arc<dyn Fn(Option<&str>) + Send + Sync>;

pub struct Relay {
    http: reqwest::Client,
    base_url: String,
    device_id: String,
    token: KeySource,
    store: TokenStore,
}

impl Relay {
    pub fn new(base_url: &str, device_id: &str, token: KeySource, store: TokenStore) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(60))
            .build()
            .expect("http client");
        Self {
            http,
            base_url: base_url.trim_end_matches('/').into(),
            device_id: device_id.into(),
            token,
            store,
        }
    }

    async fn register(
        http: &reqwest::Client,
        base: &str,
        device: &str,
    ) -> Result<String, ProviderError> {
        let resp = http
            .post(format!("{base}/v1/register"))
            .json(&json!({"device_id": device}))
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .map_err(|e| {
                ProviderError::Network(format!(
                    "the trial service is not reachable ({})",
                    e.without_url()
                ))
            })?;
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        if status >= 400 {
            return Err(ProviderError::from_status(status, &text, None));
        }
        let v: Value =
            serde_json::from_str(&text).map_err(|e| ProviderError::Parse(e.to_string()))?;
        v["token"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| ProviderError::Parse("no token".into()))
    }
}

impl ModelProvider for Relay {
    fn route(&self) -> &'static str {
        "relay"
    }

    fn stream(&self, req: ChatRequest) -> DeltaStream {
        let (tx, rx) = mpsc::channel(64);
        let (http, base, device, token, store) = (
            self.http.clone(),
            self.base_url.clone(),
            self.device_id.clone(),
            self.token.clone(),
            self.store.clone(),
        );
        tokio::spawn(async move {
            let mut current = token();
            for attempt in 0..2 {
                let t = match current.take() {
                    Some(t) => t,
                    None => match Relay::register(&http, &base, &device).await {
                        Ok(t) => {
                            store(Some(&t));
                            t
                        }
                        Err(e) => {
                            let _ = tx.send(Err(e)).await;
                            return;
                        }
                    },
                };
                let tok = t.clone();
                let client = MessagesClient::new(
                    base.clone(),
                    Auth::Bearer(Arc::new(move || Some(tok.clone()))),
                    false,
                );
                let mut inner = client.stream(req.clone());
                let mut forwarded = false;
                while let Some(d) = inner.recv().await {
                    // An expired token before anything streamed: register again once.
                    if matches!(d, Err(ProviderError::Auth)) && !forwarded && attempt == 0 {
                        store(None);
                        break;
                    }
                    forwarded = true;
                    if tx.send(d).await.is_err() {
                        return;
                    }
                }
                if forwarded {
                    return;
                }
            }
        });
        rx
    }

    fn models(&self) -> Vec<ModelInfo> {
        vec![ModelInfo {
            id: crate::anthropic::DEFAULT_MODEL.into(),
            name: "Claude (free trial)".into(),
        }]
    }

    fn default_model(&self) -> String {
        crate::anthropic::DEFAULT_MODEL.into()
    }

    fn validate(&self) -> BoxFuture<'_, Result<Account, ProviderError>> {
        Box::pin(async move {
            let t = match (self.token)() {
                Some(t) => t,
                None => {
                    let t = Relay::register(&self.http, &self.base_url, &self.device_id).await?;
                    (self.store)(Some(&t));
                    t
                }
            };
            let resp = self
                .http
                .get(format!("{}/v1/quota", self.base_url))
                .bearer_auth(t)
                .timeout(Duration::from_secs(3))
                .send()
                .await
                .map_err(|e| ProviderError::Network(e.without_url().to_string()))?;
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            if status >= 400 {
                return Err(ProviderError::from_status(status, &text, None));
            }
            let v: Value =
                serde_json::from_str(&text).map_err(|e| ProviderError::Parse(e.to_string()))?;
            let left = v["limit"]
                .as_u64()
                .unwrap_or(0)
                .saturating_sub(v["used"].as_u64().unwrap_or(0));
            Ok(Account {
                route: "relay".into(),
                plan: Some("free trial".into()),
                balance: Some(format!("{left} messages left today")),
            })
        })
    }
}
