//! Cards: what the canvas shows. The Rust core sends them; the UI only renders them.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CardKind {
    Agent,
    File,
    Page,
    App,
    Result,
    Note,
    Image,
    Audio,
    Video,
    Music,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CardStatus {
    Running,
    Done,
    Error,
    Blocked,
    /// Not a card: asks the screen to act on its cards (`body.command`: close, pause or play;
    /// `body.which`: latest, media, all, or words from a title).
    Command,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Card {
    pub id: String,
    pub turn_id: u64,
    pub kind: CardKind,
    pub title: String,
    pub status: CardStatus,
    pub body: Value,
    pub updated_ms: u64,
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl Card {
    pub fn new(
        id: impl Into<String>,
        turn_id: u64,
        kind: CardKind,
        title: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            turn_id,
            kind,
            title: title.into(),
            status: CardStatus::Running,
            body: Value::Null,
            updated_ms: now_ms(),
        }
    }
    pub fn with(mut self, status: CardStatus, body: Value) -> Self {
        self.status = status;
        self.body = body;
        self.updated_ms = now_ms();
        self
    }
}

pub type CardSink = Arc<dyn Fn(Card) + Send + Sync>;
