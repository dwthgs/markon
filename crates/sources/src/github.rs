//! GitHub notifications with a personal access token.
use crate::{BoxFuture, Item, Source, SourceError, clip};
use serde_json::Value;
use std::sync::Arc;

pub type Token = Arc<dyn Fn() -> Option<String> + Send + Sync>;

pub struct GitHub {
    pub base: String,
    pub token: Token,
    pub http: reqwest::Client,
}

impl GitHub {
    pub fn new(token: Token) -> Self {
        Self {
            base: "https://api.github.com".into(),
            token,
            http: reqwest::Client::new(),
        }
    }
}

/// `https://api.github.com/repos/o/r/pulls/7` → `https://github.com/o/r/pull/7`.
pub fn html_link(api: &str) -> Option<String> {
    let rest = api.strip_prefix("https://api.github.com/repos/")?;
    Some(format!(
        "https://github.com/{}",
        rest.replace("/pulls/", "/pull/")
    ))
}

pub fn parse(v: &Value) -> Vec<Item> {
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|n| {
                    let s = |p: &str| {
                        n.pointer(p)
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string()
                    };
                    let when = chrono::DateTime::parse_from_rfc3339(&s("/updated_at"))
                        .map(|t| t.timestamp_millis())
                        .unwrap_or(0);
                    Item {
                        source: "github".into(),
                        id: s("/id"),
                        who: s("/repository/full_name"),
                        place: s("/subject/type"),
                        when_ms: when,
                        text: clip(
                            &format!(
                                "{} ({})",
                                s("/subject/title"),
                                s("/reason").replace('_', " ")
                            ),
                            300,
                        ),
                        link: html_link(&s("/subject/url")),
                        unread: n["unread"].as_bool().unwrap_or(true),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

impl Source for GitHub {
    fn id(&self) -> &str {
        "github"
    }
    fn label(&self) -> &str {
        "GitHub"
    }
    fn fetch(&self, since_ms: i64) -> BoxFuture<'static, Result<Vec<Item>, SourceError>> {
        let token = (self.token)();
        let http = self.http.clone();
        let since = chrono::DateTime::from_timestamp_millis(since_ms)
            .unwrap_or_default()
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let url = format!("{}/notifications?since={since}", self.base);
        Box::pin(async move {
            let token =
                token.ok_or_else(|| SourceError::NotConnected("no GitHub token saved".into()))?;
            let r = http
                .get(url)
                .bearer_auth(token)
                .header("accept", "application/vnd.github+json")
                .header("x-github-api-version", "2022-11-28")
                .header("user-agent", "Markon")
                .send()
                .await
                .map_err(|e| SourceError::Failed(e.without_url().to_string()))?;
            match r.status().as_u16() {
                200 => {}
                401 | 403 => {
                    return Err(SourceError::NotConnected("GitHub refused the token".into()));
                }
                s => return Err(SourceError::Failed(format!("GitHub returned {s}"))),
            }
            let v: Value = r
                .json()
                .await
                .map_err(|e| SourceError::Failed(e.to_string()))?;
            Ok(parse(&v))
        })
    }
}
