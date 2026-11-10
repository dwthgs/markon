//! Everything OS-specific sits behind [`Platform`]. Only macOS is implemented;
//! another OS is a new impl of the same trait.
use std::path::PathBuf;

/// Keychain service name for every Markon secret.
pub const KEYCHAIN_SERVICE: &str = "com.markon.desktop";

/// Keychain account names.
pub mod accounts {
    pub const ANTHROPIC: &str = "anthropic-api-key";
    pub const OPENROUTER: &str = "openrouter-api-key";
    pub const RELAY_TOKEN: &str = "relay-token";
    pub const GITHUB: &str = "github-token";
    pub const TELEGRAM_API_HASH: &str = "telegram-api-hash";
    pub const TELEGRAM_SESSION: &str = "telegram-session";
}

#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    #[error("keychain: {0}")]
    Keychain(String),
    #[error("cannot open {0}")]
    Open(String),
    #[error("refused: {0}")]
    Refused(String),
}

pub trait Platform: Send + Sync {
    /// `~/.markon`: settings, database, models, cache, logs.
    fn data_dir(&self) -> PathBuf;
    fn secret_get(&self, account: &str) -> Result<Option<String>, PlatformError>;
    fn secret_set(&self, account: &str, value: &str) -> Result<(), PlatformError>;
    fn secret_delete(&self, account: &str) -> Result<(), PlatformError>;
    /// Open a web URL in the default browser.
    fn open_url(&self, url: &str) -> Result<(), PlatformError>;
    /// Open an installed application by name.
    fn open_app(&self, name: &str) -> Result<(), PlatformError>;
    /// Deep link to the microphone privacy setting.
    fn open_mic_settings(&self) -> Result<(), PlatformError>;
    /// Deep link to the Full Disk Access privacy setting (the Mac databases need it).
    fn open_full_disk_access_settings(&self) -> Result<(), PlatformError>;
    /// False when macOS refuses Markon a protected file. A rebuild of the app can drop the grant.
    fn has_full_disk_access(&self) -> bool;
    /// True while the user is likely in a call or sharing the screen (scheduled agents stay quiet).
    fn in_call_or_sharing(&self) -> bool;
    /// Send an iMessage (or SMS through the iPhone) with Messages. The caller has asked the user.
    fn send_imessage(&self, to: &str, text: &str) -> Result<(), PlatformError>;
    /// Add an event to Calendar. `start` is local "YYYY-MM-DD HH:MM". The caller has asked the user.
    fn add_calendar_event(
        &self,
        title: &str,
        start: &str,
        minutes: u32,
    ) -> Result<(), PlatformError>;
    /// Control the Spotify app and report what it is playing.
    fn spotify(&self, cmd: SpotifyCommand<'_>) -> Result<NowPlaying, PlatformError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpotifyCommand<'a> {
    Status,
    Play,
    Pause,
    Next,
    Previous,
    /// A `spotify:kind:id` URI, already validated by the caller.
    PlayUri(&'a str),
}

/// What Spotify is playing. Fields are empty when nothing is loaded.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NowPlaying {
    pub track: String,
    pub artist: String,
    pub album: String,
    pub artwork: String,
    /// "playing", "paused" or "stopped".
    pub state: String,
    pub position_s: f64,
    pub duration_s: f64,
}

/// A Spotify URI safe to place inside an AppleScript string.
fn plain_uri(uri: &str) -> bool {
    uri.starts_with("spotify:") && uri.chars().all(|c| c.is_ascii_alphanumeric() || c == ':')
}

/// The AppleScript for a command: one line per field, in `NowPlaying` order.
pub fn spotify_script(cmd: SpotifyCommand<'_>) -> Option<String> {
    let action = match cmd {
        SpotifyCommand::Status => String::new(),
        SpotifyCommand::Play => "play\n".into(),
        SpotifyCommand::Pause => "pause\n".into(),
        SpotifyCommand::Next => "next track\n".into(),
        SpotifyCommand::Previous => "previous track\n".into(),
        SpotifyCommand::PlayUri(u) if plain_uri(u) => format!("play track \"{u}\"\n"),
        SpotifyCommand::PlayUri(_) => return None,
    };
    Some(format!(
        "tell application \"Spotify\"\n{action}\
         if player state is stopped then return \"\" & linefeed & \"\" & linefeed & \"\" & linefeed & \"\" & linefeed & \"stopped\" & linefeed & \"0\" & linefeed & \"0\"\n\
         set t to current track\n\
         return (name of t) & linefeed & (artist of t) & linefeed & (album of t) & linefeed & (artwork url of t) & linefeed & (player state as string) & linefeed & (player position as string) & linefeed & ((duration of t) / 1000 as string)\n\
         end tell"
    ))
}

pub fn parse_now_playing(out: &str) -> NowPlaying {
    let l: Vec<&str> = out.trim_end_matches('\n').split('\n').collect();
    let s = |i: usize| l.get(i).map(|x| x.trim().to_string()).unwrap_or_default();
    // AppleScript prints decimals with the user's locale ("12,5").
    let n = |i: usize| s(i).replace(',', ".").parse().unwrap_or(0.0);
    NowPlaying {
        track: s(0),
        artist: s(1),
        album: s(2),
        artwork: s(3),
        state: s(4),
        position_s: n(5),
        duration_s: n(6),
    }
}

/// Only web and mail links; never `file:` or custom schemes that could launch arbitrary handlers.
pub fn is_safe_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    (lower.starts_with("https://") || lower.starts_with("http://") || lower.starts_with("mailto:"))
        && !url.chars().any(|c| c.is_control())
}

/// App names are plain words: no paths, no flags.
pub fn is_safe_app_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || " .-_&".contains(c))
}

