//! The watch agent's rules, with no model call: an item matters when it is from someone on the watch list or has a watch word in it,
//! and it is new since the last look.
use crate::Item;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WatchRules {
    pub enabled: bool,
    pub people: Vec<String>,
    pub words: Vec<String>,
}

impl Default for WatchRules {
    fn default() -> Self {
        Self {
            enabled: false,
            people: Vec::new(),
            words: vec!["urgent".into(), "asap".into(), "emergency".into()],
        }
    }
}

fn has_word(text: &str, word: &str) -> bool {
    let w = word.trim().to_lowercase();
    !w.is_empty()
        && text
            .to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .collect::<Vec<_>>()
            .windows(w.split_whitespace().count().max(1))
            .any(|win| win.join(" ") == w)
}

/// Why this item matters, if it does.
pub fn matters(item: &Item, rules: &WatchRules) -> Option<String> {
    let who = item.who.to_lowercase();
    if let Some(p) = rules
        .people
        .iter()
        .find(|p| !p.trim().is_empty() && who.contains(&p.trim().to_lowercase()))
    {
        return Some(format!("from {p}"));
    }
    rules
        .words
        .iter()
        .find(|w| has_word(&item.text, w))
        .map(|w| format!("says \"{w}\""))
}

/// The items worth speaking up for, and the updated list of seen keys (kept to the newest 500).
pub fn important(
    items: &[Item],
    rules: &WatchRules,
    seen: &[String],
) -> (Vec<(Item, String)>, Vec<String>) {
    let mut seen = seen.to_vec();
    let mut out = Vec::new();
    for i in items {
        let key = format!("{}:{}", i.source, i.id);
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        if let Some(why) = matters(i, rules) {
            out.push((i.clone(), why));
        }
    }
    let n = seen.len();
    if n > 500 {
        seen.drain(..n - 500);
    }
    (out, seen)
}

/// One short sentence to say out loud.
pub fn sentence(item: &Item) -> String {
    let who = if item.who.is_empty() {
        "Someone".to_string()
    } else {
        item.who.split(" <").next().unwrap_or(&item.who).to_string()
    };
    format!("{who} just wrote: {}", crate::clip(&item.text, 120))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, who: &str, text: &str) -> Item {
        Item {
            source: "imessage".into(),
            id: id.into(),
            who: who.into(),
            place: String::new(),
            when_ms: 0,
            text: text.into(),
            link: None,
            unread: true,
        }
    }

    #[test]
    fn people_and_words_on_the_list_matter_once() {
        let rules = WatchRules {
            enabled: true,
            people: vec!["Sara".into()],
            ..Default::default()
        };
        let items = [
            item("1", "Sara Tesfaye", "lunch?"),
            item("2", "Bank", "Your card was used: URGENT, call us"),
            item("3", "Sam", "nothing urgently needed"),
            item("4", "Abel", "see you"),
        ];
        let (hits, seen) = important(&items, &rules, &[]);
        assert_eq!(
            hits.iter().map(|(i, _)| i.id.as_str()).collect::<Vec<_>>(),
            ["1", "2"]
        );
        assert_eq!(hits[0].1, "from Sara");
        assert_eq!(seen.len(), 4);
        let (again, _) = important(&items, &rules, &seen);
        assert!(again.is_empty(), "nothing is announced twice");
        assert_eq!(sentence(&hits[0].0), "Sara Tesfaye just wrote: lunch?");
    }
}
