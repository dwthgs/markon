//! A stand-in Mac for tests: records every command as a line and answers from a script, so no
//! test touches real apps, settings or permissions.
use crate::control::{AppCommand, AxCommand, Key, Permission, ShortcutCommand, SystemCommand};
use crate::{NowPlaying, Platform, PlatformError, SpotifyCommand};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Default)]
pub struct FakeMac {
    /// Every command, in order ("shortcuts run Morning", "apps quit Safari").
    pub did: Mutex<Vec<String>>,
    /// Answers by the start of the command line; the longest matching start wins. Unknown → "".
    pub answers: Mutex<HashMap<String, Result<String, String>>>,
    pub granted: Mutex<HashSet<Permission>>,
    pub opened_settings: Mutex<Vec<Permission>>,
}

impl FakeMac {
    pub fn with_permissions(p: &[Permission]) -> Self {
        let f = Self::default();
        f.granted.lock().expect("granted").extend(p.iter().copied());
        f
    }

    pub fn answer(&self, starts: &str, r: Result<&str, &str>) {
        self.answers
            .lock()
            .expect("answers")
            .insert(starts.into(), r.map(str::to_string).map_err(str::to_string));
    }

    pub fn did(&self) -> Vec<String> {
        self.did.lock().expect("did").clone()
    }

    fn reply(&self, line: String) -> Result<String, PlatformError> {
        let r = self
            .answers
            .lock()
            .expect("answers")
            .iter()
            .filter(|(k, _)| line.starts_with(k.as_str()))
            .max_by_key(|(k, _)| k.len())
            .map(|(_, v)| v.clone())
            .unwrap_or(Ok(String::new()));
        self.did.lock().expect("did").push(line);
        r.map_err(PlatformError::Open)
    }
}

impl Platform for FakeMac {
    fn data_dir(&self) -> PathBuf {
        std::env::temp_dir().join("markon-fake")
    }
    fn secret_get(&self, _: &str) -> Result<Option<String>, PlatformError> {
        Ok(None)
    }
    fn secret_set(&self, _: &str, _: &str) -> Result<(), PlatformError> {
        Ok(())
    }
    fn secret_delete(&self, _: &str) -> Result<(), PlatformError> {
        Ok(())
    }
    fn open_url(&self, url: &str) -> Result<(), PlatformError> {
        self.reply(format!("open_url {url}")).map(drop)
    }
    fn open_app(&self, name: &str) -> Result<(), PlatformError> {
        self.reply(format!("apps open {name}")).map(drop)
    }
    fn open_mic_settings(&self) -> Result<(), PlatformError> {
        Ok(())
    }
    fn open_full_disk_access_settings(&self) -> Result<(), PlatformError> {
        Ok(())
    }
    fn has_full_disk_access(&self) -> bool {
        true
    }
    fn in_call_or_sharing(&self) -> bool {
        false
    }
    fn send_imessage(&self, to: &str, text: &str) -> Result<(), PlatformError> {
        self.reply(format!("imessage {to} {text}")).map(drop)
    }
    fn add_calendar_event(&self, t: &str, s: &str, m: u32) -> Result<(), PlatformError> {
        self.reply(format!("calendar {t} {s} {m}")).map(drop)
    }
    fn spotify(&self, cmd: SpotifyCommand<'_>) -> Result<NowPlaying, PlatformError> {
        self.reply(format!("spotify {cmd:?}"))
            .map(|_| NowPlaying::default())
    }
    fn shortcuts(&self, cmd: ShortcutCommand<'_>) -> Result<String, PlatformError> {
        self.reply(match cmd {
            ShortcutCommand::List => "shortcuts list".into(),
            ShortcutCommand::Run { name, input } => format!(
                "shortcuts run {name}{}",
                input.map(|i| format!(" <- {i}")).unwrap_or_default()
            ),
        })
    }
    fn system(&self, cmd: SystemCommand) -> Result<String, PlatformError> {
        self.reply(format!("system {cmd:?}"))
    }
    fn apps(&self, cmd: AppCommand<'_>) -> Result<String, PlatformError> {
        self.reply(match cmd {
            AppCommand::List => "apps list".into(),
            AppCommand::Focus(n) => format!("apps focus {n}"),
            AppCommand::Quit(n) => format!("apps quit {n}"),
            AppCommand::Window(n) => format!("apps window {}", n.unwrap_or("front")),
            AppCommand::SetWindow(n, f) => {
                format!("apps set_window {n} {},{},{},{}", f.x, f.y, f.w, f.h)
            }
            AppCommand::Screen => "apps screen".into(),
        })
    }
    fn ax(&self, cmd: AxCommand<'_>) -> Result<String, PlatformError> {
        self.reply(match cmd {
            AxCommand::Tree { app, .. } => format!("ax tree {}", app.unwrap_or("front")),
            AxCommand::Press { app, path, .. } => format!("ax press {app} {path}"),
            AxCommand::SetText {
                app, path, text, ..
            } => format!("ax type {app} {path} {text}"),
            AxCommand::Keys { app, keys } => format!(
                "ax keys {app} {}{}",
                keys.modifiers.join("+"),
                match keys.key {
                    Key::Char(c) => format!(" {c}"),
                    Key::Code(n) => format!(" #{n}"),
                }
            ),
        })
    }
    fn screenshot(&self, path: &Path) -> Result<(), PlatformError> {
        self.reply(format!("screenshot {}", path.display()))?;
        // The smallest JPEG start: enough for a magic-number check.
        std::fs::write(
            path,
            [0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10, b'J', b'F', b'I', b'F'],
        )
        .map_err(|e| PlatformError::Open(e.to_string()))
    }
    fn has_permission(&self, p: Permission) -> bool {
        self.granted.lock().expect("granted").contains(&p)
    }
    fn open_permission_settings(&self, p: Permission) -> Result<(), PlatformError> {
        self.opened_settings.lock().expect("opened").push(p);
        Ok(())
    }
}
