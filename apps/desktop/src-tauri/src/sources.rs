//! Which sources are switched on, their keychain secrets, Full Disk Access.
use crate::CoreState;
use markon_platform::{MacDb, MacOs, Platform, accounts};
use markon_sources::imap::{Imap, MailAccount};
use markon_sources::mcp::{McpSource, McpSourceConfig};
use markon_sources::telegram::{Telegram, TelegramSource};
use markon_sources::{Source, mac};
use markon_tools::mcp::McpServer;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use tauri::State;

/// Every source is off until the user switches it on.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SourceSettings {
    pub imessage: bool,
    pub calendar: bool,
    pub notes: bool,
    pub github: bool,
    pub mail: Vec<MailAccount>,
    /// MCP servers read as sources (Slack, Notion), by their `mcp.json` name.
    pub mcp: Vec<McpSourceConfig>,
    pub telegram: bool,
    /// From my.telegram.org. The matching hash is in the keychain.
    pub telegram_api_id: Option<i32>,
}

fn secret(platform: &Arc<MacOs>, account: String) -> Arc<dyn Fn() -> Option<String> + Send + Sync> {
    let p = platform.clone();
    Arc::new(move || p.secret_get(&account).ok().flatten())
}

pub fn build(
    s: &SourceSettings,
    platform: &Arc<MacOs>,
    servers: &[Arc<McpServer>],
    telegram: Option<Arc<Telegram>>,
) -> Vec<Arc<dyn Source>> {
    let home = markon_platform::home();
    let mut v: Vec<Arc<dyn Source>> = Vec::new();
    if s.imessage {
        v.push(Arc::new(mac::Messages {
            db: markon_platform::mac_db(&home, MacDb::Messages),
            contacts: markon_platform::contacts_dbs(&home),
        }));
    }
    if s.calendar {
        v.push(Arc::new(mac::Calendar { db: markon_platform::mac_db(&home, MacDb::Calendar) }));
    }
    if s.notes {
        v.push(Arc::new(mac::Notes { db: markon_platform::mac_db(&home, MacDb::Notes) }));
    }
    for a in s.mail.iter().filter(|a| a.enabled) {
        v.push(Arc::new(Imap::new(a.clone(), secret(platform, a.secret_account()))));
    }
    if s.github {
        v.push(Arc::new(markon_sources::github::GitHub::new(secret(platform, accounts::GITHUB.into()))));
    }
    if let (true, Some(t)) = (s.telegram, telegram) {
        v.push(Arc::new(TelegramSource(t)));
    }
    for cfg in &s.mcp {
        if let Some(server) = servers.iter().find(|x| x.name == cfg.server) {
            v.push(Arc::new(McpSource { cfg: cfg.clone(), server: server.clone() }));
        }
    }
    v
}

/// Rebuild the live list after a settings change.
pub fn refresh(core: &crate::core::Core) {
    let s = core.settings.lock().expect("settings").sources.clone();
    let servers = core.tools.mcp.read().map(|m| m.clone()).unwrap_or_default();
    let tg = core.telegram.lock().expect("telegram").clone();
    *core.sources.write().expect("sources") = build(&s, &core.platform, &servers, tg);
}

/// Connect Telegram with the saved id, hash and session. Must run inside the async runtime.
pub fn start_telegram(core: &crate::core::Core) -> Option<Arc<Telegram>> {
    let id = core.settings.lock().expect("settings").sources.telegram_api_id?;
    let hash = core.platform.secret_get(accounts::TELEGRAM_API_HASH).ok().flatten()?;
    let saved = core.platform.secret_get(accounts::TELEGRAM_SESSION).ok().flatten();
    let p = core.platform.clone();
    let store: markon_sources::telegram::Store = Arc::new(move |j| {
        if let Err(e) = p.secret_set(accounts::TELEGRAM_SESSION, j) {
            eprintln!("[telegram] could not save the session: {e}");
        }
    });
    let t = Telegram::new(id, hash, saved.as_deref(), store);
    *core.telegram.lock().expect("telegram") = Some(t.clone());
    refresh(core);
    Some(t)
}

fn telegram(core: &crate::core::Core) -> Result<Arc<Telegram>, String> {
    core.telegram.lock().expect("telegram").clone().ok_or_else(|| "save the Telegram API id and hash first".into())
}

