//! Spoken window and canvas commands handled locally, without a model round trip.
use crate::core::Core;
use std::sync::LazyLock;
use tauri::Emitter;

pub const EV_COMMAND: &str = "markon://command";

/// One row of `voice_commands.json`: any of `phrases` runs `command` with `arg`. A `strict` command
/// (quit) matches only as written; the rest also match a close mishearing.
#[derive(serde::Deserialize)]
struct Entry {
    command: String,
    #[serde(default)]
    arg: String,
    phrases: Vec<String>,
    #[serde(default)]
    strict: bool,
}

/// Commands by phrase. Add a phrase or a command by editing `voice_commands.json`.
static TABLE: LazyLock<Vec<Entry>> = LazyLock::new(|| {
    let mut t: Vec<Entry> = serde_json::from_str(include_str!("voice_commands.json"))
        .expect("voice_commands.json is valid");
    for e in &mut t {
        for p in &mut e.phrases {
            *p = normalise(p);
        }
    }
    t
});

fn normalise(text: &str) -> String {
    let t: String = text
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect();
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Command words for whisper to expect, so "hacker mode" is not heard as "Hi, Karamod".
/// The first phrase of each command: listing them all would make whisper invent them in noise.
pub fn vocabulary() -> String {
    let mut words: Vec<&str> = Vec::new();
    for e in TABLE.iter() {
        if let Some(p) = e.phrases.first()
            && !words.contains(&p.as_str())
        {
            words.push(p);
        }
    }
    format!("{}.", words.join(", "))
}

/// Words people wrap around a command ("Stop. Go to standby.", "okay hacker mode please").
const LEAD: [&str; 9] = [
    "hey markon ",
    "hey markan ",
    "markon ",
    "markan ",
    "stop ",
    "okay ",
    "ok ",
    "please ",
    "now ",
];
const TAIL: [&str; 4] = [" please", " now", " thanks", " thank you"];
/// Verbs in front of a command: "go to standby", "switch to hacker mode", "change normal mode".
/// Tried one at a time, so a phrase that starts with one ("go away") still matches as written.
const VERB: [&str; 8] = [
    "go to ",
    "switch to ",
    "change to ",
    "back to ",
    "turn on ",
    "change ",
    "switch ",
    "go ",
];

/// What a phrase asks for, if it is a local command. A repeated command ("Stand by. Go to stand
/// by.") counts when every sentence asks for the same thing.
pub fn parse(text: &str) -> Option<(&'static str, String)> {
    if let Some(hit) = parse_one(text) {
        return Some(hit);
    }
    let mut parts = text
        .split(['.', '!', '?', ','])
        .filter(|p| !normalise(p).is_empty());
    let first = parse_one(parts.next()?)?;
    parts
        .all(|p| parse_one(p).as_ref() == Some(&first))
        .then_some(first)
}

fn parse_one(text: &str) -> Option<(&'static str, String)> {
    let mut t = normalise(text);
    while let Some(rest) = LEAD.iter().find_map(|w| t.strip_prefix(w)) {
        t = rest.to_string();
    }
    while let Some(rest) = TAIL.iter().find_map(|w| t.strip_suffix(w)) {
        t = rest.to_string();
    }
    let mut c = t.as_str();
    loop {
        if let Some(e) = TABLE.iter().find(|e| e.phrases.iter().any(|p| p == c)) {
            return Some((e.command.as_str(), e.arg.clone()));
        }
        if let Some(e) = closest(c) {
            return Some((e.command.as_str(), e.arg.clone()));
        }
        match VERB.iter().find_map(|v| c.strip_prefix(v)) {
            Some(rest) => c = rest,
            None => break,
        }
    }
    if let Some(rest) = t.strip_prefix("bring back ") {
        return Some(("bring_back", rest.to_string()));
    }
    if let Some(rest) = t.strip_prefix("focus on ") {
        return Some(("focus", rest.to_string()));
    }
    None
}

/// Letters only: "stand-by", "stand by" and "standby" are the same word to whisper.
fn squash(t: &str) -> Vec<char> {
    t.chars().filter(|c| c.is_alphanumeric()).collect()
}

fn edits(a: &[char], b: &[char]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            cur.push(
                (prev[j] + usize::from(ca != cb))
                    .min(prev[j + 1] + 1)
                    .min(cur[j] + 1),
            );
        }
        prev = cur;
    }
    prev[b.len()]
}

/// The one command a short utterance is close to: one wrong letter per five, never more words than
/// the phrase plus one, nothing for a strict command, and nothing when two commands are as close.
fn closest(text: &str) -> Option<&'static Entry> {
    let heard = squash(text);
    let words = text.split_whitespace().count();
    let mut best: Option<(usize, &Entry)> = None;
    let mut tie = false;
    for e in TABLE.iter().filter(|e| !e.strict) {
        for p in &e.phrases {
            let want = squash(p);
            if words > p.split_whitespace().count() + 1 {
                continue;
            }
            let d = edits(&heard, &want);
            if d > want.len() / 5 {
                continue;
            }
            match best {
                Some((bd, be)) if d == bd && be.command != e.command => tie = true,
                Some((bd, _)) if d >= bd => {}
                _ => {
                    best = Some((d, e));
                    tie = false;
                }
            }
        }
    }
    best.filter(|_| !tie).map(|(_, e)| e)
}

