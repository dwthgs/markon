//! Markon desktop shell: owns the window, tray, menu and hotkey, and wires the Rust core to the UI.
mod core;
mod ptt;
mod remember;
mod settings;
mod setup;
mod sources;
mod tray;
mod voice_commands;
mod workspace;

use crate::core::Core;
use markon_agent::{ToolCall, UserTurn};
use markon_platform::Platform;
use markon_tools::permissions::Decision;
use std::sync::Arc;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

pub const EV_MENU: &str = "markon://menu";

pub fn show_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.set_focus();
    }
}

pub(crate) type CoreState = Arc<Core>;

/// Swap the push-to-talk shortcut. The old one stays if the new one cannot be registered.
pub(crate) fn register_hotkey(app: &AppHandle, old: &str, new: &str) -> Result<(), String> {
    let next: Shortcut = new
        .parse()
        .map_err(|e| format!("{new} is not a valid shortcut: {e}"))?;
    let gs = app.global_shortcut();
    if let Ok(prev) = old.parse::<Shortcut>() {
        let _ = gs.unregister(prev);
    }
    gs.register(next).map_err(|e| {
        if let Ok(prev) = old.parse::<Shortcut>() {
            let _ = gs.register(prev);
        }
        format!("cannot use {new}: {e}")
    })
}

#[tauri::command]
fn status(core: State<'_, CoreState>) -> core::Status {
    core.status()
}

#[tauri::command]
async fn submit_text(core: State<'_, CoreState>, text: String) -> Result<(), String> {
    let text = text.trim().to_string();
    if text.is_empty() || text.len() > 20_000 {
        return Err("message is empty or too long".into());
    }
    let c = core.inner().clone();
    tauri::async_runtime::spawn(async move { c.submit(UserTurn::text(&text)).await });
    Ok(())
}

#[tauri::command]
fn cancel(core: State<'_, CoreState>) {
    core.stop();
}

#[tauri::command]
fn hide_window(core: State<'_, CoreState>) {
    core.hide();
}

/// Toggle macOS full screen on whichever display the window is on now.
#[tauri::command]
fn toggle_full_screen(window: tauri::WebviewWindow) -> Result<(), String> {
    let on = window.is_fullscreen().map_err(|e| e.to_string())?;
    window.set_fullscreen(!on).map_err(|e| e.to_string())
}

