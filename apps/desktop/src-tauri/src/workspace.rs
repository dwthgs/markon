//! The tool runtime with a HUD permission prompt, agent and result cards, and MCP servers.
use markon_agent::{AgentEvent, BoxFuture};
use markon_tools::cards::{Card, CardKind, CardStatus, now_ms};
use markon_tools::mcp::{McpFile, McpServer};
use markon_tools::permissions::{Decision, Permissions, Prompter, Request};
use markon_tools::{Runtime, Shared};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tokio::sync::oneshot;

pub const EV_CARD: &str = "markon://card";
pub const EV_PERMISSION: &str = "markon://permission";
pub const EV_CARDS_LOAD: &str = "markon://cards-load";
/// A question was answered by voice: the HUD closes its dialog.
pub const EV_PERMISSION_DONE: &str = "markon://permission-answered";
/// An unanswered prompt is a "no".
const ASK_TIMEOUT: Duration = Duration::from_secs(120);

pub struct HudPrompter {
    app: AppHandle,
    next: AtomicU64,
    pending: Mutex<HashMap<u64, oneshot::Sender<Decision>>>,
}

impl HudPrompter {
    pub fn new(app: AppHandle) -> Self {
        Self {
            app,
            next: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
        }
    }

    pub fn answer(&self, id: u64, d: Decision) {
        if let Some(tx) = self.pending.lock().expect("pending").remove(&id) {
            let _ = tx.send(d);
        }
    }

    /// Answer every open question by voice, oldest first is the one on screen.
    pub fn answer_spoken(&self, d: Decision) -> bool {
        let ids: Vec<u64> = self
            .pending
            .lock()
            .expect("pending")
            .keys()
            .copied()
            .collect();
        let Some(first) = ids.iter().min().copied() else {
            return false;
        };
        self.answer(first, d);
        let _ = self.app.emit(EV_PERMISSION_DONE, first);
        true
    }

    pub fn waiting(&self) -> bool {
        !self.pending.lock().expect("pending").is_empty()
    }
}

impl Prompter for HudPrompter {
    fn ask(&self, req: Request) -> BoxFuture<'static, Decision> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().expect("pending").insert(id, tx);
        crate::show_window(&self.app);
        let _ = self.app.emit(
            EV_PERMISSION,
            json!({"id": id, "tier": req.tier, "tool": req.tool, "detail": req.detail, "pattern": req.pattern, "always_ok": req.always_ok}),
        );
        Box::pin(async move {
            tokio::time::timeout(ASK_TIMEOUT, rx)
                .await
                .ok()
                .and_then(|r| r.ok())
                .unwrap_or(Decision::Deny)
        })
    }
}

/// What a spoken reply to an open question means. None: it is not an answer.
pub fn spoken_answer(text: &str) -> Option<Decision> {
    let t: String = text
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect();
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.starts_with("always") || t.starts_with("yes always") {
        return Some(Decision::Always);
    }
    let yes = [
        "yes",
        "yeah",
        "yep",
        "send it",
        "yes send it",
        "do it",
        "go ahead",
        "ok",
        "okay",
        "sure",
        "allow",
        "yes please",
        "send",
    ];
    let no = [
        "no",
        "nope",
        "dont",
        "don't",
        "do not",
        "cancel",
        "stop",
        "deny",
        "dont send it",
        "no dont",
        "no thanks",
        "not now",
    ];
    if yes
        .iter()
        .any(|y| t == *y || t.starts_with(&format!("{y} ")))
        && !t.contains("not")
    {
        return Some(Decision::Allow);
    }
    no.iter()
        .any(|n| t == *n || t.starts_with(&format!("{n} ")))
        .then_some(Decision::Deny)
}

/// Builds and remembers the per-turn Agent card and the final Result card.
pub struct TurnCards {
    sink: Arc<dyn Fn(Card) + Send + Sync>,
    agent: Mutex<HashMap<u64, Card>>,
}

