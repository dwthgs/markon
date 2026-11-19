//! Full control of the Mac: Shortcuts, system controls, apps and windows, other apps'
//! Accessibility tree, and screenshots. Everything runs a built-in macOS command (`shortcuts`,
//! `osascript`, `pmset`, `networksetup`, `screencapture`, `sips`) with values passed as arguments,
//! never pasted into script text. The pure parts (key names, layouts, parsers) are tested here; the
//! commands themselves only run in the app.
use crate::PlatformError;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortcutCommand<'a> {
    List,
    /// Run a shortcut by its exact name, with optional text input. The caller has asked the user.
    Run {
        name: &'a str,
        input: Option<&'a str>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemCommand {
    /// "<volume 0-100>,<muted true|false>".
    Status,
    Volume(u8),
    Mute(bool),
    /// One step of the brightness keys: true is brighter.
    Brightness(bool),
    Wifi(bool),
    Sleep,
    Lock,
}

/// A window's place on screen in points, top-left origin (System Events' coordinates).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppCommand<'a> {
    /// JSON `{"front": name, "apps": [names]}` of apps with a Dock icon.
    List,
    Focus(&'a str),
    Quit(&'a str),
    /// "<app>\t<x>,<y>,<w>,<h>" of the front window of `app`, or of the frontmost app.
    Window(Option<&'a str>),
    SetWindow(&'a str, Frame),
    /// "<x>,<y>,<w>,<h>": the main display without the menu bar and Dock.
    Screen,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AxCommand<'a> {
    /// JSON `{"app": name, "nodes": [{path, depth, role, subrole, title, desc, value, secure}]}` for
    /// the front window of `app` (or of the frontmost app). Secure text fields' values are never read.
    Tree { app: Option<&'a str>, max: usize },
    /// Press the element at `path` ("1.3.2": window, then 1-based children) if its role is still `role`.
    Press {
        app: &'a str,
        path: &'a str,
        role: &'a str,
    },
    /// Put `text` in the text field at `path`. Refused for secure fields, here and by the caller.
    SetText {
        app: &'a str,
        path: &'a str,
        role: &'a str,
        text: &'a str,
    },
    /// Press a key with modifiers in `app`.
    Keys { app: &'a str, keys: Keys },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Permission {
    Accessibility,
    ScreenRecording,
}

impl Permission {
    pub fn settings_url(self) -> &'static str {
        match self {
            Permission::Accessibility => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
            }
            Permission::ScreenRecording => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
            }
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Permission::Accessibility => "Accessibility",
            Permission::ScreenRecording => "Screen Recording",
        }
    }
    /// The card's `needs_permission` value and the settings command's argument.
    pub fn id(self) -> &'static str {
        match self {
            Permission::Accessibility => "accessibility",
            Permission::ScreenRecording => "screen_recording",
        }
    }
    pub fn from_id(id: &str) -> Option<Self> {
        [Permission::Accessibility, Permission::ScreenRecording]
            .into_iter()
            .find(|p| p.id() == id)
    }
}

/// A key press: a character or a named key's code, with System Events modifier names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keys {
    pub key: Key,
    pub modifiers: Vec<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Code(u16),
}

const NAMED_KEYS: &[(&str, u16)] = &[
    ("return", 36),
    ("enter", 76),
    ("tab", 48),
    ("space", 49),
    ("delete", 51),
    ("backspace", 51),
    ("escape", 53),
    ("esc", 53),
    ("left", 123),
    ("right", 124),
    ("down", 125),
    ("up", 126),
    ("home", 115),
    ("end", 119),
    ("pageup", 116),
    ("pagedown", 121),
    ("forwarddelete", 117),
];

/// "cmd+shift+s", "return", "escape": None for anything else, so nothing odd reaches the script.
pub fn parse_keys(spec: &str) -> Option<Keys> {
    let parts: Vec<String> = spec.split('+').map(|p| p.trim().to_lowercase()).collect();
    let (last, mods) = parts.split_last()?;
    let mut modifiers = Vec::new();
    for m in mods {
        let name = match m.as_str() {
            "cmd" | "command" | "⌘" => "command down",
            "shift" | "⇧" => "shift down",
            "alt" | "option" | "opt" | "⌥" => "option down",
            "ctrl" | "control" | "⌃" => "control down",
            _ => return None,
        };
        if !modifiers.contains(&name) {
            modifiers.push(name);
        }
    }
    let mut chars = last.chars();
    let key = match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_graphic() => Key::Char(c),
        _ => Key::Code(
            NAMED_KEYS
                .iter()
                .find(|(n, _)| *n == last.replace([' ', '_', '-'], ""))?
                .1,
        ),
    };
    Some(Keys { key, modifiers })
}