/// The Mac's own databases that sources read, read-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacDb {
    Messages,
    Calendar,
    Notes,
}

pub fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| "/tmp".into()))
}

pub fn mac_db(home: &std::path::Path, db: MacDb) -> PathBuf {
    let lib = home.join("Library");
    match db {
        MacDb::Messages => lib.join("Messages/chat.db"),
        MacDb::Calendar => lib.join("Group Containers/group.com.apple.calendar/Calendar.sqlitedb"),
        MacDb::Notes => lib.join("Group Containers/group.com.apple.notes/NoteStore.sqlite"),
    }
}

/// Every Contacts database: the local one and one per synced account. Listing the folder needs
/// Full Disk Access too, so a missing grant gives an empty list.
pub fn contacts_dbs(home: &std::path::Path) -> Vec<PathBuf> {
    let root = home.join("Library/Application Support/AddressBook");
    let mut v = vec![root.join("AddressBook-v22.abcddb")];
    if let Ok(rd) = std::fs::read_dir(root.join("Sources")) {
        v.extend(
            rd.flatten()
                .map(|e| e.path().join("AddressBook-v22.abcddb")),
        );
    }
    v.retain(|p| p.exists());
    v
}

/// Messages and Calendar scripts take their values as arguments (`argv`), never pasted into the
/// script text, so a message cannot inject AppleScript.
pub const SEND_IMESSAGE: &str = "on run argv\n\
    set target to item 1 of argv\n\
    set body to item 2 of argv\n\
    tell application \"Messages\"\n\
    set svc to 1st account whose service type = iMessage\n\
    send body to participant target of svc\n\
    end tell\n\
    end run";

pub const ADD_EVENT: &str = "on run argv\n\
    set t to item 1 of argv\n\
    set y to (item 2 of argv) as integer\n\
    set mo to (item 3 of argv) as integer\n\
    set d to (item 4 of argv) as integer\n\
    set h to (item 5 of argv) as integer\n\
    set mi to (item 6 of argv) as integer\n\
    set len to (item 7 of argv) as integer\n\
    set s to current date\n\
    set day of s to 1\n\
    set year of s to y\n\
    set month of s to mo\n\
    set day of s to d\n\
    set hours of s to h\n\
    set minutes of s to mi\n\
    set seconds of s to 0\n\
    tell application \"Calendar\"\n\
    tell first calendar whose writable is true\n\
    make new event with properties {summary:t, start date:s, end date:s + len * minutes}\n\
    end tell\n\
    end tell\n\
    end run";

