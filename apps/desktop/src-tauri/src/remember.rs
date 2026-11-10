//! Recall in the prompt, the model's `remember` tool, explicit "remember that" / "forget that".
use markon_agent::{PromptSection, ToolCall, ToolRuntime, UserTurn};
use markon_memory::Memory;
use markon_provider::{BoxFuture, ToolSpec};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Facts about the user always in the prompt (newest first), plus the ones relevant to this turn.
const PROFILE: usize = 20;
const RELEVANT: usize = 8;

/// Puts what Markon knows about the user into the system prompt for each turn.
pub struct RecallSection(pub Arc<Memory>);

impl PromptSection for RecallSection {
    fn render(&self, turn: &UserTurn) -> Option<String> {
        let mut facts = self.0.latest_facts(PROFILE).unwrap_or_default();
        for f in self.0.recall(&turn.text, RELEVANT).unwrap_or_default() {
            if !facts.iter().any(|g| g.id == f.id) {
                facts.push(f);
            }
        }
        if facts.is_empty() {
            return None;
        }
        let lines: Vec<String> = facts.iter().map(|f| format!("- {}", f.statement)).collect();
        Some(format!(
            "What you know about the user from earlier conversations (use it naturally, never recite it):\n{}",
            lines.join("\n")
        ))
    }
}

pub const REMEMBER: &str = "remember";

/// Facts the model saved during a turn, by turn id. Written with the turn; dropped if it is cancelled.
pub type PendingFacts = Arc<Mutex<HashMap<u64, Vec<(String, String)>>>>;

/// The tool runtime plus `remember`, which lets Markon learn lasting things about the user.
pub struct WithMemory<T> {
    pub inner: T,
    pub pending: PendingFacts,
}

impl<T: ToolRuntime> ToolRuntime for WithMemory<T> {
    fn specs(&self) -> Vec<ToolSpec> {
        let mut v = self.inner.specs();
        v.push(ToolSpec {
            name: REMEMBER.into(),
            description: "Save one lasting fact about the user so you know it in future conversations: \
                their name, where they live, work, the people in their life, preferences, routines, goals. \
                Call it quietly whenever they reveal one; don't mention that you saved it. Skip one-off \
                requests, things you only guessed, and secrets like passwords or card numbers."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "statement": {"type": "string", "description": "The fact as a short third-person sentence, e.g. \"The user's sister is Ana.\""},
                    "subject": {"type": "string", "description": "A short key; a newer fact with the same key replaces the old one, e.g. \"sister\", \"home city\"."}
                },
                "required": ["statement", "subject"]
            }),
        });
        v
    }

    fn call(&self, call: ToolCall) -> BoxFuture<'static, Result<String, String>> {
        if call.name != REMEMBER {
            return self.inner.call(call);
        }
        let r = remember_call(&self.pending, &call);
        Box::pin(async move { r })
    }
}

pub fn remember_call(pending: &PendingFacts, call: &ToolCall) -> Result<String, String> {
    let statement = call.input["statement"].as_str().unwrap_or("").trim();
    if statement.is_empty() || statement.chars().count() > 300 {
        return Err("statement must be one short sentence".into());
    }
    let subject = call.input["subject"]
        .as_str()
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| subject_of(statement));
    pending
        .lock()
        .expect("pending")
        .entry(call.turn_id)
        .or_default()
        .push((subject, statement.to_string()));
    Ok("Saved.".into())
}

#[derive(Debug, PartialEq, Eq)]
pub enum MemoryCommand {
    /// subject, statement
    Remember(String, String),
    ForgetThat,
}

