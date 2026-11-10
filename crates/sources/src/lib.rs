//! Sources: what happened in the user's accounts, in one shape. Read-only.
pub mod act;
pub mod github;
pub mod imap;
pub mod mac;
pub mod mcp;
pub mod search;
pub mod smtp;
pub mod telegram;
pub mod tool;
pub mod watch;

pub use markon_provider::BoxFuture;
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;

/// One thing that happened: a message, a mail, an event, a note, a notification.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Item {
    /// The source's id, e.g. "imessage".
    pub source: String,
    /// Stable within the source, so a watcher can tell new from seen.
    pub id: String,
    pub who: String,
    #[serde(rename = "where")]
    pub place: String,
    /// Unix milliseconds.
    pub when_ms: i64,
    pub text: String,
    pub link: Option<String>,
    pub unread: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, thiserror::Error)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum SourceError {
    /// The Mac refused to open the file: Full Disk Access is missing.
    #[error("Markon needs Full Disk Access to read {0}")]
    NeedsAccess(String),
    /// Not set up: no account, no key, no database.
    #[error("{0}")]
    NotConnected(String),
    #[error("{0}")]
    Failed(String),
}

pub trait Source: Send + Sync {
    fn id(&self) -> &str;
    fn label(&self) -> &str;
    /// Items from `since_ms` (unix milliseconds) until now, newest first.
    fn fetch(&self, since_ms: i64) -> BoxFuture<'static, Result<Vec<Item>, SourceError>>;
}

pub type Sources = Arc<std::sync::RwLock<Vec<Arc<dyn Source>>>>;

/// What one source returned.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub id: String,
    pub label: String,
    pub result: Result<Vec<Item>, SourceError>,
}

/// Ask every source at once; a slow one is cut off at `limit` and reported as failed.
pub async fn collect(sources: &[Arc<dyn Source>], since_ms: i64, limit: Duration) -> Vec<Report> {
    let futs = sources.iter().map(|s| {
        let (id, label) = (s.id().to_string(), s.label().to_string());
        let f = s.fetch(since_ms);
        async move {
            let result = match tokio::time::timeout(limit, f).await {
                Ok(r) => r,
                Err(_) => Err(SourceError::Failed(format!(
                    "{label} did not answer within {} s",
                    limit.as_secs()
                ))),
            };
            Report { id, label, result }
        }
    });
    futures_join_all(futs).await
}

async fn futures_join_all<F>(futs: impl IntoIterator<Item = F>) -> Vec<F::Output>
where
    F: std::future::Future + Send + 'static,
    F::Output: Send + 'static,
{
    let handles: Vec<_> = futs.into_iter().map(tokio::spawn).collect();
    let mut out = Vec::with_capacity(handles.len());
    for h in handles {
        if let Ok(v) = h.await {
            out.push(v);
        }
    }
    out
}

/// Unix milliseconds now.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Cap text at `n` characters with an ellipsis.
pub fn clip(s: &str, n: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= n {
        return s.to_string();
    }
    let mut out: String = s.chars().take(n).collect();
    out.push('…');
    out
}
