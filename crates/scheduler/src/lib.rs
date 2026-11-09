//! Scheduler: agents that run on a cron line without a prompt. Schedules live in the
//! memory database, so they survive restarts; a run missed while asleep happens once on wake.
use chrono::{DateTime, Local, TimeZone};
use croner::Cron;
use markon_memory::params;
use markon_memory::{Memory, Source};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::sync::Arc;

/// What a scheduled agent does: a prompt and the tools it may use.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentSpec {
    pub prompt: String,
    pub tools: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Schedule {
    pub id: i64,
    pub name: String,
    pub cron: String,
    pub agent: AgentSpec,
    pub enabled: bool,
    pub last_run: Option<i64>,
    pub next_run: Option<i64>,
}

/// A scheduled agent never speaks while the user is in a call or sharing the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Delivery {
    Speak,
    CardOnly,
}

impl Delivery {
    pub fn for_busy(busy: bool) -> Self {
        if busy {
            Delivery::CardOnly
        } else {
            Delivery::Speak
        }
    }
}

pub const MORNING_BRIEFING: &str = "Morning briefing";

pub fn morning_briefing() -> AgentSpec {
    AgentSpec {
        prompt: "Give me my morning briefing in under a minute of speech: today's calendar and an inbox summary if \
                 you have tools for them (say plainly if you don't), and today's weather (fetch \
                 https://wttr.in/?format=3)."
            .into(),
        tools: vec!["fetch_page".into()],
    }
}

fn next_after(cron: &str, after: DateTime<Local>) -> Result<i64, String> {
    let c = Cron::from_str(cron).map_err(|e| format!("bad cron line `{cron}`: {e}"))?;
    c.find_next_occurrence(&after, false)
        .map(|t| t.timestamp_millis())
        .map_err(|e| e.to_string())
}

pub struct Scheduler {
    mem: Arc<Memory>,
}

impl Scheduler {
    pub fn new(mem: Arc<Memory>) -> Self {
        Self { mem }
    }

    /// Add a schedule if it is not there yet. An existing one keeps its state.
    pub fn ensure(
        &self,
        name: &str,
        cron: &str,
        agent: &AgentSpec,
        now: DateTime<Local>,
    ) -> Result<(), String> {
        let next = next_after(cron, now)?;
        let agent = serde_json::to_string(agent).map_err(|e| e.to_string())?;
        self.mem
            .with(|db| {
                db.execute(
                    "INSERT INTO schedules(name, cron, agent, enabled, next_run) VALUES (?1,?2,?3,1,?4) ON CONFLICT(name) DO NOTHING",
                    params![name, cron, agent, next],
                )
            })
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    pub fn list(&self) -> Result<Vec<Schedule>, String> {
        self.mem
            .with(|db| {
                let mut st = db.prepare("SELECT id, name, cron, agent, enabled, last_run, next_run FROM schedules ORDER BY id")?;
                let rows = st.query_map([], |r| {
                    let agent: String = r.get(3)?;
                    Ok(Schedule {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        cron: r.get(2)?,
                        agent: serde_json::from_str(&agent).unwrap_or(AgentSpec { prompt: String::new(), tools: vec![] }),
                        enabled: r.get(4)?,
                        last_run: r.get(5)?,
                        next_run: r.get(6)?,
                    })
                })?;
                rows.collect()
            })
            .map_err(|e| e.to_string())
    }

    pub fn set_enabled(&self, name: &str, enabled: bool) -> Result<(), String> {
        self.mem
            .with(|db| {
                db.execute(
                    "UPDATE schedules SET enabled=?2 WHERE name=?1",
                    params![name, enabled],
                )
            })
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Schedules due at `now`. Each is marked as run and moved to its next time after `now`,
    /// so any number of missed occurrences collapse into this one run.
    pub fn take_due(&self, now: DateTime<Local>) -> Result<Vec<Schedule>, String> {
        let now_ms = now.timestamp_millis();
        let due: Vec<Schedule> = self
            .list()?
            .into_iter()
            .filter(|s| s.enabled && s.next_run.is_some_and(|n| n <= now_ms))
            .collect();
        for s in &due {
            let next = next_after(&s.cron, now)?;
            self.mem
                .with(|db| {
                    db.execute(
                        "UPDATE schedules SET last_run=?2, next_run=?3 WHERE id=?1",
                        params![s.id, now_ms, next],
                    )
                })
                .map_err(|e| e.to_string())?;
        }
        Ok(due)
    }

    /// Every run is a normal conversation in History.
    pub fn open_run(&self, s: &Schedule) -> Result<i64, String> {
        let id = self
            .mem
            .start_conversation(Source::Schedule)
            .map_err(|e| e.to_string())?;
        self.mem.set_title(id, &s.name).map_err(|e| e.to_string())?;
        Ok(id)
    }
}

/// Local time from epoch milliseconds (for display).
pub fn local(ms: i64) -> Option<DateTime<Local>> {
    Local.timestamp_millis_opt(ms).single()
}
