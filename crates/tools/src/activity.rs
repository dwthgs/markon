//! The activity log and undo. Wraps the whole tool stack: every call that changed
//! something is written to the memory database with its result, and with what undo needs when a tool
//! left that behind (`Runtime::take_undo`). File writes and window moves can be undone; the rest says so.
use crate::Runtime;
use markon_agent::{ToolCall, ToolRuntime};
use markon_memory::Memory;
use markon_memory::activity::Action;
use markon_platform::{AppCommand, Frame};
use markon_provider::{BoxFuture, ToolSpec};
use serde_json::{Value, json};
use std::sync::Arc;

pub struct WithActivity<T> {
    pub inner: T,
    pub rt: Arc<Runtime>,
    pub memory: Arc<Memory>,
}

/// Tools that change something outside Markon. Reads, screen-only actions and the sandbox are left out.
const CHANGES: &[&str] = &[
    "write_file",
    "run_command",
    "open_url",
    "open_app",
    "run_shortcut",
    "focus_app",
    "quit_app",
    "arrange_window",
    "click_in_app",
    "type_in_app",
    "press_keys",
    "browser_open",
    "browser_click",
    "browser_type",
    "send_imessage",
    "send_mail",
    "send_slack",
    "send_telegram",
    "archive_mail",
    "add_event",
    "remind_me",
    "save_skill",
];

fn changes(c: &ToolCall) -> bool {
    match c.name.as_str() {
        // Their status action only reads.
        "system_control" | "spotify" => c.input["action"] != "status",
        n => CHANGES.contains(&n),
    }
}

/// What the action was done to, for the log and for "what did you just do?".
fn target(input: &Value) -> String {
    [
        "path", "name", "app", "to", "url", "title", "command", "action", "keys", "ref",
    ]
    .iter()
    .find_map(|k| match &input[*k] {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(format!("[{n}]")),
        _ => None,
    })
    .unwrap_or_default()
}

fn first_line(s: &str) -> String {
    let line = markon_agent::scrub::scrub(s.lines().next().unwrap_or(""));
    line.chars().take(200).collect()
}

fn ago(at_ms: i64) -> String {
    let now = crate::cards::now_ms() as i64;
    match (now - at_ms).max(0) / 1000 {
        s if s < 60 => format!("{s} s ago"),
        s if s < 3600 => format!("{} min ago", s / 60),
        s if s < 86_400 => format!("{} h ago", s / 3600),
        s => format!("{} days ago", s / 86_400),
    }
}

pub fn specs() -> Vec<ToolSpec> {
    let t = |name: &str, description: &str, props: Value| ToolSpec {
        name: name.into(),
        description: description.into(),
        input_schema: json!({"type": "object", "properties": props, "additionalProperties": false}),
    };
    vec![
        t(
            "recent_activity",
            "What Markon did recently that changed something (files written, apps and windows, \
             messages sent, shortcuts run), newest first, and whether each can be undone. Use it to \
             answer \"what did you just do?\".",
            json!({"limit": {"type": "integer", "minimum": 1, "maximum": 30}}),
        ),
        t(
            "undo_last",
            "Undo the last thing Markon changed, when the user says \"undo that\". Only file writes \
             and window moves can be undone; for anything else it says it cannot.",
            json!({}),
        ),
    ]
}

impl<T: ToolRuntime> ToolRuntime for WithActivity<T> {
    fn specs(&self) -> Vec<ToolSpec> {
        let mut v = self.inner.specs();
        v.extend(specs());
        v
    }

    fn call(&self, c: ToolCall) -> BoxFuture<'static, Result<String, String>> {
        let (rt, memory) = (self.rt.clone(), self.memory.clone());
        match c.name.as_str() {
            "recent_activity" => {
                let n = c.input["limit"].as_u64().unwrap_or(10).clamp(1, 30) as usize;
                return Box::pin(async move { recent(&memory, n) });
            }
            "undo_last" => return Box::pin(async move { undo_last(&rt, &memory).await }),
            _ => {}
        }
        let logged = changes(&c);
        let fut = self.inner.call(c.clone());
        Box::pin(async move {
            let r = fut.await;
            if logged {
                let undo = r.as_ref().ok().and_then(|_| rt.take_undo(&c.id));
                let text = match &r {
                    Ok(s) => first_line(s),
                    Err(e) => format!("failed: {}", first_line(e)),
                };
                if let Err(e) = memory.record_action(
                    &c.name,
                    &target(&c.input),
                    &text,
                    r.is_ok(),
                    undo.as_ref(),
                ) {
                    eprintln!("[activity] {e}");
                }
            }
            r
        })
    }
}

fn recent(memory: &Memory, n: usize) -> Result<String, String> {
    let log = memory.actions(n).map_err(|e| e.to_string())?;
    if log.is_empty() {
        return Ok("Markon has not changed anything yet.".into());
    }
    Ok(log
        .iter()
        .map(|a| {
            let state = match (a.ok, a.undone, a.undo.is_some()) {
                (false, ..) => "it failed",
                (_, true, _) => "undone",
                (_, _, true) => "can be undone",
                _ => "cannot be undone",
            };
            format!(
                "- {}: {} {}: {} ({state})",
                ago(a.at),
                a.tool,
                a.target,
                a.result
            )
        })
        .collect::<Vec<_>>()
        .join("\n"))
}

async fn undo_last(rt: &Runtime, memory: &Memory) -> Result<String, String> {
    let last: Option<Action> = memory
        .actions(50)
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|a| a.ok && !a.undone);
    let a = last.ok_or("there is nothing to undo")?;
    let undo = a.undo.clone().ok_or_else(|| {
        format!(
            "The last thing Markon did ({} {}) cannot be undone. Only file writes and window moves can.",
            a.tool, a.target
        )
    })?;
    let said = match undo["kind"].as_str() {
        Some("file") => {
            let path = undo["path"].as_str().unwrap_or("").to_string();
            // The folder was allowed for the write being undone, so this does not ask again.
            let p = rt.perms.write("undo_last", path.as_ref()).await?;
            match undo["previous"].as_str() {
                Some(text) => {
                    std::fs::write(&p, text).map_err(|e| e.to_string())?;
                    format!("Put {} back the way it was.", p.display())
                }
                None => {
                    std::fs::remove_file(&p).map_err(|e| e.to_string())?;
                    format!("Removed {}, which Markon had created.", p.display())
                }
            }
        }
        Some("window") => {
            let app = undo["app"].as_str().unwrap_or("").to_string();
            let n: Vec<i64> = undo["frame"]
                .as_array()
                .map(|v| v.iter().filter_map(Value::as_i64).collect())
                .unwrap_or_default();
            let [x, y, w, h] = n[..] else {
                return Err("the window's old place was not saved".into());
            };
            let a2 = app.clone();
            rt.mac(move |p| p.apps(AppCommand::SetWindow(&a2, Frame { x, y, w, h })))
                .await?;
            format!("Moved {app}'s window back.")
        }
        _ => return Err("Markon does not know how to undo that".into()),
    };
    memory.mark_undone(a.id).map_err(|e| e.to_string())?;
    Ok(said)
}
