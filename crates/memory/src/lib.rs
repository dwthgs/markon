//! Memory: conversations, turns, cards, facts and preferences in one SQLite file (WAL),
//! with FTS5 recall. Facts come only from explicit user statements or confirmed tool results.
use rusqlite::OptionalExtension;
/// Re-exported for crates that keep their own tables in this database (the scheduler).
pub use rusqlite::{Connection, params};
use serde::Serialize;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const MIGRATIONS: &[&str] = &[include_str!("../migrations/001_initial.sql")];
pub const SCHEMA_VERSION: i64 = MIGRATIONS.len() as i64;

#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),
    #[error("model guesses are never stored as facts")]
    NotAFact,
}
pub type Result<T> = std::result::Result<T, MemoryError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Voice,
    Text,
    Schedule,
}
impl Source {
    fn as_str(self) -> &'static str {
        match self {
            Source::Voice => "voice",
            Source::Text => "text",
            Source::Schedule => "schedule",
        }
    }
}

/// Where a fact came from. `ModelGuess` exists so callers can say it, and be refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactOrigin {
    UserStated,
    ToolConfirmed,
    ModelGuess,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Fact {
    pub id: i64,
    pub subject: String,
    pub statement: String,
    pub confidence: f64,
    pub created_at: i64,
    pub superseded_by: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Conversation {
    pub id: i64,
    pub started_at: i64,
    pub title: String,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StoredCard {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub payload: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Turn {
    pub id: i64,
    pub role: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Usage {
    pub provider: String,
    pub model: String,
    pub tokens_in: i64,
    pub tokens_out: i64,
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// UTC calendar day for the usage table.
fn today() -> String {
    let days = now() / 86_400_000;
    // Civil-from-days (Howard Hinnant), enough for a daily key.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

const STOP: &[&str] = &[
    "a", "an", "the", "is", "are", "am", "was", "do", "does", "did", "i", "me", "my", "you",
    "your", "what", "which", "who", "how", "to", "of", "in", "on", "and", "or", "it", "that",
    "this", "for", "about", "with", "can", "be", "please", "tell",
];

/// An FTS5 query that ORs the meaningful words, each quoted so user text cannot inject syntax.
fn fts_query(text: &str) -> Option<String> {
    let words: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .map(|w| w.to_lowercase())
        .filter(|w| w.len() > 1 && !STOP.contains(&w.as_str()))
        .map(|w| format!("\"{w}\""))
        .collect();
    (!words.is_empty()).then(|| words.join(" OR "))
}

pub struct Memory {
    db: Mutex<Connection>,
}

impl Memory {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(p) = path.parent() {
            let _ = std::fs::create_dir_all(p);
        }
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(c: Connection) -> Result<Self> {
        // WAL keeps the file consistent after a crash; busy_timeout rides out a lock left by one.
        let _: String = c.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
        c.execute_batch(
            "PRAGMA foreign_keys=ON; PRAGMA busy_timeout=3000; PRAGMA synchronous=NORMAL;",
        )?;
        let v: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(v as usize) {
            let tx = c.unchecked_transaction()?;
            tx.execute_batch(sql)?;
            tx.execute_batch(&format!("PRAGMA user_version={}", i + 1))?;
            tx.commit()?;
        }
        Ok(Self { db: Mutex::new(c) })
    }

    fn db(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.db.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn schema_version(&self) -> Result<i64> {
        Ok(self
            .db()
            .query_row("PRAGMA user_version", [], |r| r.get(0))?)
    }

    pub fn journal_mode(&self) -> Result<String> {
        Ok(self
            .db()
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))?)
    }

    pub fn start_conversation(&self, source: Source) -> Result<i64> {
        let db = self.db();
        db.execute(
            "INSERT INTO conversations(started_at, source) VALUES (?1, ?2)",
            params![now(), source.as_str()],
        )?;
        Ok(db.last_insert_rowid())
    }

    pub fn set_title(&self, conv: i64, title: &str) -> Result<()> {
        self.db().execute(
            "UPDATE conversations SET title=?2 WHERE id=?1",
            params![conv, title],
        )?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_turn(
        &self,
        conv: i64,
        role: &str,
        text: &str,
        tokens_in: u64,
        tokens_out: u64,
        provider: &str,
        model: &str,
    ) -> Result<i64> {
        let db = self.db();
        db.execute(
            "INSERT INTO turns(conversation_id, role, text, tokens_in, tokens_out, provider, model, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![conv, role, text, tokens_in as i64, tokens_out as i64, provider, model, now()],
        )?;
        Ok(db.last_insert_rowid())
    }

    pub fn turns(&self, conv: i64) -> Result<Vec<Turn>> {
        let db = self.db();
        let mut st = db.prepare_cached(
            "SELECT id, role, text FROM turns WHERE conversation_id=?1 ORDER BY id",
        )?;
        let rows = st.query_map([conv], |r| {
            Ok(Turn {
                id: r.get(0)?,
                role: r.get(1)?,
                text: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Store a fact. A newer fact about the same subject supersedes the older one.
    pub fn add_fact(
        &self,
        subject: &str,
        statement: &str,
        source_turn: i64,
        confidence: f64,
        origin: FactOrigin,
    ) -> Result<i64> {
        let origin = match origin {
            FactOrigin::UserStated => "user_stated",
            FactOrigin::ToolConfirmed => "tool_confirmed",
            FactOrigin::ModelGuess => return Err(MemoryError::NotAFact),
        };
        let mut db = self.db();
        let tx = db.transaction()?;
        tx.execute(
            "INSERT INTO facts(subject, statement, source_turn_id, origin, confidence, created_at) VALUES (?1,?2,?3,?4,?5,?6)",
            params![subject, statement, source_turn, origin, confidence, now()],
        )?;
        let id = tx.last_insert_rowid();
        tx.execute("UPDATE facts SET superseded_by=?1 WHERE subject=?2 AND id<>?1 AND superseded_by IS NULL AND derived_from IS NULL", params![id, subject])?;
        tx.commit()?;
        Ok(id)
    }

    /// A fact that exists only because of another one (forgotten with it).
    pub fn add_derived_fact(&self, subject: &str, statement: &str, from: i64) -> Result<i64> {
        let db = self.db();
        db.execute(
            "INSERT INTO facts(subject, statement, derived_from, origin, confidence, created_at) VALUES (?1,?2,?3,'derived',1.0,?4)",
            params![subject, statement, from, now()],
        )?;
        Ok(db.last_insert_rowid())
    }

    fn fact_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Fact> {
        Ok(Fact {
            id: r.get(0)?,
            subject: r.get(1)?,
            statement: r.get(2)?,
            confidence: r.get(3)?,
            created_at: r.get(4)?,
            superseded_by: r.get(5)?,
        })
    }

    pub fn fact(&self, id: i64) -> Result<Option<Fact>> {
        Ok(self
            .db()
            .query_row("SELECT id, subject, statement, confidence, created_at, superseded_by FROM facts WHERE id=?1", [id], Self::fact_row)
            .optional()?)
    }

    /// Current facts for a subject; with `with_superseded`, the older ones too (shown on request).
    pub fn facts_about(&self, subject: &str, with_superseded: bool) -> Result<Vec<Fact>> {
        let db = self.db();
        let mut st = db.prepare_cached(
            "SELECT id, subject, statement, confidence, created_at, superseded_by FROM facts WHERE subject=?1 AND (?2 OR superseded_by IS NULL) ORDER BY id",
        )?;
        let rows = st.query_map(params![subject, with_superseded], Self::fact_row)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// The `k` most relevant current facts for a prompt.
    pub fn recall(&self, text: &str, k: usize) -> Result<Vec<Fact>> {
        let Some(q) = fts_query(text) else {
            return Ok(Vec::new());
        };
        let db = self.db();
        let mut st = db.prepare_cached(
            "SELECT f.id, f.subject, f.statement, f.confidence, f.created_at, f.superseded_by
             FROM facts_fts JOIN facts f ON f.id = facts_fts.rowid
             WHERE facts_fts MATCH ?1 AND f.superseded_by IS NULL
             ORDER BY bm25(facts_fts) LIMIT ?2",
        )?;
        let rows = st.query_map(params![q, k as i64], Self::fact_row)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// The newest facts that are still current: what Markon knows about the user, whatever the question.
    pub fn latest_facts(&self, k: usize) -> Result<Vec<Fact>> {
        let db = self.db();
        let mut st = db.prepare_cached(
            "SELECT id, subject, statement, confidence, created_at, superseded_by
             FROM facts WHERE superseded_by IS NULL ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = st.query_map([k as i64], Self::fact_row)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn forget(&self, id: i64) -> Result<bool> {
        // Derived facts go with it (ON DELETE CASCADE on derived_from).
        Ok(self.db().execute("DELETE FROM facts WHERE id=?1", [id])? > 0)
    }

    /// "Forget that": the newest fact written in this conversation.
    pub fn forget_last(&self, conv: i64) -> Result<Option<i64>> {
        let id: Option<i64> = self
            .db()
            .query_row(
                "SELECT f.id FROM facts f JOIN turns t ON t.id = f.source_turn_id WHERE t.conversation_id=?1 ORDER BY f.id DESC LIMIT 1",
                [conv],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = id {
            self.forget(id)?;
        }
        Ok(id)
    }

    pub fn save_card(
        &self,
        conv: i64,
        id: &str,
        kind: &str,
        title: &str,
        payload: &Value,
    ) -> Result<()> {
        self.db().execute(
            "INSERT INTO cards(id, conversation_id, kind, title, payload_json, created_at) VALUES (?1,?2,?3,?4,?5,?6)
             ON CONFLICT(conversation_id, id) DO UPDATE SET kind=excluded.kind, title=excluded.title, payload_json=excluded.payload_json",
            params![id, conv, kind, title, payload.to_string(), now()],
        )?;
        Ok(())
    }

    pub fn delete_cards_where(&self, conv: i64, ids: &[String]) -> Result<()> {
        let db = self.db();
        for id in ids {
            db.execute(
                "DELETE FROM cards WHERE conversation_id=?1 AND id=?2",
                params![conv, id],
            )?;
        }
        Ok(())
    }

    pub fn cards(&self, conv: i64) -> Result<Vec<StoredCard>> {
        let db = self.db();
        let mut st = db.prepare_cached("SELECT id, kind, title, payload_json FROM cards WHERE conversation_id=?1 ORDER BY created_at, rowid")?;
        let rows = st.query_map([conv], |r| {
            let p: String = r.get(3)?;
            Ok(StoredCard {
                id: r.get(0)?,
                kind: r.get(1)?,
                title: r.get(2)?,
                payload: serde_json::from_str(&p).unwrap_or(Value::Null),
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn conversations(&self, limit: usize) -> Result<Vec<Conversation>> {
        let db = self.db();
        let mut st = db.prepare_cached(
            "SELECT id, started_at, title, source FROM conversations ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = st.query_map([limit as i64], |r| {
            Ok(Conversation {
                id: r.get(0)?,
                started_at: r.get(1)?,
                title: r.get(2)?,
                source: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// "Bring back [topic]": the latest conversation whose title or turns match.
    pub fn find_conversation(&self, topic: &str) -> Result<Option<i64>> {
        let Some(q) = fts_query(topic) else {
            return Ok(None);
        };
        let db = self.db();
        let by_turn: Option<i64> = db
            .query_row(
                "SELECT t.conversation_id FROM turns_fts JOIN turns t ON t.id = turns_fts.rowid WHERE turns_fts MATCH ?1 ORDER BY t.id DESC LIMIT 1",
                [&q],
                |r| r.get(0),
            )
            .optional()?;
        let words: Vec<String> = q
            .split(" OR ")
            .map(|w| w.trim_matches('"').to_string())
            .collect();
        let mut st =
            db.prepare_cached("SELECT id, title FROM conversations ORDER BY id DESC LIMIT 500")?;
        let by_title = st
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
            .filter_map(|r| r.ok())
            .find(|(_, t)| {
                let t = t.to_lowercase();
                words
                    .iter()
                    .any(|w| t.split(|c: char| !c.is_alphanumeric()).any(|x| x == w))
            })
            .map(|(id, _)| id);
        Ok(match (by_title, by_turn) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        })
    }

    pub fn record_usage(
        &self,
        provider: &str,
        model: &str,
        tokens_in: u64,
        tokens_out: u64,
    ) -> Result<()> {
        self.db().execute(
            "INSERT INTO usage(day, provider, model, tokens_in, tokens_out) VALUES (?1,?2,?3,?4,?5)
             ON CONFLICT(day, provider, model) DO UPDATE SET tokens_in=tokens_in+excluded.tokens_in, tokens_out=tokens_out+excluded.tokens_out",
            params![today(), provider, model, tokens_in as i64, tokens_out as i64],
        )?;
        Ok(())
    }

    pub fn usage_today(&self) -> Result<Vec<Usage>> {
        let db = self.db();
        let mut st = db.prepare_cached(
            "SELECT provider, model, tokens_in, tokens_out FROM usage WHERE day=?1",
        )?;
        let rows = st.query_map([today()], |r| {
            Ok(Usage {
                provider: r.get(0)?,
                model: r.get(1)?,
                tokens_in: r.get(2)?,
                tokens_out: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn set_preference(&self, key: &str, value: &Value) -> Result<()> {
        self.db().execute(
            "INSERT INTO preferences(key, value_json, updated_at) VALUES (?1,?2,?3) ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json, updated_at=excluded.updated_at",
            params![key, value.to_string(), now()],
        )?;
        Ok(())
    }

    /// Run `f` with the connection (used by the scheduler for the schedules table).
    pub fn with<T>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Result<T> {
        Ok(f(&self.db())?)
    }

    pub fn export(&self) -> Result<Value> {
        let db = self.db();
        let dump = |sql: &str, cols: &[&str]| -> rusqlite::Result<Value> {
            let mut st = db.prepare(sql)?;
            let rows = st.query_map([], |r| {
                let mut o = serde_json::Map::new();
                for (i, c) in cols.iter().enumerate() {
                    let v: rusqlite::types::Value = r.get(i)?;
                    o.insert(
                        (*c).into(),
                        match v {
                            rusqlite::types::Value::Null => Value::Null,
                            rusqlite::types::Value::Integer(n) => json!(n),
                            rusqlite::types::Value::Real(f) => json!(f),
                            rusqlite::types::Value::Text(t) => json!(t),
                            rusqlite::types::Value::Blob(_) => Value::Null,
                        },
                    );
                }
                Ok(Value::Object(o))
            })?;
            Ok(Value::Array(rows.collect::<rusqlite::Result<_>>()?))
        };
        Ok(json!({
            "schema_version": SCHEMA_VERSION,
            "conversations": dump("SELECT id, started_at, title, source FROM conversations", &["id", "started_at", "title", "source"])?,
            "turns": dump("SELECT id, conversation_id, role, text, created_at FROM turns", &["id", "conversation_id", "role", "text", "created_at"])?,
            "facts": dump("SELECT id, subject, statement, origin, confidence, created_at, superseded_by FROM facts", &["id", "subject", "statement", "origin", "confidence", "created_at", "superseded_by"])?,
            "preferences": dump("SELECT key, value_json, updated_at FROM preferences", &["key", "value_json", "updated_at"])?,
            "cards": dump("SELECT id, conversation_id, kind, title, payload_json FROM cards", &["id", "conversation_id", "kind", "title", "payload_json"])?,
        }))
    }

    /// Delete everything Markon remembers (settings and keychain are wiped by the app).
    pub fn wipe(&self) -> Result<()> {
        self.db().execute_batch(
            "DELETE FROM cards; DELETE FROM tool_calls; DELETE FROM facts; DELETE FROM turns; DELETE FROM conversations;
             DELETE FROM preferences; DELETE FROM usage; INSERT INTO facts_fts(facts_fts) VALUES('rebuild');
             INSERT INTO turns_fts(turns_fts) VALUES('rebuild'); VACUUM;",
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn fts_queries_are_quoted() {
        assert_eq!(
            super::fts_query("what coffee do I like?").as_deref(),
            Some("\"coffee\" OR \"like\"")
        );
        assert_eq!(
            super::fts_query("\"); DROP TABLE facts; --").as_deref(),
            Some("\"drop\" OR \"table\" OR \"facts\"")
        );
        assert_eq!(super::fts_query("the a"), None);
    }

    #[test]
    fn today_is_a_date() {
        let d = super::today();
        assert_eq!(d.len(), 10);
        assert!(d.starts_with("20"));
    }
}
