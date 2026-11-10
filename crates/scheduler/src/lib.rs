//! Scheduler: agents that run on a cron line without a prompt. Schedules live in the
//! memory database, so they survive restarts; a run missed while asleep happens once on wake.
//! Agents are files; reminders and meeting prep run from the same loop.
pub mod files;
pub mod reminders;

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
    #[serde(default)]
    pub max_rounds: Option<usize>,
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

/// Names of the schedules that came from agent files at the last sync.
const FILE_AGENTS: &str = "scheduler.file_agents";

/// The agents that ship with Markon, written to `~/.markon/agents` once.
pub const BUNDLED: &[(&str, &str)] = &[
    ("morning-briefing.toml", include_str!("../agents/morning-briefing.toml")),
    ("meeting-prep.toml", include_str!("../agents/meeting-prep.toml")),
];

/// Events that should get a meeting prep card now: starting in 5 to 11 minutes, with other people,
/// not prepared yet. `(id, start_ms, has_attendees)`.
pub fn meeting_prep_due(events: &[(String, i64, bool)], now_ms: i64, done: &[String]) -> Vec<String> {
    events
        .iter()
        .filter(|(id, start, people)| {
            *people && (now_ms + 5 * 60_000..=now_ms + 11 * 60_000).contains(start) && !done.contains(id)
        })
        .map(|(id, ..)| id.clone())
        .collect()
}

/// Fill `{title}`, `{start}` and `{attendees}` in the meeting prep prompt.
pub fn fill(template: &str, title: &str, start: &str, attendees: &str) -> String {
    template.replace("{title}", title).replace("{start}", start).replace("{attendees}", attendees)
}


pub(crate) fn next_after(cron: &str, after: DateTime<Local>) -> Result<i64, String> {
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
                        agent: serde_json::from_str(&agent).unwrap_or(AgentSpec { prompt: String::new(), tools: vec![], max_rounds: None }),
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

    /// Make the schedules match the agent files: add new ones, update changed ones, drop ones whose
    /// file is gone. Rows that never came from a file (none today) are left alone.
    pub fn sync_files(&self, agents: &[files::AgentFile], now: DateTime<Local>) -> Result<(), String> {
        let from_files: Vec<String> = self
            .mem
            .preference(FILE_AGENTS)
            .ok()
            .flatten()
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
        let existing = self.list()?;
        let mut names = Vec::new();
        for f in agents {
            let Some(cron) = &f.schedule else { continue };
            names.push(f.name.clone());
            let spec = serde_json::to_string(&f.spec()).map_err(|e| e.to_string())?;
            match existing.iter().find(|s| s.name == f.name) {
                Some(s) if s.cron == *cron && s.agent == f.spec() && s.enabled == f.enabled => {}
                Some(s) => {
                    let next = if s.cron == *cron { s.next_run } else { Some(next_after(cron, now)?) };
                    self.mem
                        .with(|db| {
                            db.execute(
                                "UPDATE schedules SET cron=?2, agent=?3, enabled=?4, next_run=?5 WHERE id=?1",
                                params![s.id, cron, spec, f.enabled, next],
                            )
                        })
                        .map_err(|e| e.to_string())?;
                }
                None => {
                    let next = next_after(cron, now)?;
                    self.mem
                        .with(|db| {
                            db.execute(
                                "INSERT INTO schedules(name, cron, agent, enabled, next_run) VALUES (?1,?2,?3,?4,?5)",
                                params![f.name, cron, spec, f.enabled, next],
                            )
                        })
                        .map_err(|e| e.to_string())?;
                }
            }
        }
        for gone in from_files.iter().filter(|n| !names.contains(n)) {
            self.mem
                .with(|db| db.execute("DELETE FROM schedules WHERE name=?1", params![gone]))
                .map_err(|e| e.to_string())?;
        }
        self.mem.set_preference(FILE_AGENTS, &serde_json::json!(names)).map_err(|e| e.to_string())
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
