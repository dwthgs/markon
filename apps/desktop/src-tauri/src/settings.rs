//! `~/.markon/settings.json`: everything except secrets, which live in the keychain.
use markon_audio::Voice;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Route {
    Anthropic,
    OpenRouter,
    Relay,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Chosen once at first launch. None until onboarding picks it.
    pub voice: Option<Voice>,
    pub route: Route,
    /// Model id for the route; None = the route's default.
    pub model: Option<String>,
    /// Push-to-talk: hold to talk, release to send.
    pub hotkey: String,
    /// Folders Markon may read (and write after asking once).
    pub approved_folders: Vec<PathBuf>,
    pub onboarded: bool,
    pub reduced_motion: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            voice: None,
            route: Route::Anthropic,
            model: None,
            hotkey: "Alt+Space".into(),
            approved_folders: Vec::new(),
            onboarded: false,
            reduced_motion: false,
        }
    }
}

impl Settings {
    pub fn path(dir: &Path) -> PathBuf {
        dir.join("settings.json")
    }

    /// A missing or unreadable file gives defaults; a corrupt one is kept aside, not overwritten silently.
    pub fn load(dir: &Path) -> Self {
        let p = Self::path(dir);
        match std::fs::read_to_string(&p) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
                eprintln!("[settings] unreadable ({e}); keeping a copy and using defaults");
                let _ = std::fs::copy(&p, p.with_extension("json.bad"));
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    /// Atomic write: temp file then rename.
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let p = Self::path(dir);
        let tmp = p.with_extension("json.tmp");
        std::fs::write(
            &tmp,
            serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?,
        )?;
        std::fs::rename(tmp, p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_survives_corruption() {
        let dir = std::env::temp_dir().join(format!("markon-settings-{}", std::process::id()));
        let mut s = Settings {
            voice: Some(Voice::Markan),
            ..Default::default()
        };
        s.approved_folders.push("/tmp/x".into());
        s.save(&dir).unwrap();
        assert_eq!(Settings::load(&dir), s);
        std::fs::write(Settings::path(&dir), "{not json").unwrap();
        assert_eq!(Settings::load(&dir), Settings::default());
        assert!(dir.join("settings.json.bad").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn secrets_are_never_part_of_settings() {
        let json = serde_json::to_string(&Settings::default())
            .unwrap()
            .to_lowercase();
        assert!(!json.contains("key") || json.contains("hotkey"));
        assert!(!json.contains("token") && !json.contains("secret"));
    }
}
