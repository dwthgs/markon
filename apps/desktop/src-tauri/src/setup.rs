//! Onboarding and settings: everything goes to settings.json except secrets (keychain).
use crate::CoreState;
use crate::settings::{Route, Settings};
use markon_audio::Voice;
use markon_platform::{Platform, accounts};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::PathBuf;
use tauri::State;

#[derive(Serialize)]
pub struct SettingsView {
    settings: Settings,
    keys: Value,
    models: Vec<markon_provider::ModelInfo>,
    default_model: String,
    mcp: Value,
    verified_mcp: Value,
    usage_today: Vec<markon_memory::Usage>,
    sources: Value,
    automation: Value,
    local: Value,
    endpoints: Vec<Value>,
    data_dir: String,
}

fn account(route: Route) -> Option<&'static str> {
    match route {
        Route::Anthropic => Some(accounts::ANTHROPIC),
        Route::OpenRouter => Some(accounts::OPENROUTER),
        Route::Relay | Route::Local | Route::BuiltIn => None,
    }
}

#[tauri::command]
pub fn settings_get(core: State<'_, CoreState>) -> SettingsView {
    let s = core.settings.lock().expect("settings").clone();
    let has = |a| core.platform.secret_get(a).ok().flatten().is_some();
    let p = core.agent.provider();
    let model_host = match s.route {
        Route::Anthropic => "api.anthropic.com".to_string(),
        Route::OpenRouter => "openrouter.ai".to_string(),
        Route::Local => format!("{} (this Mac)", s.local_url),
        Route::BuiltIn => "none: the built-in model runs on this Mac".into(),
        Route::Relay => {
            if s.relay_url.is_empty() {
                "free-trial relay (not available in this build)".into()
            } else {
                s.relay_url.clone()
            }
        }
    };
    let mut endpoints = vec![
        json!({"name": "Model", "host": model_host, "on": true, "note": "Needed to answer you."}),
        json!({"name": "Model downloads", "host": "huggingface.co, raw.githubusercontent.com", "on": true, "note": "Only when a pinned model file is missing."}),
        json!({"name": "Updates", "host": "none", "on": false, "note": "Not in this build."}),
        json!({"name": "Crash reports", "host": "none", "on": false, "note": "Not in this build; always opt-in."}),
    ];
    for m in core
        .tools
        .mcp
        .read()
        .map(|m| m.iter().map(|s| s.name.clone()).collect::<Vec<_>>())
        .unwrap_or_default()
    {
        endpoints.push(json!({"name": format!("MCP: {m}"), "host": "from ~/.markon/mcp.json", "on": true, "note": "Remove it from mcp.json to turn it off."}));
    }
    SettingsView {
        keys: json!({"anthropic": has(accounts::ANTHROPIC), "openrouter": has(accounts::OPENROUTER)}),
        models: p.models(),
        default_model: p.default_model(),
        mcp: crate::workspace::mcp_status(&core.tools),
        verified_mcp: serde_json::from_str(markon_tools::mcp::VERIFIED).unwrap_or_default(),
        usage_today: core.memory.usage_today().unwrap_or_default(),
        sources: crate::sources::view(&core),
        automation: automation(&core),
        local: local(&core, &s),
        endpoints,
        data_dir: core.dir.display().to_string(),
        settings: s,
    }
}

#[derive(Deserialize, Default)]
pub struct Patch {
    voice: Option<Voice>,
    route: Option<Route>,
    model: Option<Option<String>>,
    hotkey: Option<String>,
    approved_folders: Option<Vec<PathBuf>>,
    onboarded: Option<bool>,
    reduced_motion: Option<bool>,
    sources: Option<crate::sources::SourceSettings>,
    watch: Option<markon_sources::watch::WatchRules>,
    local_url: Option<String>,
    local_model: Option<Option<String>>,
}

/// The local routes as settings shows them: this Mac's memory, models that fit, the built-in file.
fn local(core: &crate::core::Core, s: &Settings) -> Value {
    let ram = markon_platform::total_memory_gb();
    let entry = markon_audio::manifest::Manifest::bundled();
    let builtin = entry.entry(markon_llm::MODEL_ID).ok();
    json!({
        "ram_gb": ram.round(),
        "fit": markon_provider::local::models_that_fit_in(ram),
        "model": crate::core::local_model(s),
        "builtin_ready": builtin.is_some_and(|e| core.dir.join("models").join(&e.path).exists()),
        "builtin_mb": builtin.map(|e| e.size / 1_000_000).unwrap_or(0),
        "chunks": core.memory.chunk_count().unwrap_or(0),
        "meaning_search": core.embed.read().map(|e| e.is_some()).unwrap_or(false),
    })
}