/// Returns true when the phrase was a command and has been handled.
pub fn handle(core: &Core, text: &str) -> bool {
    let Some((cmd, arg)) = parse(text) else {
        return false;
    };
    if cmd == "hide" {
        core.hide();
    }
    if cmd == "standby" {
        // The caller sets the state to idle once a command is handled.
        core.stop();
        return true;
    }
    if cmd == "bring_back" {
        match core.memory.find_conversation(&arg) {
            Ok(Some(id)) => {
                let _ = core.open_conversation(id);
            }
            _ => core.say(&format!("I couldn't find anything about {arg}.")),
        }
        return true;
    }
    // Quit asks for confirmation in the UI; the rest are canvas actions the UI performs.
    let _ = core.app.emit(
        EV_COMMAND,
        serde_json::json!({ "command": cmd, "arg": arg }),
    );
    true
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn recognises_window_and_canvas_commands() {
        assert_eq!(parse("Hide."), Some(("hide", String::new())));
        assert_eq!(parse("Hey Markon, quit."), Some(("quit", String::new())));
        assert_eq!(parse("Clear the screen!"), Some(("clear", String::new())));
        assert_eq!(
            parse("Bring back the VR report"),
            Some(("bring_back", "the vr report".into()))
        );
        assert_eq!(
            parse("Focus on shipments"),
            Some(("focus", "shipments".into()))
        );
    }

    #[test]
    fn hacker_mode_switches_the_theme() {
        assert_eq!(parse("Hacker mode!"), Some(("theme", "ares".into())));
        assert_eq!(
            parse("Markon, change hacker mode."),
            Some(("theme", "ares".into()))
        );
        assert_eq!(parse("Normal mode"), Some(("theme", "tron".into())));
        assert_eq!(parse("Go to normal mode."), Some(("theme", "tron".into())));
        assert_eq!(
            parse("Switch to hacker mode"),
            Some(("theme", "ares".into()))
        );
        assert_eq!(parse("Go away"), Some(("hide", String::new())));
        assert_eq!(
            parse("Okay, hacker mode please."),
            Some(("theme", "ares".into()))
        );
    }

    #[test]
    fn standby_as_heard_by_whisper() {
        for heard in [
            "Go on standby",
            "Back to standby.",
            "Go to Stand B.",
            "Markon, stop, go to standby.",
            "Standby.",
            "Stand by.",
            "Go to standby.",
            "Stop. Go to stand-by.",
            "Hey Markon, standby",
            "Stand by. Go to Stand by.",
        ] {
            assert_eq!(parse(heard), Some(("standby", String::new())), "{heard}");
        }
    }

    /// Commands match by closeness, so whisper's mishearings need no phrase of their own.
    #[test]
    fn near_misses_match_without_their_own_phrase() {
        for p in super::TABLE.iter().flat_map(|e| &e.phrases) {
            assert!(!["stand b", "stand bye"].contains(&p.as_str()), "{p}");
        }
        for heard in ["Stand bye.", "Go to stand b", "Standbye", "Stand buy."] {
            assert_eq!(parse(heard), Some(("standby", String::new())), "{heard}");
        }
        assert_eq!(parse("Hacker mood."), Some(("theme", "ares".into())));
        assert_eq!(parse("Setting."), Some(("settings", String::new())));
        assert_eq!(parse("Clear the scream"), Some(("clear", String::new())));
    }

    #[test]
    fn quit_needs_the_exact_word() {
        assert_eq!(parse("Quit."), Some(("quit", String::new())));
        assert_eq!(parse("Exit"), Some(("quit", String::new())));
        for heard in ["Quiet.", "Quits", "Exits", "Quit Marco", "Edit"] {
            assert_eq!(parse(heard), None, "{heard}");
        }
    }

    #[test]
    fn a_sentence_that_contains_a_command_is_not_one() {
        for heard in [
            "Could you tell me what standby mode does on a TV?",
            "I want to tidy up my desktop folder later",
            "Make the font in this document bigger",
            "Hide and seek rules",
            "Stand up straight",
            "Settings for my router",
        ] {
            assert_eq!(parse(heard), None, "{heard}");
        }
    }

    #[test]
    fn vocabulary_names_each_command_once() {
        let v = super::vocabulary();
        assert!(v.contains("hacker mode") && v.contains("standby"), "{v}");
        assert!(!v.contains("change to hacker mode"), "{v}");
    }

    #[test]
    fn every_phrase_maps_to_one_command() {
        let mut seen = std::collections::HashSet::new();
        for e in super::TABLE.iter() {
            assert!(!e.phrases.is_empty(), "{} has no phrases", e.command);
            for p in &e.phrases {
                assert!(seen.insert(p.clone()), "phrase used twice: {p}");
            }
        }
    }

    #[test]
    fn ordinary_requests_go_to_the_model() {
        assert_eq!(
            parse("Hide the files I opened yesterday from the report"),
            None
        );
        assert_eq!(parse("How do I quit smoking?"), None);
        assert_eq!(parse("What is hacker mode in Linux?"), None);
        assert_eq!(parse("Now tell me about standby power"), None);
        assert_eq!(parse("Stand by. Then tell me the weather."), None);
    }
}