impl TurnCards {
    pub fn new(sink: Arc<dyn Fn(Card) + Send + Sync>) -> Self {
        Self {
            sink,
            agent: Mutex::new(HashMap::new()),
        }
    }

    pub fn on_agent(&self, e: &AgentEvent, user_text: &str) {
        let mut cards = self.agent.lock().expect("cards");
        match e {
            AgentEvent::ToolCallStarted {
                turn_id,
                name,
                input,
                ..
            } => {
                let c = cards.entry(*turn_id).or_insert_with(|| {
                    let mut title: String = user_text.chars().take(60).collect();
                    if title.is_empty() {
                        title = "Working".into();
                    }
                    Card::new(format!("agent-{turn_id}"), *turn_id, CardKind::Agent, title).with(
                        CardStatus::Running,
                        json!({"steps": [], "sources": 0, "started_ms": now_ms()}),
                    )
                });
                if let Some(steps) = c.body["steps"].as_array_mut() {
                    steps.push(json!({"name": name, "status": "running"}));
                }
                // Cited sources for the Result card: every page read and every search of the user's data.
                let cited = match name.as_str() {
                    "fetch_page" => input["url"].as_str().map(String::from),
                    "search_my_stuff" => input["query"]
                        .as_str()
                        .map(|q| format!("your documents: \"{q}\"")),
                    "read_file" => input["path"].as_str().map(String::from),
                    _ => None,
                };
                if let Some(s) = cited {
                    let list = c
                        .body
                        .as_object_mut()
                        .map(|o| o.entry("cited").or_insert(json!([])));
                    if let Some(Value::Array(a)) = list
                        && !a.iter().any(|x| x == &json!(s))
                    {
                        a.push(json!(s));
                    }
                }
                c.updated_ms = now_ms();
                (self.sink)(c.clone());
            }
            AgentEvent::ToolCallFinished {
                turn_id, name, ok, ..
            } => {
                if let Some(c) = cards.get_mut(turn_id) {
                    if let Some(step) = c.body["steps"].as_array_mut().and_then(|s| {
                        s.iter_mut()
                            .find(|s| s["name"] == *name && s["status"] == "running")
                    }) {
                        step["status"] = json!(if *ok { "done" } else { "error" });
                    }
                    if *ok && name == "fetch_page" {
                        let n = c.body["sources"].as_u64().unwrap_or(0) + 1;
                        c.body["sources"] = json!(n);
                    }
                    c.updated_ms = now_ms();
                    (self.sink)(c.clone());
                }
            }
            AgentEvent::TurnFinished(t) => {
                if let Some(mut c) = cards.remove(&t.turn_id) {
                    c.status = CardStatus::Done;
                    c.updated_ms = now_ms();
                    (self.sink)(c.clone());
                    // A turn that used tools ends with its answer pinned as a Result card.
                    (self.sink)(
                        Card::new(
                            format!("result-{}", t.turn_id),
                            t.turn_id,
                            CardKind::Result,
                            c.title.clone(),
                        )
                        .with(
                            CardStatus::Done,
                            json!({"text": t.text, "partial": t.partial, "sources": c.body["cited"].clone()}),
                        ),
                    );
                }
            }
            AgentEvent::TurnCancelled { turn_id } => {
                cards.remove(turn_id);
            }
            _ => {}
        }
    }
}

pub fn runtime(
    approved: Vec<PathBuf>,
    prompter: Arc<HudPrompter>,
    cache: &Path,
    sink: Arc<dyn Fn(Card) + Send + Sync>,
) -> Arc<Runtime> {
    let mut rt = Runtime::new(Permissions::new(approved, prompter), sink);
    rt.cache_dir = cache.to_path_buf();
    Arc::new(rt)
}