/// Simple window layouts on the visible screen.
pub fn layout_frame(layout: &str, screen: Frame) -> Option<Frame> {
    let half = screen.w / 2;
    Some(match layout {
        "left" => Frame { w: half, ..screen },
        "right" => Frame {
            x: screen.x + half,
            w: screen.w - half,
            ..screen
        },
        "full" => screen,
        "center" => Frame {
            x: screen.x + screen.w / 6,
            y: screen.y + screen.h / 8,
            w: screen.w * 2 / 3,
            h: screen.h * 3 / 4,
        },
        _ => return None,
    })
}

pub fn parse_frame(s: &str) -> Option<Frame> {
    let n: Vec<i64> = s
        .split(',')
        .map(|v| v.trim().parse::<f64>().ok().map(|f| f.round() as i64))
        .collect::<Option<_>>()?;
    match n[..] {
        [x, y, w, h] if w > 0 && h > 0 => Some(Frame { x, y, w, h }),
        _ => None,
    }
}

/// The Wi-Fi device ("en0") from `networksetup -listallhardwareports`.
pub fn wifi_device(ports: &str) -> Option<String> {
    let mut lines = ports.lines();
    while let Some(l) = lines.next() {
        if l.trim() == "Hardware Port: Wi-Fi" || l.trim() == "Hardware Port: AirPort" {
            return lines
                .next()?
                .trim()
                .strip_prefix("Device: ")
                .map(str::to_string);
        }
    }
    None
}

/// Shortcut names are passed as one argument; refuse ones that would read as a flag.
pub fn is_safe_shortcut_name(name: &str) -> bool {
    !name.trim().is_empty()
        && name.len() <= 200
        && !name.starts_with('-')
        && !name.chars().any(char::is_control)
}

/// An element path from the tree: "1", "1.3", "1.3.12".
pub fn is_element_path(p: &str) -> bool {
    p.split('.')
        .all(|n| !n.is_empty() && n.len() <= 4 && n.chars().all(|c| c.is_ascii_digit()))
        && p.starts_with('1')
}