/// "2026-11-11 09:30" as (year, month, day, hour, minute).
pub fn split_local(start: &str) -> Option<(i32, u32, u32, u32, u32)> {
    let (date, time) = start.trim().split_once([' ', 'T'])?;
    let mut d = date.split('-').map(|x| x.parse::<u32>().ok());
    let mut t = time.split(':').map(|x| x.parse::<u32>().ok());
    let (y, mo, day) = (d.next()??, d.next()??, d.next()??);
    let (h, mi) = (t.next()??, t.next()??);
    ((1..=12).contains(&mo) && (1..=31).contains(&day) && h < 24 && mi < 60)
        .then_some((y as i32, mo, day, h, mi))
}

fn osascript(script: &str, args: &[&str], what: &str) -> Result<(), PlatformError> {
    let out = std::process::Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(script)
        .args(args)
        .output()
        .map_err(|e| PlatformError::Open(e.to_string()))?;
    if out.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    Err(PlatformError::Open(if err.contains("-1743") {
        format!("Markon is not allowed to control {what} (System Settings > Privacy > Automation)")
    } else {
        format!("{what} did not do it: {}", err.trim())
    }))
}

/// Installed memory in GB, for picking a local model that fits.
pub fn total_memory_gb() -> f64 {
    std::process::Command::new("/usr/sbin/sysctl")
        .args(["-n", "hw.memsize"])
        .output()
        .ok()
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .parse::<f64>()
                .ok()
        })
        .map(|b| b / 1_073_741_824.0)
        .unwrap_or(8.0)
}

pub fn current() -> impl Platform {
    MacOs::default()
}

/// Secrets are read from the keychain once per launch and kept in memory: every keychain read by a
/// new (unsigned dev) build makes macOS ask for the login password.
#[derive(Default)]
pub struct MacOs {
    secrets: std::sync::Mutex<std::collections::HashMap<String, Option<String>>>,
}

fn entry(account: &str) -> Result<keyring::Entry, PlatformError> {
    keyring::Entry::new(KEYCHAIN_SERVICE, account)
        .map_err(|e| PlatformError::Keychain(e.to_string()))
}

fn run_open(args: &[&str]) -> Result<(), PlatformError> {
    let ok = std::process::Command::new("/usr/bin/open")
        .args(args)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if ok {
        Ok(())
    } else {
        Err(PlatformError::Open(args.join(" ")))
    }
}

/// Processes that mean a call or screen share is in progress.
const CALL_PROCESSES: &[&str] = &[
    "CptHost",
    "zoom.us",
    "FaceTime",
    "Microsoft Teams",
    "MSTeams",
    "Webex",
    "Slack Huddle",
];

