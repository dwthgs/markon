//! The index: approved folders and sources cut into chunks,
//! found by keyword (FTS5) and by meaning (a vector per chunk), with the two
//! rankings fused by reciprocal rank (their `hybrid_search.py`: k = 60, equal weights).
use crate::{Memory, Result, fts_query, now};
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use std::collections::HashMap;

pub const MAX_CHARS: usize = 1600;
pub const OVERLAP_CHARS: usize = 200;
pub const RRF_K: f64 = 60.0;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Hit {
    pub id: i64,
    pub source: String,
    pub reference: String,
    pub title: String,
    pub text: String,
    pub score: f64,
}

fn cut_at(s: &str, max: usize) -> usize {
    if s.len() <= max {
        return s.len();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    // Prefer a sentence end, then a space, in the last third of the window.
    let window = &s[..end];
    let floor = end * 2 / 3;
    window
        .rfind(['.', '!', '?', '\n'])
        .filter(|i| *i > floor)
        .map(|i| i + 1)
        .or_else(|| window.rfind(' ').filter(|i| *i > floor))
        .unwrap_or(end)
}

/// Paragraphs packed up to `max` characters, long ones split at sentence ends, each chunk starting with
/// the last `overlap` characters of the one before.
pub fn chunk(text: &str, max: usize, overlap: usize) -> Vec<String> {
    let mut pieces: Vec<String> = Vec::new();
    for para in text.split("\n\n").map(str::trim).filter(|p| !p.is_empty()) {
        let mut rest = para;
        while !rest.is_empty() {
            let end = cut_at(rest, max);
            pieces.push(rest[..end].trim().to_string());
            rest = rest[end..].trim_start();
        }
    }
    let mut packed: Vec<String> = Vec::new();
    for p in pieces {
        match packed.last_mut() {
            Some(last) if last.len() + p.len() + 2 <= max => {
                last.push_str("\n\n");
                last.push_str(&p);
            }
            _ => packed.push(p),
        }
    }
    let mut out = Vec::with_capacity(packed.len());
    for (i, c) in packed.iter().enumerate() {
        if i == 0 || overlap == 0 {
            out.push(c.clone());
            continue;
        }
        let prev = &packed[i - 1];
        let mut start = prev.len().saturating_sub(overlap);
        while !prev.is_char_boundary(start) {
            start += 1;
        }
        let tail = prev[start..]
            .trim_start_matches(|ch: char| !ch.is_whitespace())
            .trim();
        out.push(if tail.is_empty() {
            c.clone()
        } else {
            format!("{tail} {c}")
        });
    }
    out
}

fn to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn from_blob(b: &[u8]) -> Vec<f32> {
    b.as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect()
}

fn cosine(a: &[f32], b: &[f32]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let (mut dot, mut na, mut nb) = (0.0f64, 0.0f64, 0.0f64);
    for (x, y) in a.iter().zip(b) {
        dot += (*x as f64) * (*y as f64);
        na += (*x as f64).powi(2);
        nb += (*y as f64).powi(2);
    }
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na.sqrt() * nb.sqrt())
    }
}

/// Reciprocal rank fusion: each list adds `weight / (k + rank)` for every id in it (rank from 1).
pub fn rrf(lists: &[(f64, Vec<i64>)], k: f64) -> Vec<(i64, f64)> {
    let mut score: HashMap<i64, f64> = HashMap::new();
    for (w, ids) in lists {
        for (rank, id) in ids.iter().enumerate() {
            *score.entry(*id).or_default() += w / (k + rank as f64 + 1.0);
        }
    }
    let mut v: Vec<(i64, f64)> = score.into_iter().collect();
    v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    v
}

pub type Embed<'a> = &'a dyn Fn(&str) -> Option<Vec<f32>>;

impl Memory {
    /// When `reference` was last indexed (its file's modification time), if it was.
    pub fn indexed_mtime(&self, reference: &str) -> Result<Option<i64>> {
        Ok(self
            .db()
            .query_row(
                "SELECT MAX(mtime) FROM chunks WHERE reference=?1",
                [reference],
                |r| r.get::<_, Option<i64>>(0),
            )
            .optional()?
            .flatten())
    }

    /// Replace what is indexed for `reference` with `text`, cut into chunks, each with a vector if
    /// `embed` gives one. Returns the number of chunks.
    pub fn index(
        &self,
        source: &str,
        reference: &str,
        title: &str,
        mtime: i64,
        text: &str,
        embed: Option<Embed<'_>>,
    ) -> Result<usize> {
        let chunks = chunk(text, MAX_CHARS, OVERLAP_CHARS);
        let vectors: Vec<Option<Vec<u8>>> = chunks
            .iter()
            .map(|c| {
                embed
                    .and_then(|e| e(&format!("{title}\n{c}")))
                    .map(|v| to_blob(&v))
            })
            .collect();
        let db = self.db();
        let tx = db.unchecked_transaction()?;
        tx.execute("DELETE FROM chunks WHERE reference=?1", [reference])?;
        for (c, v) in chunks.iter().zip(vectors) {
            tx.execute(
                "INSERT INTO chunks(source, reference, title, text, mtime, vector) VALUES (?1,?2,?3,?4,?5,?6)",
                params![source, reference, title, c, mtime, v],
            )?;
        }
        tx.commit()?;
        Ok(chunks.len())
    }