/// Download the meaning-search model (23 MB) and its vocabulary, pinned by SHA-256, then load it.
#[tauri::command]
pub async fn download_embedder(core: State<'_, CoreState>) -> Result<(), String> {
    let m = markon_audio::manifest::Manifest::bundled();
    let dir = core.dir.join("models");
    for id in [markon_llm::embed::MODEL_ID, markon_llm::embed::VOCAB_ID] {
        let e = m.entry(id).map_err(|e| e.to_string())?.clone();
        if e.verify(&dir).is_err() {
            e.download(&dir, |_, _| {})
                .await
                .map_err(|e| e.to_string())?;
        }
    }
    let c = core.inner().clone();
    tauri::async_runtime::spawn_blocking(move || c.load_embedder())
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Download the built-in model, checked against its pinned SHA-256. Progress shows in
/// the status line.
#[tauri::command]
pub async fn download_builtin_model(core: State<'_, CoreState>) -> Result<(), String> {
    let m = markon_audio::manifest::Manifest::bundled();
    let e = m
        .entry(markon_llm::MODEL_ID)
        .map_err(|e| e.to_string())?
        .clone();
    let dir = core.dir.join("models");
    let c = core.inner().clone();
    let mut last = 0;
    e.download(&dir, |got, total| {
        let pct = got * 100 / total.max(1);
        if pct >= last + 5 {
            last = pct;
            c.note_public(format!("Downloading the built-in model ({pct}%)"));
        }
    })
    .await
    .map_err(|e| e.to_string())?;
    core.note_public(String::new());
    crate::sources::refresh(&core);
    Ok(())
}

/// Agent files and skills as settings shows them, with the files that did not parse.
fn automation(core: &crate::core::Core) -> Value {
    let (agents, agent_problems) = core.agent_files();
    let (skills, skill_problems) = core.skills.all();
    json!({
        "agents_dir": core.dir.join("agents"),
        "skills_dir": core.skills.root,
        "agents": agents.iter().map(|a| json!({"name": a.name, "schedule": a.schedule, "tools": a.tools, "enabled": a.enabled})).collect::<Vec<_>>(),
        "agent_problems": agent_problems,
        "skills": skills.iter().map(|s| json!({"name": s.name, "description": s.description, "trust": s.trust})).collect::<Vec<_>>(),
        "skill_problems": skill_problems,
    })
}

/// The user approves an unreviewed skill as it is now.
#[tauri::command]
pub fn approve_skill(core: State<'_, CoreState>, name: String) -> Result<(), String> {
    core.skills.approve(&name)
}

/// Apply and save a change. Takes effect immediately: the next turn uses the new route (no restart).
#[tauri::command]
pub fn settings_set(
    core: State<'_, CoreState>,
    app: tauri::AppHandle,
    patch: Patch,
) -> Result<(), String> {
    let mut s = core.settings.lock().expect("settings").clone();
    if let Some(v) = patch.voice {
        if core.audio.is_ready() {
            core.audio
                .set_voice(&core.dir.join("models"), v)
                .map_err(|e| e.to_string())?;
        }
        s.voice = Some(v);
    }
    let route_changed = patch.route.is_some_and(|r| r != s.route);
    if let Some(r) = patch.route {
        s.route = r;
    }
    if let Some(m) = patch.model {
        s.model = m;
    } else if route_changed {
        s.model = None;
    }
    if let Some(h) = patch.hotkey {
        crate::register_hotkey(&app, &s.hotkey, &h)?;
        s.hotkey = h;
    }
    if let Some(f) = patch.approved_folders {
        let f: Vec<PathBuf> = f
            .into_iter()
            .filter(|p| p.is_absolute() && p.is_dir())
            .collect();
        core.tools.perms.set_approved(f.clone());
        // A folder taken off the list is taken out of the index too.
        for gone in s.approved_folders.iter().filter(|g| !f.contains(g)) {
            let _ = core.memory.unindex_where(&gone.to_string_lossy());
        }
        s.approved_folders = f;
    }
    if let Some(o) = patch.onboarded {
        s.onboarded = o;
    }
    if let Some(r) = patch.reduced_motion {
        s.reduced_motion = r;
    }
    if let Some(w) = patch.watch {
        s.watch = w;
    }
    let local_changed = patch.local_url.is_some() || patch.local_model.is_some();
    if let Some(u) = patch.local_url {
        let u = u.trim().to_string();
        if !(u.starts_with("http://127.0.0.1")
            || u.starts_with("http://localhost")
            || u.starts_with("https://"))
        {
            return Err(
                "a local model server is http://127.0.0.1:<port> or http://localhost:<port>".into(),
            );
        }
        s.local_url = u;
    }
    if let Some(m) = patch.local_model {
        s.local_model = m.filter(|m| !m.trim().is_empty());
    }
    let sources_changed = patch.sources.is_some();
    if let Some(src) = patch.sources {
        s.sources = src;
    }
    s.save(&core.dir).map_err(|e| e.to_string())?;
    core.agent
        .set_provider(crate::core::provider_for(&s, core.platform.clone()));
    core.agent.set_model(s.model.clone());
    *core.settings.lock().expect("settings") = s;
    // The private route follows the local settings too.
    if sources_changed || local_changed || route_changed {
        crate::sources::refresh(&core);
    }
    core.emit_status();
    Ok(())
}

/// Stores a key in the macOS keychain. The value never touches disk or logs.
#[tauri::command]
pub fn set_secret(core: State<'_, CoreState>, route: Route, value: String) -> Result<(), String> {
    let value = value.trim();
    let a = account(route).ok_or("this route has no key")?;
    if value.is_empty() || value.len() > 512 || value.chars().any(char::is_whitespace) {
        return Err("that does not look like an API key".into());
    }
    core.platform
        .secret_set(a, value)
        .map_err(|e| e.to_string())?;
    core.emit_status();
    Ok(())
}

#[tauri::command]
pub fn delete_secret(core: State<'_, CoreState>, route: Route) -> Result<(), String> {
    let a = account(route).ok_or("this route has no key")?;
    core.platform.secret_delete(a).map_err(|e| e.to_string())?;
    core.emit_status();
    Ok(())
}

/// Checks the route's key (or trial) and reports plan or balance when the service exposes it.
#[tauri::command]
pub async fn validate_route(
    core: State<'_, CoreState>,
    route: Route,
) -> Result<markon_provider::Account, String> {
    let mut s = core.settings.lock().expect("settings").clone();
    s.route = route;
    let p = crate::core::provider_for(&s, core.platform.clone());
    p.validate().await.map_err(|e| e.to_string())
}

/// Plays a short sample in the given voice, then switches back to the chosen one.
#[tauri::command]
pub async fn preview_voice(core: State<'_, CoreState>, voice: Voice) -> Result<(), String> {
    let dir = core.dir.join("models");
    core.audio
        .set_voice(&dir, voice)
        .map_err(|e| e.to_string())?;
    core.audio.cancel_speech();
    core.audio.begin_reply();
    core.audio
        .speak(&format!(
            "Hi, I'm {}. I'll be the voice you hear every time.",
            voice.name()
        ))
        .map_err(|e| e.to_string())?;
    // The synthesis has been queued with this voice; the setting stays unchanged until the user chooses.
    let chosen = core
        .settings
        .lock()
        .expect("settings")
        .voice
        .unwrap_or(Voice::Markon);
    let audio = core.audio.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(4)).await;
        let _ = audio.set_voice(&dir, chosen);
    });
    Ok(())
}

