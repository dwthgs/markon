//! Reminders: "remind me at three to call Sara". One row per reminder; when it is due
//! Markon says it, or posts a card during a call.
use chrono::{DateTime, Duration, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone};
use markon_agent::{ToolCall, ToolRuntime};
use markon_memory::{Memory, params};
use markon_provider::{BoxFuture, ToolSpec};
use serde_json::json;
use std::sync::Arc;

pub const REMIND_ME: &str = "remind_me";

pub struct Reminders {
    mem: Arc<Memory>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Reminder {
    pub id: i64,
    pub text: String,
    pub due_ms: i64,
}

impl Reminders {
    pub fn new(mem: Arc<Memory>) -> Self {
        let _ = mem.with(|db| {
            db.execute_batch(
                "CREATE TABLE IF NOT EXISTS reminders (id INTEGER PRIMARY KEY, text TEXT NOT NULL, \
                 due_ms INTEGER NOT NULL, done INTEGER NOT NULL DEFAULT 0)",
            )
        });
        Self { mem }
    }

    pub fn add(&self, text: &str, due: DateTime<Local>) -> Result<i64, String> {
        self.mem
            .with(|db| {
                db.execute("INSERT INTO reminders(text, due_ms) VALUES (?1, ?2)", params![text, due.timestamp_millis()])?;
                Ok(db.last_insert_rowid())
            })
            .map_err(|e| e.to_string())
    }

    /// Reminders due at `now`, marked done so each is delivered once.
    pub fn take_due(&self, now: DateTime<Local>) -> Result<Vec<Reminder>, String> {
        self.mem
            .with(|db| {
                let mut st = db.prepare("SELECT id, text, due_ms FROM reminders WHERE done=0 AND due_ms<=?1 ORDER BY due_ms")?;
                let due: Vec<Reminder> = st
                    .query_map([now.timestamp_millis()], |r| Ok(Reminder { id: r.get(0)?, text: r.get(1)?, due_ms: r.get(2)? }))?
                    .collect::<Result<_, _>>()?;
                for r in &due {
                    db.execute("UPDATE reminders SET done=1 WHERE id=?1", [r.id])?;
                }
                Ok(due)
            })
            .map_err(|e| e.to_string())
    }
}

/// "in 20 minutes", "in 2 hours", "15:00", "3pm", "3:30 pm", "2026-11-11 09:00" or "2026-11-11T09:00".
/// A bare time already past today means tomorrow.
pub fn parse_when(s: &str, now: DateTime<Local>) -> Result<DateTime<Local>, String> {
    let t = s.trim().to_lowercase();
    let bad = || format!("I don't understand the time \"{s}\"; use 15:00, 3pm, \"in 20 minutes\" or 2026-11-11 09:00");
    if let Some(rest) = t.strip_prefix("in ") {
        let mut it = rest.split_whitespace();
        let n: f64 = it.next().map(|n| if n == "a" || n == "an" { "1" } else { n }).and_then(|n| n.parse().ok()).ok_or_else(bad)?;
        let unit = it.next().ok_or_else(bad)?;
        let mins = if unit.starts_with("min") {
            n
        } else if unit.starts_with("hour") || unit == "h" {
            n * 60.0
        } else if unit.starts_with("day") {
            n * 1440.0
        } else {
            return Err(bad());
        };
        if mins <= 0.0 {
            return Err(bad());
        }
        return Ok(now + Duration::seconds((mins * 60.0) as i64));
    }
    for f in ["%Y-%m-%d %H:%M", "%Y-%m-%dT%H:%M", "%Y-%m-%dT%H:%M:%S"] {
        if let Ok(dt) = NaiveDateTime::parse_from_str(&t, f) {
            return Local.from_local_datetime(&dt).single().ok_or_else(bad);
        }
    }
    let (clock, pm, am) = match (t.strip_suffix("pm"), t.strip_suffix("am")) {
        (Some(c), _) => (c.trim().to_string(), true, false),
        (_, Some(c)) => (c.trim().to_string(), false, true),
        _ => (t.clone(), false, false),
    };
    let (h, m) = match clock.split_once(':') {
        Some((h, m)) => (h.parse::<u32>().map_err(|_| bad())?, m.parse::<u32>().map_err(|_| bad())?),
        None => (clock.parse::<u32>().map_err(|_| bad())?, 0),
    };
    let h = match (pm, am, h) {
        (true, _, h) if h < 12 => h + 12,
        (_, true, 12) => 0,
        (false, false, h) if h <= 12 && !clock.contains(':') => return Err(bad()),
        _ => h,
    };
    let time = NaiveTime::from_hms_opt(h, m, 0).ok_or_else(bad)?;
    let today: NaiveDate = now.date_naive();
    let mut at = Local.from_local_datetime(&today.and_time(time)).single().ok_or_else(bad)?;
    if at <= now {
        at += Duration::days(1);
    }
    Ok(at)
}

pub fn spec() -> ToolSpec {
    ToolSpec {
        name: REMIND_ME.into(),
        description: "Set a reminder. Markon says it out loud at that time (or shows a card during a call). \
            `text` is what to remind them of, as you would say it (\"call Sara back\"). `when` is a time today \
            (\"15:00\", \"3pm\"), relative (\"in 20 minutes\") or a date and time (\"2026-11-11 09:00\")."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {"text": {"type": "string"}, "when": {"type": "string"}},
            "required": ["text", "when"],
            "additionalProperties": false
        }),
    }
}