    /// Forget everything indexed from these references (a folder taken off the approved list).
    pub fn unindex_where(&self, reference_prefix: &str) -> Result<usize> {
        Ok(self.db().execute(
            "DELETE FROM chunks WHERE substr(reference, 1, length(?1)) = ?1",
            [reference_prefix],
        )?)
    }

    pub fn chunk_count(&self) -> Result<i64> {
        Ok(self
            .db()
            .query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))?)
    }

    fn keyword_ids(&self, q: &str, k: usize) -> Result<Vec<i64>> {
        let Some(fq) = fts_query(q) else {
            return Ok(vec![]);
        };
        let db = self.db();
        let mut st = db.prepare("SELECT rowid FROM chunks_fts WHERE chunks_fts MATCH ?1 ORDER BY bm25(chunks_fts) LIMIT ?2")?;
        let ids = st
            .query_map(params![fq, k as i64], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(ids)
    }

    fn vector_ids(&self, qv: &[f32], k: usize) -> Result<Vec<i64>> {
        // shortcut: a brute-force scan; fine to tens of thousands of chunks, then add a vector index.
        let db = self.db();
        let mut st = db.prepare("SELECT id, vector FROM chunks WHERE vector IS NOT NULL")?;
        let mut scored: Vec<(i64, f64)> = st
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?)))?
            .flatten()
            .map(|(id, b)| (id, cosine(qv, &from_blob(&b))))
            .collect();
        scored.sort_by(|a, b| b.1.total_cmp(&a.1));
        Ok(scored.into_iter().take(k).map(|(id, _)| id).collect())
    }

    /// Keyword and meaning search fused. Without a query vector it is keyword search alone.
    pub fn search(&self, q: &str, qv: Option<&[f32]>, k: usize) -> Result<Vec<Hit>> {
        let recall = 200.max(k);
        let mut lists = vec![(0.5, self.keyword_ids(q, recall)?)];
        if let Some(v) = qv {
            lists.push((0.5, self.vector_ids(v, recall)?));
        }
        let fused = rrf(&lists, RRF_K);
        let db = self.db();
        let mut st = db.prepare("SELECT source, reference, title, text FROM chunks WHERE id=?1")?;
        let mut out = Vec::new();
        for (id, score) in fused.into_iter().take(k) {
            if let Some((source, reference, title, text)) = st
                .query_row([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
                .optional()?
            {
                out.push(Hit {
                    id,
                    source,
                    reference,
                    title,
                    text,
                    score,
                });
            }
        }
        Ok(out)
    }

    // ---------- one person across every source ----------

    /// Say that `value` (a phone number, mail address, Slack or Telegram name) is `name`.
    pub fn link_person(&self, name: &str, kind: &str, value: &str) -> Result<i64> {
        let db = self.db();
        db.execute(
            "INSERT INTO people(name) VALUES (?1) ON CONFLICT(name) DO NOTHING",
            [name.trim()],
        )?;
        let id: i64 = db.query_row("SELECT id FROM people WHERE name=?1", [name.trim()], |r| {
            r.get(0)
        })?;
        db.execute(
            "INSERT INTO identities(person_id, kind, value) VALUES (?1,?2,?3) ON CONFLICT(kind, value) DO UPDATE SET person_id=excluded.person_id",
            params![id, kind, normalise_identity(kind, value)],
        )?;
        Ok(id)
    }

    /// The person behind an identity, if known.
    pub fn person_for(&self, kind: &str, value: &str) -> Result<Option<String>> {
        Ok(self
            .db()
            .query_row(
                "SELECT p.name FROM identities i JOIN people p ON p.id = i.person_id WHERE i.kind=?1 AND i.value=?2",
                params![kind, normalise_identity(kind, value)],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Every identity a person has, as (kind, value).
    pub fn identities(&self, name: &str) -> Result<Vec<(String, String)>> {
        let db = self.db();
        let mut st = db.prepare(
            "SELECT i.kind, i.value FROM identities i JOIN people p ON p.id = i.person_id WHERE p.name=?1 ORDER BY i.kind, i.value",
        )?;
        let v = st
            .query_map([name.trim()], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(v)
    }

    pub fn touch_index(&self) -> Result<()> {
        self.set_preference("index.updated_ms", &serde_json::json!(now()))
    }
}

/// Phone numbers by their last nine digits, everything else lowercased.
pub fn normalise_identity(kind: &str, value: &str) -> String {
    if kind == "phone" {
        let d: String = value.chars().filter(char::is_ascii_digit).collect();
        return d[d.len().saturating_sub(9)..].to_string();
    }
    value.trim().to_lowercase()
}