/// Save the API id (settings) and hash (keychain) from my.telegram.org, then connect.
#[tauri::command]
pub async fn telegram_setup(core: State<'_, CoreState>, api_id: i32, api_hash: String) -> Result<(), String> {
    let hash = api_hash.trim();
    if api_id <= 0 || hash.len() != 32 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("the API id is a number and the hash is 32 letters and digits".into());
    }
    core.platform.secret_set(accounts::TELEGRAM_API_HASH, hash).map_err(|e| e.to_string())?;
    {
        let mut s = core.settings.lock().expect("settings");
        s.sources.telegram_api_id = Some(api_id);
        s.save(&core.dir).map_err(|e| e.to_string())?;
    }
    start_telegram(&core).map(|_| ()).ok_or_else(|| "could not start Telegram".into())
}

#[tauri::command]
pub async fn telegram_request_code(core: State<'_, CoreState>, phone: String) -> Result<(), String> {
    telegram(&core)?.request_code(phone.trim()).await
}

/// Returns true when the account has a two-step password, asked for next.
#[tauri::command]
pub async fn telegram_sign_in(core: State<'_, CoreState>, code: String) -> Result<bool, String> {
    telegram(&core)?.sign_in(&code).await
}

#[tauri::command]
pub async fn telegram_password(core: State<'_, CoreState>, password: String) -> Result<(), String> {
    telegram(&core)?.check_password(&password).await
}

#[tauri::command]
pub async fn telegram_status(core: State<'_, CoreState>) -> Result<bool, String> {
    let t = core.telegram.lock().expect("telegram").clone();
    Ok(match t {
        Some(t) => t.is_authorized().await,
        None => false,
    })
}

/// For settings: whether Full Disk Access is granted and which secrets are saved.
pub fn view(core: &crate::core::Core) -> Value {
    let s = core.settings.lock().expect("settings").sources.clone();
    let has = |a: &str| core.platform.secret_get(a).ok().flatten().is_some();
    json!({
        "full_disk_access": core.platform.has_full_disk_access(),
        "github_token": has(accounts::GITHUB),
        "mail_passwords": s.mail.iter().map(|a| has(&a.secret_account())).collect::<Vec<_>>(),
    })
}

#[tauri::command]
pub fn open_full_disk_access(core: State<'_, CoreState>) -> Result<(), String> {
    core.platform.open_full_disk_access_settings().map_err(|e| e.to_string())
}

/// Save a source's secret in the keychain: `github` (a token) or `mail` (an app password for `user@host`).
#[tauri::command]
pub fn set_source_secret(
    core: State<'_, CoreState>,
    kind: String,
    user: Option<String>,
    host: Option<String>,
    value: String,
) -> Result<(), String> {
    let account = account_for(&kind, user, host)?;
    let value = value.trim();
    if value.is_empty() || value.len() > 512 {
        return Err("that does not look like a password or token".into());
    }
    core.platform.secret_set(&account, value).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_source_secret(
    core: State<'_, CoreState>,
    kind: String,
    user: Option<String>,
    host: Option<String>,
) -> Result<(), String> {
    let account = account_for(&kind, user, host)?;
    core.platform.secret_delete(&account).map_err(|e| e.to_string())
}

fn account_for(kind: &str, user: Option<String>, host: Option<String>) -> Result<String, String> {
    match kind {
        "github" => Ok(accounts::GITHUB.into()),
        "mail" => {
            let (user, host) = (user.unwrap_or_default(), host.unwrap_or_default());
            if user.is_empty() || host.is_empty() || host.contains(char::is_whitespace) {
                return Err("a mail account needs a user and a host".into());
            }
            Ok(MailAccount { host, port: 993, user, enabled: true }.secret_account())
        }
        _ => Err(format!("unknown source secret {kind}")),
    }
}

/// Sign in to an MCP server that uses OAuth (Slack, Notion): opens the browser.
#[tauri::command]
pub async fn mcp_sign_in(core: State<'_, CoreState>, name: String) -> Result<(), String> {
    let server = core
        .tools
        .mcp
        .read()
        .map_err(|e| e.to_string())?
        .iter()
        .find(|s| s.name == name)
        .cloned()
        .ok_or_else(|| format!("no MCP server {name}"))?;
    server.sign_in(core.platform.clone()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_switched_on_by_default() {
        let s = SourceSettings::default();
        assert!(build(&s, &Arc::new(MacOs::default()), &[], None).is_empty());
    }

    #[test]
    fn secrets_go_to_named_keychain_accounts_only() {
        assert_eq!(account_for("github", None, None).unwrap(), "github-token");
        assert_eq!(
            account_for("mail", Some("me@icloud.com".into()), Some("imap.mail.me.com".into())).unwrap(),
            "mail:me@icloud.com@imap.mail.me.com"
        );
        assert!(account_for("anthropic-api-key", None, None).is_err());
        assert!(account_for("mail", None, Some("x".into())).is_err());
    }
}
