//! The activity log: one row per tool action that changed something, with what it
//! takes to undo it when that is possible. On this Mac only; wiped with everything else.
use crate::{Memory, Result, now};
use rusqlite::params;
use serde::Serialize;
use serde_json::Value;

const KEEP: i64 = 1000;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Action {
    pub id: i64,
    pub at: i64,
    pub tool: String,
    pub target: String,
    pub result: String,
    pub ok: bool,
    /// What undo needs (a file's previous text, a window's previous place). None: it cannot be undone.
    pub undo: Option<Value>,
    pub undone: bool,
}

impl Memory {
    pub fn record_action(
        &self,
        tool: &str,
        target: &str,
        result: &str,
        ok: bool,
        undo: Option<&Value>,
    ) -> Result<i64> {
        let db = self.db();
        db.execute(
            "INSERT INTO activity(at, tool, target, result, ok, undo_json) VALUES (?1,?2,?3,?4,?5,?6)",
            params![
                now(),
                tool,
                target,
                result,
                ok,
                undo.map(Value::to_string)
            ],
        )?;
        let id = db.last_insert_rowid();
        db.execute(
            "DELETE FROM activity WHERE id <= ?1 - ?2",
            params![id, KEEP],
        )?;
        Ok(id)
    }

    /// Newest first.
    pub fn actions(&self, limit: usize) -> Result<Vec<Action>> {
        let db = self.db();
        let mut st = db.prepare(
            "SELECT id, at, tool, target, result, ok, undo_json, undone FROM activity ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = st.query_map([limit as i64], |r| {
            Ok(Action {
                id: r.get(0)?,
                at: r.get(1)?,
                tool: r.get(2)?,
                target: r.get(3)?,
                result: r.get(4)?,
                ok: r.get(5)?,
                undo: r
                    .get::<_, Option<String>>(6)?
                    .and_then(|s| serde_json::from_str(&s).ok()),
                undone: r.get(7)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn mark_undone(&self, id: i64) -> Result<()> {
        self.db()
            .execute("UPDATE activity SET undone = 1 WHERE id = ?1", [id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::Memory;
    use serde_json::json;

    #[test]
    fn actions_are_kept_newest_first_and_wiped_with_everything() {
        let m = Memory::open_in_memory().unwrap();
        m.record_action(
            "write_file",
            "/a/notes.md",
            "Wrote 5 bytes",
            true,
            Some(&json!({"previous": "old"})),
        )
        .unwrap();
        let id = m
            .record_action("send_imessage", "Sara", "Sent", true, None)
            .unwrap();
        let a = m.actions(10).unwrap();
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].tool, "send_imessage");
        assert_eq!(a[0].undo, None);
        assert_eq!(a[1].undo, Some(json!({"previous": "old"})));
        m.mark_undone(id).unwrap();
        assert!(m.actions(1).unwrap()[0].undone);
        m.wipe().unwrap();
        assert!(m.actions(10).unwrap().is_empty());
    }
}
