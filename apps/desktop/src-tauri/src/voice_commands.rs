//! Spoken window and canvas commands handled locally, without a model round trip.
use crate::core::Core;
use tauri::Emitter;

pub const EV_COMMAND: &str = "markon://command";

/// What a phrase asks for, if it is a local command.
pub fn parse(text: &str) -> Option<(&'static str, String)> {
    let t: String = text
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect();
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    let t = t
        .strip_prefix("hey markon ")
        .or_else(|| t.strip_prefix("hey markan "))
        .unwrap_or(&t)
        .to_string();
    let t = t
        .strip_prefix("markon ")
        .or_else(|| t.strip_prefix("markan "))
        .unwrap_or(&t)
        .to_string();
    let exact = |words: &[&str]| words.contains(&t.as_str());
    if exact(&["hide", "hide yourself", "go away", "hide the window"]) {
        return Some(("hide", String::new()));
    }
    if exact(&["quit", "quit markon", "quit markan", "exit"]) {
        return Some(("quit", String::new()));
    }
    if exact(&["settings", "open settings", "show settings"]) {
        return Some(("settings", String::new()));
    }
    if exact(&["tidy up", "tidy"]) {
        return Some(("tidy", String::new()));
    }
    if exact(&[
        "clear the screen",
        "clear screen",
        "clear view",
        "clear the view",
    ]) {
        return Some(("clear", String::new()));
    }
    if exact(&["bigger", "make it bigger"]) {
        return Some(("bigger", String::new()));
    }
    if let Some(rest) = t.strip_prefix("bring back ") {
        return Some(("bring_back", rest.to_string()));
    }
    if let Some(rest) = t.strip_prefix("focus on ") {
        return Some(("focus", rest.to_string()));
    }
    None
}

/// Returns true when the phrase was a command and has been handled.
pub fn handle(core: &Core, text: &str) -> bool {
    let Some((cmd, arg)) = parse(text) else {
        return false;
    };
    if cmd == "hide" {
        core.hide();
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
    fn ordinary_requests_go_to_the_model() {
        assert_eq!(
            parse("Hide the files I opened yesterday from the report"),
            None
        );
        assert_eq!(parse("How do I quit smoking?"), None);
    }
}
