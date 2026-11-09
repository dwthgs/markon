//! Markon desktop shell: owns the window and wires the Rust core to the UI.

/// Dev builds report the window's frame rate here so it can be checked without devtools.
#[tauri::command]
fn report_fps(fps: f64) {
    println!("[fps] {fps:.1}");
}

pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![report_fps])
        .run(tauri::generate_context!())
        .expect("error while running Markon");
}
