//! Traces: one row per turn with where its
//! time went. They never leave the Mac.
use crate::{Memory, Result, now};
use rusqlite::params;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ToolTime {
    pub name: String,
    pub ms: u64,
    pub ok: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Trace {
    pub id: i64,
    pub at: i64,
    pub user_text: String,
    pub provider: String,
    pub model: String,
    /// Speech to text, from the end of speech to the transcript.
    pub stt_ms: Option<u64>,
    /// From the request to the model's first word.
    pub first_text_ms: Option<u64>,
    /// From the request to the first sound.
    pub first_audio_ms: Option<u64>,
    pub total_ms: u64,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub tools: Vec<ToolTime>,
    pub ok: bool,
}

impl Memory {
    pub fn record_trace(&self, t: &Trace) -> Result<i64> {
        let db = self.db();
        db.execute(
            "INSERT INTO traces(at, user_text, provider, model, stt_ms, first_text_ms, first_audio_ms, total_ms, tokens_in, tokens_out, tools_json, ok)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            params![
                now(),
                t.user_text.chars().take(200).collect::<String>(),
                t.provider,
                t.model,
                t.stt_ms.map(|v| v as i64),
                t.first_text_ms.map(|v| v as i64),
                t.first_audio_ms.map(|v| v as i64),
                t.total_ms as i64,
                t.tokens_in as i64,
                t.tokens_out as i64,
                serde_json::to_string(&t.tools).unwrap_or_else(|_| "[]".into()),
                t.ok,
            ],
        )?;
        let id = db.last_insert_rowid();
        // Keep the newest 2,000.
        db.execute("DELETE FROM traces WHERE id <= ?1 - 2000", [id])?;
        Ok(id)
    }

    /// The first sound of a turn arrives after the turn is written.
    pub fn set_first_audio(&self, id: i64, ms: u64) -> Result<()> {
        self.db().execute(
            "UPDATE traces SET first_audio_ms=?2 WHERE id=?1",
            params![id, ms as i64],
        )?;
        Ok(())
    }

    pub fn traces(&self, limit: usize) -> Result<Vec<Trace>> {
        let db = self.db();
        let mut st = db.prepare(
            "SELECT id, at, user_text, provider, model, stt_ms, first_text_ms, first_audio_ms, total_ms, tokens_in, tokens_out, tools_json, ok
             FROM traces ORDER BY id DESC LIMIT ?1",
        )?;
        let v = st
            .query_map([limit as i64], |r| {
                let tools: String = r.get(11)?;
                Ok(Trace {
                    id: r.get(0)?,
                    at: r.get(1)?,
                    user_text: r.get(2)?,
                    provider: r.get(3)?,
                    model: r.get(4)?,
                    stt_ms: r.get::<_, Option<i64>>(5)?.map(|v| v as u64),
                    first_text_ms: r.get::<_, Option<i64>>(6)?.map(|v| v as u64),
                    first_audio_ms: r.get::<_, Option<i64>>(7)?.map(|v| v as u64),
                    total_ms: r.get::<_, i64>(8)? as u64,
                    tokens_in: r.get::<_, i64>(9)? as u64,
                    tokens_out: r.get::<_, i64>(10)? as u64,
                    tools: serde_json::from_str(&tools).unwrap_or_default(),
                    ok: r.get(12)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(v)
    }
}