/// The tool runtime plus `remind_me`.
pub struct WithReminders<T> {
    pub inner: T,
    pub reminders: Arc<Reminders>,
}

impl<T: ToolRuntime> ToolRuntime for WithReminders<T> {
    fn specs(&self) -> Vec<ToolSpec> {
        let mut v = self.inner.specs();
        v.push(spec());
        v
    }
    fn call(&self, call: ToolCall) -> BoxFuture<'static, Result<String, String>> {
        if call.name != REMIND_ME {
            return self.inner.call(call);
        }
        let r = (|| {
            let text = call.input["text"].as_str().map(str::trim).filter(|t| !t.is_empty()).ok_or("say what to remind them of")?;
            let at = parse_when(call.input["when"].as_str().unwrap_or(""), Local::now())?;
            self.reminders.add(text, at)?;
            Ok(format!("Reminder set for {}: {text}", at.format("%a %H:%M")))
        })();
        Box::pin(async move { r })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 11, 10, 14, 30, 0).single().unwrap()
    }

    #[test]
    fn understands_the_ways_people_say_a_time() {
        let n = now();
        let hm = |s: &str| parse_when(s, n).map(|t| t.format("%d %H:%M").to_string());
        assert_eq!(hm("in 20 minutes").unwrap(), "10 14:50");
        assert_eq!(hm("in 2 hours").unwrap(), "10 16:30");
        assert_eq!(hm("in an hour").unwrap(), "10 15:30");
        assert_eq!(hm("15:00").unwrap(), "10 15:00");
        assert_eq!(hm("3pm").unwrap(), "10 15:00");
        assert_eq!(hm("3:30 pm").unwrap(), "10 15:30");
        assert_eq!(hm("9am").unwrap(), "11 09:00", "already past today: tomorrow");
        assert_eq!(hm("12am").unwrap(), "11 00:00");
        assert_eq!(hm("2026-11-12 09:00").unwrap(), "12 09:00");
        assert!(hm("3").is_err(), "a bare hour is ambiguous");
        assert!(hm("whenever").is_err());
    }

    #[test]
    fn a_reminder_is_delivered_once_when_due() {
        let r = Reminders::new(Arc::new(Memory::open_in_memory().unwrap()));
        r.add("call Sara back", now() + Duration::minutes(10)).unwrap();
        assert!(r.take_due(now()).unwrap().is_empty());
        let due = r.take_due(now() + Duration::minutes(10)).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].text, "call Sara back");
        assert!(r.take_due(now() + Duration::hours(1)).unwrap().is_empty());
    }
}