/// Facts are written only from explicit user statements: "remember that ...".
pub fn parse(text: &str) -> Option<MemoryCommand> {
    let t = text.trim().trim_end_matches(['.', '!']);
    let lower = t.to_lowercase();
    let lower = lower
        .strip_prefix("hey markon, ")
        .or_else(|| lower.strip_prefix("hey markon "))
        .unwrap_or(&lower)
        .to_string();
    if ["forget that", "forget it", "forget what i just said"].contains(&lower.as_str()) {
        return Some(MemoryCommand::ForgetThat);
    }
    let start = t.len() - lower.len();
    for prefix in ["remember that ", "please remember that ", "remember "] {
        if lower.starts_with(prefix) {
            let statement = t[start + prefix.len()..].trim().to_string();
            if statement.split_whitespace().count() < 2 {
                return None;
            }
            // Subject: the first words, so a later "remember that my X is ..." supersedes it.
            let subject = subject_of(&statement);
            let mut s = statement;
            if let Some(f) = s.get_mut(0..1) {
                f.make_ascii_uppercase();
            }
            return Some(MemoryCommand::Remember(subject, s));
        }
    }
    None
}

/// "my sister is called Ana" → "sister"; "I prefer tea" → "i prefer"; otherwise the first two words.
pub fn subject_of(statement: &str) -> String {
    let words: Vec<String> = statement
        .split_whitespace()
        .map(|w| {
            w.to_lowercase()
                .trim_matches(|c: char| !c.is_alphanumeric())
                .to_string()
        })
        .collect();
    let verbs = [
        "is", "are", "was", "lives", "likes", "works", "called", "has",
    ];
    if words.first().map(String::as_str) == Some("my")
        && let Some(i) = words.iter().position(|w| verbs.contains(&w.as_str()))
        && i > 1
    {
        return words[1..i].join(" ");
    }
    words.iter().take(2).cloned().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory_with(facts: &[(&str, &str)]) -> Arc<Memory> {
        let m = Arc::new(Memory::open_in_memory().unwrap());
        let c = m.start_conversation(markon_memory::Source::Voice).unwrap();
        let t = m.add_turn(c, "user", "x", 0, 0, "fake", "fake").unwrap();
        for (subject, statement) in facts {
            m.add_fact(
                subject,
                statement,
                t,
                1.0,
                markon_memory::FactOrigin::UserStated,
            )
            .unwrap();
        }
        m
    }

    #[test]
    fn what_markon_knows_about_the_user_is_always_in_the_prompt() {
        let m = memory_with(&[
            ("name", "The user's name is Dev"),
            ("coffee", "Likes flat white"),
        ]);
        let text = RecallSection(m)
            .render(&UserTurn::text("what's the weather"))
            .unwrap();
        assert!(text.contains("The user's name is Dev"), "{text}");
        assert!(text.contains("Likes flat white"));
    }

    #[test]
    fn the_remember_tool_holds_facts_until_the_turn_is_saved() {
        let pending = PendingFacts::default();
        let fut = remember_call(
            &pending,
            &markon_agent::ToolCall {
                id: "r1".into(),
                name: REMEMBER.into(),
                input: serde_json::json!({"statement": "The user's name is Dev", "subject": "Name"}),
                turn_id: 7,
            },
        );
        assert!(fut.is_ok());
        assert_eq!(
            pending.lock().unwrap().get(&7).cloned().unwrap(),
            vec![("name".to_string(), "The user's name is Dev".to_string())]
        );
        let empty = remember_call(
            &pending,
            &markon_agent::ToolCall {
                id: "r2".into(),
                name: REMEMBER.into(),
                input: serde_json::json!({"statement": "  "}),
                turn_id: 7,
            },
        );
        assert!(empty.is_err());
    }

    #[test]
    fn explicit_statements_become_facts() {
        assert_eq!(
            parse("Remember that my sister is called Ana."),
            Some(MemoryCommand::Remember(
                "sister".into(),
                "My sister is called Ana".into()
            ))
        );
        assert_eq!(
            parse("Hey Markon, forget that."),
            Some(MemoryCommand::ForgetThat)
        );
        assert_eq!(parse("Do you remember the meeting?"), None);
        assert_eq!(parse("remember"), None);
    }

    #[test]
    fn subjects_line_up_so_newer_facts_supersede() {
        assert_eq!(
            subject_of("my coffee order is a flat white"),
            "coffee order"
        );
        assert_eq!(subject_of("My coffee order is espresso"), "coffee order");
        assert_eq!(subject_of("I prefer short answers"), "i prefer");
    }
}