impl Platform for MacOs {
    fn data_dir(&self) -> PathBuf {
        std::env::var_os("MARKON_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| "/tmp".into()))
                    .join(".markon")
            })
    }

    fn secret_get(&self, account: &str) -> Result<Option<String>, PlatformError> {
        let mut cache = self.secrets.lock().expect("secrets");
        if let Some(v) = cache.get(account) {
            return Ok(v.clone());
        }
        // A failed read (for example a denied prompt) is not cached, so the next use can ask again.
        let v = match entry(account)?.get_password() {
            Ok(v) => Some(v),
            Err(keyring::Error::NoEntry) => None,
            Err(e) => return Err(PlatformError::Keychain(e.to_string())),
        };
        cache.insert(account.to_string(), v.clone());
        Ok(v)
    }

    fn secret_set(&self, account: &str, value: &str) -> Result<(), PlatformError> {
        entry(account)?
            .set_password(value)
            .map_err(|e| PlatformError::Keychain(e.to_string()))?;
        self.secrets
            .lock()
            .expect("secrets")
            .insert(account.to_string(), Some(value.to_string()));
        Ok(())
    }

    fn secret_delete(&self, account: &str) -> Result<(), PlatformError> {
        match entry(account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {}
            Err(e) => return Err(PlatformError::Keychain(e.to_string())),
        }
        self.secrets
            .lock()
            .expect("secrets")
            .insert(account.to_string(), None);
        Ok(())
    }

    fn open_url(&self, url: &str) -> Result<(), PlatformError> {
        if !is_safe_url(url) {
            return Err(PlatformError::Refused(format!("not a web link: {url}")));
        }
        run_open(&[url])
    }

    fn open_app(&self, name: &str) -> Result<(), PlatformError> {
        if !is_safe_app_name(name) {
            return Err(PlatformError::Refused(format!("not an app name: {name}")));
        }
        run_open(&["-a", name])
    }

    fn open_mic_settings(&self) -> Result<(), PlatformError> {
        run_open(&["x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"])
    }

    fn send_imessage(&self, to: &str, text: &str) -> Result<(), PlatformError> {
        if to.trim().is_empty() || to.starts_with('-') || text.trim().is_empty() {
            return Err(PlatformError::Refused(
                "a message needs a recipient and text".into(),
            ));
        }
        osascript(SEND_IMESSAGE, &[to.trim(), text], "Messages")
    }

    fn add_calendar_event(
        &self,
        title: &str,
        start: &str,
        minutes: u32,
    ) -> Result<(), PlatformError> {
        let (y, mo, d, h, mi) = split_local(start)
            .ok_or_else(|| PlatformError::Refused(format!("{start} is not YYYY-MM-DD HH:MM")))?;
        let n = |v: u32| v.to_string();
        osascript(
            ADD_EVENT,
            &[
                title,
                &y.to_string(),
                &n(mo),
                &n(d),
                &n(h),
                &n(mi),
                &n(minutes.clamp(5, 24 * 60)),
            ],
            "Calendar",
        )
    }

    fn open_full_disk_access_settings(&self) -> Result<(), PlatformError> {
        run_open(&["x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles"])
    }

    fn has_full_disk_access(&self) -> bool {
        // Opening (not reading) the privacy database is the usual probe: only Full Disk Access allows it.
        let tcc = home().join("Library/Application Support/com.apple.TCC/TCC.db");
        !matches!(std::fs::File::open(tcc), Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied)
    }

    fn spotify(&self, cmd: SpotifyCommand<'_>) -> Result<NowPlaying, PlatformError> {
        if !std::path::Path::new("/Applications/Spotify.app").exists() {
            return Err(PlatformError::Open("Spotify is not installed".into()));
        }
        let script = spotify_script(cmd)
            .ok_or_else(|| PlatformError::Refused("not a Spotify link".into()))?;
        let out = std::process::Command::new("/usr/bin/osascript")
            .args(["-e", &script])
            .output()
            .map_err(|e| PlatformError::Open(e.to_string()))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(PlatformError::Open(if err.contains("-1743") {
                "Markon is not allowed to control Spotify (System Settings > Privacy > Automation)"
                    .into()
            } else {
                format!("Spotify did not respond: {}", err.trim())
            }));
        }
        Ok(parse_now_playing(&String::from_utf8_lossy(&out.stdout)))
    }

    fn in_call_or_sharing(&self) -> bool {
        // shortcut: process-name heuristic; upgrade to the CoreAudio "device in use" property when it misfires.
        let Ok(out) = std::process::Command::new("/bin/ps")
            .args(["-axo", "comm"])
            .output()
        else {
            return false;
        };
        let list = String::from_utf8_lossy(&out.stdout);
        list.lines()
            .any(|l| CALL_PROCESSES.iter().any(|p| l.ends_with(p)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_web_links_open() {
        assert!(is_safe_url("https://example.com/a?b=1"));
        assert!(is_safe_url("mailto:x@example.com"));
        assert!(!is_safe_url("file:///etc/passwd"));
        assert!(!is_safe_url("javascript:alert(1)"));
        assert!(!is_safe_url("vscode://x"));
        assert!(!is_safe_url("https://a\nb"));
    }

    #[test]
    fn app_names_cannot_smuggle_flags_or_paths() {
        assert!(is_safe_app_name("Safari"));
        assert!(is_safe_app_name("Visual Studio Code"));
        assert!(!is_safe_app_name("-n"));
        assert!(!is_safe_app_name("/System/Applications/Calculator.app"));
        assert!(!is_safe_app_name("Calc;rm -rf"));
    }

    #[test]
    fn spotify_scripts_cannot_be_injected() {
        let s = spotify_script(SpotifyCommand::PlayUri(
            "spotify:track:4uLU6hMCjMI75M1A2tKUQC",
        ))
        .unwrap();
        assert!(s.contains("play track \"spotify:track:4uLU6hMCjMI75M1A2tKUQC\""));
        assert_eq!(
            spotify_script(SpotifyCommand::PlayUri(
                "spotify:x\" & do shell script \"id"
            )),
            None
        );
        assert!(
            !spotify_script(SpotifyCommand::Status)
                .unwrap()
                .contains("play track")
        );
        assert!(
            spotify_script(SpotifyCommand::Pause)
                .unwrap()
                .contains("\npause\n")
        );
    }

    #[test]
    fn reads_what_spotify_reports() {
        let n =
            parse_now_playing("Song\nBand\nAlbum\nhttps://i.scdn.co/a\nplaying\n12,5\n200.25\n");
        assert_eq!(n.track, "Song");
        assert_eq!(n.state, "playing");
        assert_eq!(n.position_s, 12.5);
        assert_eq!(n.duration_s, 200.25);
        assert_eq!(parse_now_playing("\n\n\n\nstopped\n0\n0").state, "stopped");
    }

    #[test]
    fn scripts_take_values_as_arguments_only() {
        for s in [SEND_IMESSAGE, ADD_EVENT] {
            assert!(s.starts_with("on run argv") && s.contains("item 1 of argv"));
        }
        assert_eq!(split_local("2026-11-11 09:30"), Some((2026, 11, 11, 9, 30)));
        assert_eq!(split_local("2026-11-11T09:30"), Some((2026, 11, 11, 9, 30)));
        assert_eq!(split_local("tomorrow"), None);
        assert_eq!(split_local("2026-13-01 09:00"), None);
    }

    #[test]
    fn mac_databases_live_under_the_users_library() {
        let h = std::path::Path::new("/Users/x");
        assert_eq!(
            mac_db(h, MacDb::Messages),
            std::path::Path::new("/Users/x/Library/Messages/chat.db")
        );
        assert!(mac_db(h, MacDb::Notes).ends_with("group.com.apple.notes/NoteStore.sqlite"));
        assert!(contacts_dbs(std::path::Path::new("/nonexistent")).is_empty());
    }

    #[test]
    fn data_dir_is_under_home() {
        if std::env::var_os("MARKON_HOME").is_none() {
            assert!(MacOs::default().data_dir().ends_with(".markon"));
        }
    }

    /// Touches the real login keychain; run with `cargo test -p markon-platform -- --ignored`.
    #[test]
    #[ignore]
    fn keychain_round_trip() {
        let p = MacOs::default();
        let acct = "markon-test-account";
        p.secret_set(acct, "value-1").unwrap();
        assert_eq!(p.secret_get(acct).unwrap().as_deref(), Some("value-1"));
        p.secret_delete(acct).unwrap();
        assert_eq!(p.secret_get(acct).unwrap(), None);
    }
}