/// Native folder picker (AppleScript `choose folder`, no extra dependency).
#[tauri::command]
pub async fn pick_folder() -> Result<Option<String>, String> {
    let out = tokio::process::Command::new("/usr/bin/osascript")
        .args([
            "-e",
            "POSIX path of (choose folder with prompt \"Choose a folder Markon may use\")",
        ])
        .output()
        .await
        .map_err(|e| e.to_string())?;
    let p = String::from_utf8_lossy(&out.stdout)
        .trim()
        .trim_end_matches('/')
        .to_string();
    Ok(if out.status.success() && !p.is_empty() {
        Some(p)
    } else {
        None
    })
}

/// Opens the microphone briefly so macOS asks for permission.
#[tauri::command]
pub async fn request_mic(core: State<'_, CoreState>) -> Result<bool, String> {
    let audio = core.audio.clone();
    tokio::task::spawn_blocking(move || {
        let ok = audio.start_listening().is_ok();
        if ok {
            std::thread::sleep(std::time::Duration::from_millis(300));
            let _ = audio.stop_listening();
        }
        ok
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn export_memory(core: State<'_, CoreState>) -> Result<String, String> {
    let dump = core.memory.export().map_err(|e| e.to_string())?;
    let path = core.dir.join(format!(
        "markon-export-{}.json",
        markon_tools::cards::now_ms()
    ));
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&dump).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(path.display().to_string())
}

/// One button: memory, settings, keychain entries and cache. Models stay (they are public files).
#[tauri::command]
pub fn wipe_all(core: State<'_, CoreState>) -> Result<(), String> {
    core.memory.wipe().map_err(|e| e.to_string())?;
    for a in [
        accounts::ANTHROPIC,
        accounts::OPENROUTER,
        accounts::RELAY_TOKEN,
    ] {
        core.platform.secret_delete(a).map_err(|e| e.to_string())?;
    }
    let _ = std::fs::remove_dir_all(core.dir.join("cache"));
    let fresh = Settings {
        device_id: Some(crate::settings::random_id()),
        ..Settings::default()
    };
    fresh.save(&core.dir).map_err(|e| e.to_string())?;
    core.tools.perms.set_approved(Vec::new());
    *core.settings.lock().expect("settings") = fresh;
    core.emit_status();
    Ok(())
}