/// Run a command, killing it after `limit`. Returns stdout, or stderr as the error.
pub(crate) fn run(program: &str, args: &[&str], limit: Duration) -> Result<String, String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{program}: {e}"))?;
    let started = Instant::now();
    loop {
        if child.try_wait().map_err(|e| e.to_string())?.is_some() {
            break;
        }
        if started.elapsed() > limit {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "it did not finish within {} s and was stopped",
                limit.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

const SHORT: Duration = Duration::from_secs(15);

/// osascript errors in words: missing Automation or Accessibility permission are named.
fn script_error(err: String, what: &str) -> PlatformError {
    if err.contains("-1743") {
        PlatformError::Open(format!(
            "Markon is not allowed to control {what} (System Settings > Privacy > Automation)"
        ))
    } else if err.contains("-1719") || err.contains("-25211") || err.contains("assistive") {
        PlatformError::Refused(
            "Markon needs the Accessibility permission (System Settings > Privacy > Accessibility)"
                .into(),
        )
    } else {
        PlatformError::Open(format!("{what} did not do it: {err}"))
    }
}

fn jxa(script: &str, args: &[&str], what: &str) -> Result<String, PlatformError> {
    let mut a = vec!["-l", "JavaScript", "-e", script];
    a.extend_from_slice(args);
    run("/usr/bin/osascript", &a, SHORT).map_err(|e| script_error(e, what))
}

fn applescript(script: &str, args: &[&str], what: &str) -> Result<String, PlatformError> {
    let mut a = vec!["-e", script];
    a.extend_from_slice(args);
    run("/usr/bin/osascript", &a, SHORT).map_err(|e| script_error(e, what))
}

pub(crate) fn shortcuts(cmd: ShortcutCommand<'_>) -> Result<String, PlatformError> {
    let bin = "/usr/bin/shortcuts";
    let fail = |e: String| PlatformError::Open(format!("Shortcuts: {e}"));
    match cmd {
        ShortcutCommand::List => run(bin, &["list"], SHORT).map_err(fail),
        ShortcutCommand::Run { name, input } => {
            if !is_safe_shortcut_name(name) {
                return Err(PlatformError::Refused(format!(
                    "not a shortcut name: {name}"
                )));
            }
            let dir = std::env::temp_dir().join(format!("markon-shortcut-{}", std::process::id()));
            std::fs::create_dir_all(&dir).map_err(|e| fail(e.to_string()))?;
            let out = dir.join("out.txt");
            let _ = std::fs::remove_file(&out);
            let mut args = vec!["run".to_string(), name.to_string()];
            if let Some(text) = input {
                let inp = dir.join("in.txt");
                std::fs::write(&inp, text).map_err(|e| fail(e.to_string()))?;
                args.extend(["--input-path".into(), inp.to_string_lossy().into_owned()]);
            }
            args.extend([
                "--output-path".into(),
                out.to_string_lossy().into_owned(),
                "--output-type".into(),
                "public.plain-text".into(),
            ]);
            let a: Vec<&str> = args.iter().map(String::as_str).collect();
            run(bin, &a, Duration::from_secs(60)).map_err(fail)?;
            let text = std::fs::read_to_string(&out).unwrap_or_default();
            let _ = std::fs::remove_dir_all(&dir);
            Ok(text)
        }
    }
}

const KEY_SCRIPT: &str = "function run(argv) {
  const se = Application('System Events');
  if (argv[0]) { Application(argv[0]).activate(); delay(0.3); }
  const mods = argv[3] ? argv[3].split(',') : [];
  if (argv[2]) se.keyCode(parseInt(argv[2], 10), {using: mods});
  else se.keystroke(argv[1], {using: mods});
  return 'ok';
}";

fn press(app: &str, keys: &Keys) -> Result<String, PlatformError> {
    let (ch, code) = match keys.key {
        Key::Char(c) => (c.to_string(), String::new()),
        Key::Code(n) => (String::new(), n.to_string()),
    };
    jxa(
        KEY_SCRIPT,
        &[app, &ch, &code, &keys.modifiers.join(",")],
        "System Events",
    )
}

pub(crate) fn system(cmd: SystemCommand) -> Result<String, PlatformError> {
    match cmd {
        SystemCommand::Status => applescript(
            "set s to get volume settings\nreturn (output volume of s as string) & \",\" & (output muted of s as string)",
            &[],
            "the volume",
        ),
        SystemCommand::Volume(v) => applescript(
            "on run argv\nset volume output volume ((item 1 of argv) as integer)\nend run",
            &[&v.min(100).to_string()],
            "the volume",
        ),
        SystemCommand::Mute(m) => applescript(
            "on run argv\nset volume output muted ((item 1 of argv) is \"true\")\nend run",
            &[if m { "true" } else { "false" }],
            "the volume",
        ),
        SystemCommand::Brightness(up) => press(
            "",
            &Keys {
                key: Key::Code(if up { 144 } else { 145 }),
                modifiers: vec![],
            },
        ),
        SystemCommand::Wifi(on) => {
            let ports = run("/usr/sbin/networksetup", &["-listallhardwareports"], SHORT)
                .map_err(PlatformError::Open)?;
            let dev = wifi_device(&ports)
                .ok_or_else(|| PlatformError::Open("this Mac has no Wi-Fi".into()))?;
            run(
                "/usr/sbin/networksetup",
                &["-setairportpower", &dev, if on { "on" } else { "off" }],
                SHORT,
            )
            .map_err(PlatformError::Open)
        }
        SystemCommand::Sleep => {
            run("/usr/bin/pmset", &["sleepnow"], SHORT).map_err(PlatformError::Open)
        }
        // Control-Command-Q is the system's own lock shortcut.
        SystemCommand::Lock => press(
            "",
            &Keys {
                key: Key::Char('q'),
                modifiers: vec!["control down", "command down"],
            },
        ),
    }
}

const APPS_SCRIPT: &str = "function run() {
  const se = Application('System Events');
  const front = se.processes.whose({frontmost: true});
  return JSON.stringify({front: front.length ? front[0].name() : '', apps: se.processes.whose({backgroundOnly: false}).name()});
}";

