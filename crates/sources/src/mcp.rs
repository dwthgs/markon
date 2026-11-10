//! A source read through a connected MCP server (Slack, Notion): one tool call with fixed arguments.
//! The server's reply is free text, so it becomes one item per line block, capped.
use crate::{BoxFuture, Item, Source, SourceError, clip};
use markon_tools::mcp::{McpServer, ServerStatus};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpSourceConfig {
    /// The server's name in `mcp.json`.
    pub server: String,
    pub tool: String,
    #[serde(default)]
    pub args: Value,
}

impl McpSourceConfig {
    /// A guess at Slack's hosted server (not verified against the real server).
    pub fn slack() -> Self {
        Self {
            server: "slack".into(),
            tool: "slack_search_public_and_private".into(),
            args: serde_json::json!({"query": "is:unread", "sort": "timestamp", "limit": 20}),
        }
    }
}

pub struct McpSource {
    pub cfg: McpSourceConfig,
    pub server: Arc<McpServer>,
}

/// Split a reply into items: blank lines separate entries; a short reply is one item.
pub fn items_from_text(source: &str, text: &str) -> Vec<Item> {
    let now = crate::now_ms();
    text.split("\n\n")
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .take(30)
        .enumerate()
        .map(|(i, b)| Item {
            source: source.into(),
            id: format!("{i}:{}", clip(b, 40)),
            who: String::new(),
            place: String::new(),
            when_ms: now,
            text: clip(b, 600),
            link: None,
            unread: true,
        })
        .collect()
}

impl Source for McpSource {
    fn id(&self) -> &str {
        &self.cfg.server
    }
    fn label(&self) -> &str {
        &self.cfg.server
    }
    fn fetch(&self, _since_ms: i64) -> BoxFuture<'static, Result<Vec<Item>, SourceError>> {
        let (server, cfg) = (self.server.clone(), self.cfg.clone());
        Box::pin(async move {
            match server.status() {
                ServerStatus::Online { .. } => {}
                ServerStatus::NeedsSignIn => {
                    return Err(SourceError::NotConnected(format!("sign in to {} in settings", cfg.server)));
                }
                other => {
                    return Err(SourceError::NotConnected(format!("{} is not connected ({other:?})", cfg.server)));
                }
            }
            let text = server.call(&cfg.tool, cfg.args.clone()).await.map_err(SourceError::Failed)?;
            Ok(items_from_text(&cfg.server, &text))
        })
    }
}
