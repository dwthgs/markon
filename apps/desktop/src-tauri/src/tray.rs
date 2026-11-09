//! Menu-bar icon: pre-rendered stills of the Minimal core, one per state.
use markon_agent::AgentState;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter};

const TRAY_ID: &str = "markon";

fn icon(s: AgentState) -> Image<'static> {
    let bytes: &'static [u8] = match s {
        AgentState::Idle => include_bytes!("../icons/tray-idle.png"),
        AgentState::Listening => include_bytes!("../icons/tray-listening.png"),
        AgentState::Thinking => include_bytes!("../icons/tray-thinking.png"),
        AgentState::Speaking => include_bytes!("../icons/tray-speaking.png"),
    };
    Image::from_bytes(bytes).expect("bundled tray image")
}

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Show Markon", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Markon…", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon(AgentState::Idle))
        .tooltip("Markon")
        .menu(&menu)
        .on_menu_event(|app, e| match e.id().as_ref() {
            "show" => crate::show_window(app),
            "quit" => {
                crate::show_window(app);
                let _ = app.emit(
                    crate::voice_commands::EV_COMMAND,
                    serde_json::json!({"command": "quit", "arg": ""}),
                );
            }
            _ => {}
        })
        .build(app)?;
    Ok(())
}

pub fn set_state(app: &AppHandle, s: AgentState) {
    if let Some(t) = app.tray_by_id(TRAY_ID) {
        let _ = t.set_icon(Some(icon(s)));
    }
}
