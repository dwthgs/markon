//! Recall in the prompt, explicit "remember that" / "forget that", and the scheduler loop.
use markon_agent::{PromptSection, UserTurn};
use markon_memory::Memory;
use std::sync::Arc;

/// Puts the most relevant remembered facts into the system prompt for each turn.
pub struct RecallSection(pub Arc<Memory>);

impl PromptSection for RecallSection {
    fn render(&self, turn: &UserTurn) -> Option<String> {
        let facts = self.0.recall(&turn.text, 8).ok()?;
        if facts.is_empty() {
            return None;
        }
        let lines: Vec<String> = facts.iter().map(|f| format!("- {}", f.statement)).collect();
        Some(format!(
            "Things the user told you before (use them when relevant, do not recite them):\n{}",
            lines.join("\n")
        ))
    }
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