const QUIT_SCRIPT: &str = "function run(argv) {
  const a = Application(argv[0]);
  if (!a.running()) return 'not running';
  a.quit();
  return 'quit';
}";

const WINDOW_SCRIPT: &str = "function run(argv) {
  const se = Application('System Events');
  const p = argv[0] ? se.processes.byName(argv[0]) : se.processes.whose({frontmost: true})[0];
  const w = p.windows[0];
  const pos = w.position(), size = w.size();
  return p.name() + '\\t' + [pos[0], pos[1], size[0], size[1]].join(',');
}";

const SET_WINDOW_SCRIPT: &str = "function run(argv) {
  const se = Application('System Events');
  const w = se.processes.byName(argv[0]).windows[0];
  w.position = [parseInt(argv[1], 10), parseInt(argv[2], 10)];
  w.size = [parseInt(argv[3], 10), parseInt(argv[4], 10)];
  return 'ok';
}";

// shortcut: the primary display only; windows on a second display are laid out on the first.
const SCREEN_SCRIPT: &str = "function run() {
  ObjC.import('AppKit');
  const s = $.NSScreen.screens.objectAtIndex(0);
  const f = s.frame, v = s.visibleFrame;
  return [v.origin.x, f.size.height - v.origin.y - v.size.height, v.size.width, v.size.height].join(',');
}";

fn app_name(name: &str) -> Result<&str, PlatformError> {
    if crate::is_safe_app_name(name) {
        Ok(name)
    } else {
        Err(PlatformError::Refused(format!("not an app name: {name}")))
    }
}

pub(crate) fn apps(cmd: AppCommand<'_>) -> Result<String, PlatformError> {
    match cmd {
        AppCommand::List => jxa(APPS_SCRIPT, &[], "System Events"),
        AppCommand::Focus(n) => crate::run_open(&["-a", app_name(n)?]).map(|_| "focused".into()),
        AppCommand::Quit(n) => jxa(QUIT_SCRIPT, &[app_name(n)?], n),
        AppCommand::Window(n) => {
            let n = n.map(app_name).transpose()?.unwrap_or("");
            jxa(WINDOW_SCRIPT, &[n], "System Events")
        }
        AppCommand::SetWindow(n, f) => jxa(
            SET_WINDOW_SCRIPT,
            &[
                app_name(n)?,
                &f.x.to_string(),
                &f.y.to_string(),
                &f.w.to_string(),
                &f.h.to_string(),
            ],
            "System Events",
        ),
        AppCommand::Screen => jxa(SCREEN_SCRIPT, &[], "the screen"),
    }
}

const TREE_SCRIPT: &str = "function run(argv) {
  const se = Application('System Events');
  const p = argv[0] ? se.processes.byName(argv[0]) : se.processes.whose({frontmost: true})[0];
  const max = parseInt(argv[1], 10);
  const out = [];
  const get = (f) => { try { const v = f(); return (v === null || v === undefined) ? '' : v; } catch (e) { return ''; } };
  function walk(el, path, depth) {
    if (out.length >= max || depth > 30) return;
    const role = String(get(() => el.role()));
    const sub = String(get(() => el.subrole()));
    const secure = role === 'AXSecureTextField' || sub === 'AXSecureTextField';
    let value = '';
    if (!secure) { const v = get(() => el.value()); if (typeof v !== 'object') value = String(v).slice(0, 200); }
    out.push({path: path, depth: depth, role: role, subrole: sub, title: String(get(() => el.title())).slice(0, 120),
      desc: String(get(() => el.description())).slice(0, 120), value: value, secure: secure});
    const kids = get(() => el.uiElements());
    for (let i = 0; i < (kids.length || 0); i++) walk(kids[i], path + '.' + (i + 1), depth + 1);
  }
  const wins = p.windows();
  if (wins.length) walk(p.windows[0], '1', 0);
  return JSON.stringify({app: p.name(), nodes: out});
}";

