//! Spoken progress: when a turn with tool calls runs longer than a few seconds, one
//! short line says what Markon is doing, at most one every several seconds and a few per turn. Never
//! over the user, never once the reply has started, and never while the caller says to stay quiet
//! (media starting, a card-only delivery, Markon already talking).
use crate::{AgentEvent, AgentState};
use std::time::{Duration, Instant};

/// No line for turns that finish quickly.
pub const FIRST_AFTER: Duration = Duration::from_secs(4);
/// At most one line this often.
pub const GAP: Duration = Duration::from_secs(10);
pub const MAX_LINES: usize = 3;

#[derive(Debug, Default)]
pub struct Progress {
    started: Option<Instant>,
    tool: Option<String>,
    last: Option<Instant>,
    said: usize,
    replying: bool,
}

/// What Markon is doing, in a few words, by the tool it is running.
pub fn line_for(tool: &str) -> &'static str {
    match tool {
        "what_did_i_miss" => "Checking your messages.",
        "fetch_page" | "web_fetch" | "web_search" => "Reading up on it.",
        t if t.starts_with("browser") => "Working in the browser.",
        "search_my_stuff" | "research" | "who_is" => "Looking through your things.",
        "read_app" | "click_in_app" | "type_in_app" | "press_keys" => "Working in the app.",
        "look_at_screen" => "Looking at your screen.",
        "run_shortcut" => "Running your shortcut.",
        "run_command" | "run_js" => "Running it now.",
        _ => "Still working on it.",
    }
}

impl Progress {
    pub fn on_event(&mut self, e: &AgentEvent, now: Instant) {
        match e {
            AgentEvent::StateChanged(AgentState::Thinking) if self.started.is_none() => {
                self.started = Some(now)
            }
            AgentEvent::StateChanged(AgentState::Idle)
            | AgentEvent::TurnFinished(_)
            | AgentEvent::TurnCancelled { .. } => *self = Self::default(),
            AgentEvent::ToolCallStarted { name, .. } => self.tool = Some(name.clone()),
            AgentEvent::TextDelta { .. } => self.replying = true,
            _ => {}
        }
    }

    /// The line to say now, if one is due. `quiet` is the caller's veto: the user is talking, media is
    /// starting, the turn is card-only, or Markon is already speaking.
    pub fn due(&mut self, now: Instant, quiet: bool) -> Option<&'static str> {
        let started = self.started?;
        let tool = self.tool.as_deref()?;
        if quiet
            || self.replying
            || self.said >= MAX_LINES
            || now.duration_since(started) < FIRST_AFTER
            || self.last.is_some_and(|l| now.duration_since(l) < GAP)
        {
            return None;
        }
        let line = if self.said == 0 {
            line_for(tool)
        } else {
            "Still on it."
        };
        self.said += 1;
        self.last = Some(now);
        Some(line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool(name: &str) -> AgentEvent {
        AgentEvent::ToolCallStarted {
            turn_id: 1,
            id: "1".into(),
            name: name.into(),
            input: json!({}),
        }
    }

    fn started(t0: Instant) -> Progress {
        let mut p = Progress::default();
        p.on_event(&AgentEvent::StateChanged(AgentState::Thinking), t0);
        p
    }

    #[test]
    fn a_long_turn_with_tools_gets_one_line_every_few_seconds() {
        let t0 = Instant::now();
        let s = |n: u64| t0 + Duration::from_secs(n);
        let mut p = started(t0);
        assert_eq!(p.due(s(5), false), None, "no tool call yet");
        p.on_event(&tool("what_did_i_miss"), s(1));
        assert_eq!(p.due(s(2), false), None, "too soon");
        assert_eq!(p.due(s(5), false), Some("Checking your messages."));
        assert_eq!(p.due(s(9), false), None, "one every ten seconds");
        assert_eq!(p.due(s(15), false), Some("Still on it."));
        assert_eq!(p.due(s(25), false), Some("Still on it."));
        assert_eq!(p.due(s(40), false), None, "three at most");
    }

    #[test]
    fn never_over_the_user_media_or_the_reply() {
        let t0 = Instant::now();
        let mut p = started(t0);
        p.on_event(&tool("look_at_screen"), t0);
        let later = t0 + Duration::from_secs(6);
        assert_eq!(p.due(later, true), None, "the caller said quiet");
        p.on_event(
            &AgentEvent::TextDelta {
                turn_id: 1,
                text: "It's".into(),
            },
            later,
        );
        assert_eq!(p.due(later, false), None, "the reply has started");
    }

    #[test]
    fn each_turn_starts_fresh() {
        let t0 = Instant::now();
        let mut p = started(t0);
        p.on_event(&tool("run_shortcut"), t0);
        p.on_event(&AgentEvent::StateChanged(AgentState::Idle), t0);
        let t1 = t0 + Duration::from_secs(60);
        p.on_event(&AgentEvent::StateChanged(AgentState::Thinking), t1);
        p.on_event(&tool("read_app"), t1);
        assert_eq!(p.due(t1 + Duration::from_secs(1), false), None);
        assert_eq!(
            p.due(t1 + Duration::from_secs(5), false),
            Some("Working in the app.")
        );
    }
}