/// The Spotify card's buttons and its progress (polled while the card is on screen).
#[tauri::command]
async fn spotify_control(action: String) -> Result<serde_json::Value, String> {
    use markon_platform::{Platform, SpotifyCommand};
    let np = tauri::async_runtime::spawn_blocking(move || {
        markon_platform::MacOs::default().spotify(match action.as_str() {
            "play" => SpotifyCommand::Play,
            "pause" => SpotifyCommand::Pause,
            "next" => SpotifyCommand::Next,
            "previous" => SpotifyCommand::Previous,
            _ => SpotifyCommand::Status,
        })
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    Ok(
        serde_json::json!({"kind": "music", "track": np.track, "artist": np.artist, "album": np.album,
        "artwork": np.artwork, "state": np.state, "position_s": np.position_s, "duration_s": np.duration_s}),
    )
}

#[tauri::command]
fn quit_app(app: AppHandle) {
    app.exit(0);
}

#[tauri::command]
fn open_mic_settings(core: State<'_, CoreState>) -> Result<(), String> {
    core.platform.open_mic_settings().map_err(|e| e.to_string())
}

#[tauri::command]
fn answer_permission(core: State<'_, CoreState>, id: u64, decision: Decision) {
    core.prompter.answer(id, decision);
}

/// The Camera control: the user picks a window to capture (asks first), shown as a card.
#[tauri::command]
async fn capture_window(core: State<'_, CoreState>) -> Result<(), String> {
    let id = format!("camera-{}", markon_tools::cards::now_ms());
    core.tools
        .call(ToolCall {
            id,
            name: "screenshot_window".into(),
            input: serde_json::json!({}),
            turn_id: 0,
            tainted: false,
        })
        .await
        .map(|_| ())
}

#[tauri::command]
fn mcp_status(core: State<'_, CoreState>) -> serde_json::Value {
    workspace::mcp_status(&core.tools)
}

#[tauri::command]
fn history_list(core: State<'_, CoreState>) -> Result<Vec<markon_memory::Conversation>, String> {
    core.memory.conversations(50).map_err(|e| e.to_string())
}

#[tauri::command]
fn open_conversation(core: State<'_, CoreState>, id: i64) -> Result<usize, String> {
    core.open_conversation(id)
}

/// "Bring back [topic]".
#[tauri::command]
fn bring_back(core: State<'_, CoreState>, topic: String) -> Result<usize, String> {
    match core
        .memory
        .find_conversation(&topic)
        .map_err(|e| e.to_string())?
    {
        Some(id) => core.open_conversation(id),
        None => {
            core.say(&format!("I couldn't find anything about {topic}."));
            Ok(0)
        }
    }
}

/// Dev builds report the window's frame rate here so it can be checked without devtools.
#[tauri::command]
fn report_fps(fps: f64) {
    println!("[fps] {fps:.1}");
}

fn menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    // Quit and Settings go through the UI: quit asks for confirmation, settings opens in the HUD.
    let settings = MenuItem::with_id(app, "settings", "Settings…", true, Some("CmdOrCtrl+,"))?;
    let quit = MenuItem::with_id(app, "quit", "Quit Markon…", true, Some("CmdOrCtrl+Q"))?;
    let app_menu = Submenu::with_items(
        app,
        "Markon",
        true,
        &[&settings, &PredefinedMenuItem::separator(app)?, &quit],
    )?;
    // Text fields need the standard Edit items on macOS for copy and paste to work.
    let edit = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;
    Menu::with_items(app, &[&app_menu, &edit])
}

pub fn run() {
    tauri::Builder::default()
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    let core = app.state::<CoreState>();
                    match event.state() {
                        ShortcutState::Pressed => core.ptt_pressed(),
                        ShortcutState::Released => core.ptt_released(),
                    }
                })
                .build(),
        )
        .menu(menu)
        .on_menu_event(|app, e| {
            let _ = app.emit(EV_MENU, e.id().as_ref());
        })
        .setup(|app| {
            let core = Core::start(app.handle().clone());
            let hotkey = core.settings.lock().expect("settings").hotkey.clone();
            app.manage(core);
            tray::build(app.handle())?;
            match hotkey.parse::<Shortcut>() {
                Ok(s) => {
                    if let Err(e) = app.global_shortcut().register(s) {
                        eprintln!("[hotkey] could not register {hotkey}: {e}");
                    }
                }
                Err(e) => eprintln!("[hotkey] invalid {hotkey}: {e}"),
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            status,
            submit_text,
            cancel,
            hide_window,
            toggle_full_screen,
            spotify_control,
            quit_app,
            open_mic_settings,
            answer_permission,
            capture_window,
            mcp_status,
            history_list,
            open_conversation,
            bring_back,
            setup::settings_get,
            setup::settings_set,
            setup::set_secret,
            setup::delete_secret,
            setup::validate_route,
            setup::preview_voice,
            setup::pick_folder,
            setup::request_mic,
            setup::export_memory,
            setup::wipe_all,
            setup::approve_skill,
            setup::suggest_improvements,
            setup::approve_proposal,
            setup::reject_proposal,
            setup::download_builtin_model,
            setup::download_embedder,
            sources::open_full_disk_access,
            sources::set_source_secret,
            sources::delete_source_secret,
            sources::mcp_sign_in,
            sources::telegram_setup,
            sources::telegram_request_code,
            sources::telegram_sign_in,
            sources::telegram_password,
            sources::telegram_status,
            sources::forget_approvals,
            report_fps
        ])
        .run(tauri::generate_context!())
        .expect("error while running Markon");
}