/// Finds the element at argv[1] in app argv[0], checking its role is still argv[2].
const FIND: &str = "const se = Application('System Events');
  let el = se.processes.byName(argv[0]).windows[0];
  for (const i of argv[1].split('.').slice(1)) el = el.uiElements[parseInt(i, 10) - 1];
  if (el.role() !== argv[2]) throw new Error('the window changed; read it again');
  const secure = el.role() === 'AXSecureTextField' || (function () { try { return el.subrole() === 'AXSecureTextField'; } catch (e) { return false; } })();";

fn press_script() -> String {
    format!(
        "function run(argv) {{\n  {FIND}\n  Application(argv[0]).activate();\n  try {{ el.actions.byName('AXPress').perform(); }} catch (e) {{ el.click(); }}\n  return 'ok';\n}}"
    )
}

fn set_text_script() -> String {
    format!(
        "function run(argv) {{\n  {FIND}\n  if (secure) throw new Error('secure field');\n  Application(argv[0]).activate();\n  el.focused = true;\n  el.value = argv[3];\n  return 'ok';\n}}"
    )
}

pub(crate) fn ax(cmd: AxCommand<'_>) -> Result<String, PlatformError> {
    let path_ok = |p: &str| {
        if is_element_path(p) {
            Ok(())
        } else {
            Err(PlatformError::Refused(format!("not an element: {p}")))
        }
    };
    match cmd {
        AxCommand::Tree { app, max } => {
            let a = app.map(app_name).transpose()?.unwrap_or("");
            jxa(TREE_SCRIPT, &[a, &max.to_string()], "System Events")
        }
        AxCommand::Press { app, path, role } => {
            path_ok(path)?;
            jxa(&press_script(), &[app_name(app)?, path, role], app)
        }
        AxCommand::SetText {
            app,
            path,
            role,
            text,
        } => {
            path_ok(path)?;
            jxa(&set_text_script(), &[app_name(app)?, path, role, text], app)
        }
        AxCommand::Keys { app, keys } => press(app_name(app)?, &keys),
    }
}

/// The whole main display as a JPEG at most 1568 px on its long side (the size Claude reads best).
pub(crate) fn screenshot(path: &Path) -> Result<(), PlatformError> {
    let p = path.to_string_lossy();
    run(
        "/usr/sbin/screencapture",
        &["-x", "-m", "-t", "jpg", &p],
        SHORT,
    )
    .map_err(PlatformError::Open)?;
    run("/usr/bin/sips", &["-Z", "1568", &p], SHORT).map_err(PlatformError::Open)?;
    Ok(())
}

const AX_TRUSTED: &str = "ObjC.import('ApplicationServices'); String($.AXIsProcessTrusted())";

/// Window names of other apps are only visible with Screen Recording. No other app's window on screen
/// gives no answer, taken as granted (the capture then shows what there is).
const SCREEN_PROBE: &str = "function run(argv) {
  ObjC.import('CoreGraphics'); ObjC.import('Foundation');
  const me = [$.NSProcessInfo.processInfo.processIdentifier, parseInt(argv[0], 10)];
  const all = ObjC.deepUnwrap(ObjC.castRefToObject($.CGWindowListCopyWindowInfo(1, 0))) || [];
  const others = all.filter(w => w.kCGWindowLayer === 0 && !me.includes(w.kCGWindowOwnerPID));
  return String(others.length === 0 || others.some(w => w.kCGWindowName !== undefined));
}";