/// Connect every server in `mcp.json` in the background. Failures show on the server's status, not as crashes.
pub fn start_mcp(rt: Arc<Runtime>, file: &Path, platform: Arc<dyn markon_platform::Platform>) {
    let servers: Vec<Arc<McpServer>> = match McpFile::load(file) {
        Ok(f) => f
            .servers
            .iter()
            .map(|(n, c)| Arc::new(McpServer::from_config(n, c, platform.clone())))
            .collect(),
        Err(e) => {
            eprintln!("[mcp] {e}");
            return;
        }
    };
    rt.set_mcp(servers.clone());
    for s in servers {
        tauri::async_runtime::spawn(async move {
            if let Err(e) = s.connect().await {
                eprintln!("[mcp] {}: {e}", s.name);
            }
        });
    }
}

pub fn mcp_status(rt: &Runtime) -> Value {
    json!(
        rt.mcp
            .read()
            .map(|m| m
                .iter()
                .map(|s| json!({"name": s.name, "status": s.status()}))
                .collect::<Vec<_>>())
            .unwrap_or_default()
    )
}

pub fn shared(rt: &Arc<Runtime>) -> Shared {
    Shared(rt.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use markon_agent::TurnSummary;

    #[test]
    fn a_turn_with_tools_gets_an_agent_card_with_progress_and_a_result_card() {
        let seen = Arc::new(Mutex::new(Vec::<Card>::new()));
        let s = seen.clone();
        let tc = TurnCards::new(Arc::new(move |c| s.lock().unwrap().push(c)));
        let start = |name: &str| AgentEvent::ToolCallStarted {
            turn_id: 3,
            id: name.into(),
            name: name.into(),
            input: json!({"url": "https://example.com/vr"}),
        };
        let finish = |name: &str| AgentEvent::ToolCallFinished {
            turn_id: 3,
            id: name.into(),
            name: name.into(),
            ok: true,
            output: String::new(),
        };
        tc.on_agent(&start("fetch_page"), "State of VR");
        tc.on_agent(&start("read_file"), "State of VR");
        tc.on_agent(&finish("fetch_page"), "State of VR");
        let last = seen.lock().unwrap().last().cloned().unwrap();
        assert_eq!(last.kind, CardKind::Agent);
        assert_eq!(last.title, "State of VR");
        assert_eq!(last.body["steps"].as_array().unwrap().len(), 2);
        assert_eq!(last.body["sources"], 1);
        tc.on_agent(
            &AgentEvent::TurnFinished(TurnSummary {
                turn_id: 3,
                user: "State of VR".into(),
                text: "Here it is.".into(),
                partial: false,
                usage: (1, 1),
                provider: "fake".into(),
                model: "fake".into(),
                timings: Default::default(),
            }),
            "",
        );
        let all = seen.lock().unwrap();
        let result = all.last().unwrap();
        assert_eq!(result.kind, CardKind::Result);
        assert_eq!(result.body["text"], "Here it is.");
        assert_eq!(
            result.body["sources"],
            json!(["https://example.com/vr"]),
            "cited sources ride on the result"
        );
        assert!(
            all.iter()
                .any(|c| c.kind == CardKind::Agent && c.status == CardStatus::Done)
        );
    }

    #[test]
    fn spoken_answers_to_an_open_question() {
        for (t, d) in [
            ("Yes, send it.", Some(Decision::Allow)),
            ("go ahead", Some(Decision::Allow)),
            ("Always yes for Sara", Some(Decision::Always)),
            ("No, don't.", Some(Decision::Deny)),
            ("do not send that", Some(Decision::Deny)),
            ("yes, but do not send it yet", None),
            ("what's the weather", None),
        ] {
            assert_eq!(spoken_answer(t), d, "{t}");
        }
    }

    #[test]
    fn a_turn_without_tools_adds_no_cards() {
        let seen = Arc::new(Mutex::new(0));
        let s = seen.clone();
        let tc = TurnCards::new(Arc::new(move |_| *s.lock().unwrap() += 1));
        tc.on_agent(
            &AgentEvent::TextDelta {
                turn_id: 1,
                text: "hi".into(),
            },
            "hi",
        );
        assert_eq!(*seen.lock().unwrap(), 0);
    }
}