pub(crate) fn has_permission(p: Permission) -> bool {
    let r = match p {
        Permission::Accessibility => jxa(AX_TRUSTED, &[], "the permission check"),
        Permission::ScreenRecording => jxa(
            SCREEN_PROBE,
            &[&std::process::id().to_string()],
            "the permission check",
        ),
    };
    r.is_ok_and(|v| v.trim() == "true")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_parsed_into_codes_and_modifiers() {
        assert_eq!(
            parse_keys("cmd+shift+S"),
            Some(Keys {
                key: Key::Char('s'),
                modifiers: vec!["command down", "shift down"]
            })
        );
        assert_eq!(parse_keys("Return").unwrap().key, Key::Code(36));
        assert_eq!(parse_keys("page down").unwrap().key, Key::Code(121));
        assert_eq!(parse_keys("ctrl+esc").unwrap().modifiers, ["control down"]);
        assert_eq!(parse_keys("hyper+x"), None);
        assert_eq!(parse_keys("rm -rf"), None);
        assert_eq!(parse_keys(""), None);
    }

    #[test]
    fn layouts_split_the_visible_screen() {
        let s = Frame {
            x: 0,
            y: 33,
            w: 1512,
            h: 949,
        };
        assert_eq!(
            layout_frame("left", s),
            Some(Frame {
                x: 0,
                y: 33,
                w: 756,
                h: 949
            })
        );
        assert_eq!(layout_frame("right", s).unwrap().x, 756);
        assert_eq!(layout_frame("full", s), Some(s));
        assert_eq!(layout_frame("diagonal", s), None);
        assert_eq!(
            parse_frame("0, 33, 1512.0, 949"),
            Some(Frame {
                x: 0,
                y: 33,
                w: 1512,
                h: 949
            })
        );
        assert_eq!(parse_frame("1,2,0,4"), None);
    }

    #[test]
    fn finds_the_wifi_device() {
        let ports = "Hardware Port: Ethernet\nDevice: en4\n\nHardware Port: Wi-Fi\nDevice: en0\nEthernet Address: x\n";
        assert_eq!(wifi_device(ports).as_deref(), Some("en0"));
        assert_eq!(wifi_device("Hardware Port: Ethernet\nDevice: en4\n"), None);
    }

    #[test]
    fn names_and_paths_cannot_smuggle_flags() {
        assert!(is_safe_shortcut_name("Set Focus: Work"));
        assert!(!is_safe_shortcut_name("--help"));
        assert!(!is_safe_shortcut_name("a\nb"));
        assert!(is_element_path("1.3.12"));
        assert!(!is_element_path("2.1"));
        assert!(!is_element_path("1..2"));
        assert!(!is_element_path("1.a"));
    }

    #[test]
    fn scripts_take_values_as_arguments_only() {
        for s in [
            KEY_SCRIPT,
            QUIT_SCRIPT,
            WINDOW_SCRIPT,
            SET_WINDOW_SCRIPT,
            TREE_SCRIPT,
            &press_script(),
            &set_text_script(),
        ] {
            assert!(s.starts_with("function run("), "{s}");
        }
        assert!(
            TREE_SCRIPT.contains("if (!secure)"),
            "secure values are never read"
        );
        assert!(set_text_script().contains("if (secure) throw"));
        assert_eq!(
            Permission::from_id("screen_recording"),
            Some(Permission::ScreenRecording)
        );
    }

    #[test]
    fn a_hung_command_is_stopped() {
        let t = Instant::now();
        assert!(run("/bin/sleep", &["5"], Duration::from_millis(200)).is_err());
        assert!(t.elapsed() < Duration::from_secs(2));
        assert_eq!(run("/bin/echo", &["hi"], SHORT).unwrap(), "hi");
    }

    /// Reads only, no prompt: `cargo test -p markon-platform -- --ignored real_mac_reads`.
    #[test]
    #[ignore]
    fn real_mac_reads_shortcuts_volume_and_the_screen_size() {
        shortcuts(ShortcutCommand::List).unwrap();
        let v = system(SystemCommand::Status).unwrap();
        assert!(v.contains(','), "{v}");
        assert!(parse_frame(&apps(AppCommand::Screen).unwrap()).is_some());
    }

    /// Drives System Events: needs Automation and Accessibility for the app running the test, and may
    /// ask for them. `cargo test -p markon-platform -- --ignored real_mac_drives`.
    #[test]
    #[ignore]
    fn real_mac_drives_the_front_window() {
        let v = apps(AppCommand::List).unwrap();
        assert!(v.contains("\"apps\""), "{v}");
        let w = apps(AppCommand::Window(None)).unwrap();
        assert!(w.contains('\t'), "{w}");
        if has_permission(Permission::Accessibility) {
            let t = ax(AxCommand::Tree { app: None, max: 50 }).unwrap();
            assert!(t.contains("\"nodes\""), "{t}");
        }
    }
}
