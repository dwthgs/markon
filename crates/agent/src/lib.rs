//! The agent loop: build the prompt from sections, stream the model, run tool calls in
//! parallel, and keep the Idle → Listening → Thinking → Speaking → Idle state machine honest.
use markon_provider::*;
use serde::Serialize;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};

pub use markon_provider::BoxFuture;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentState {
    Idle,
    Listening,
    Thinking,
    Speaking,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TurnSource {
    Voice,
    Text,
    Schedule,
}

#[derive(Debug, Clone)]
pub struct UserTurn {
    pub text: String,
    pub source: TurnSource,
}

impl UserTurn {
    pub fn voice(t: &str) -> Self {
        Self {
            text: t.into(),
            source: TurnSource::Voice,
        }
    }
    pub fn text(t: &str) -> Self {
        Self {
            text: t.into(),
            source: TurnSource::Text,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: Value,
    pub turn_id: u64,
}

/// Native tools and MCP servers behind one list (implemented in `markon-tools`).
pub trait ToolRuntime: Send + Sync {
    fn specs(&self) -> Vec<ToolSpec>;
    fn call(&self, call: ToolCall) -> BoxFuture<'static, Result<String, String>>;
}

/// No tools: talking only.
pub struct NoTools;
impl ToolRuntime for NoTools {
    fn specs(&self) -> Vec<ToolSpec> {
        Vec::new()
    }
    fn call(&self, _: ToolCall) -> BoxFuture<'static, Result<String, String>> {
        Box::pin(async { Err("no tools are available".into()) })
    }
}

/// A piece of the system prompt (persona, memory hits, mode). Returns None to stay out of this turn.
pub trait PromptSection: Send + Sync {
    fn render(&self, turn: &UserTurn) -> Option<String>;
}

pub struct StaticSection(pub String);
impl PromptSection for StaticSection {
    fn render(&self, _: &UserTurn) -> Option<String> {
        Some(self.0.clone())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TurnSummary {
    pub turn_id: u64,
    pub user: String,
    pub text: String,
    /// The stream dropped before the reply finished; `text` is what arrived.
    pub partial: bool,
    pub usage: (u64, u64),
    pub provider: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
/// Serialised as `{ "type": "...", "data": ... }` for the UI.
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum AgentEvent {
    StateChanged(AgentState),
    TextDelta {
        turn_id: u64,
        text: String,
    },
    ToolCallStarted {
        turn_id: u64,
        id: String,
        name: String,
        input: Value,
    },
    ToolCallFinished {
        turn_id: u64,
        id: String,
        name: String,
        ok: bool,
        output: String,
    },
    TurnFinished(TurnSummary),
    TurnCancelled {
        turn_id: u64,
    },
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Concurrency {
    /// A second turn waits for the first.
    Queue,
    /// A second turn cancels the first.
    CancelFirst,
}

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub max_rounds: usize,
    pub turn_limit: Duration,
    /// Give up on a reply when nothing arrives for this long (a stalled connection).
    pub idle_limit: Duration,
    /// Stop waiting for a round of tool calls after this long.
    pub tool_limit: Duration,
    pub max_tool_output: usize,
    pub tries: u32,
    pub retry_base: Duration,
    pub max_tokens: u32,
    pub concurrency: Concurrency,
    pub model: Option<String>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_rounds: 12,
            turn_limit: Duration::from_secs(90),
            idle_limit: Duration::from_secs(45),
            tool_limit: Duration::from_secs(60),
            max_tool_output: 32 * 1024,
            tries: 3,
            retry_base: Duration::from_millis(500),
            max_tokens: 64_000,
            concurrency: Concurrency::Queue,
            model: None,
        }
    }
}

const APOLOGY: &str = "Sorry, I can't reach the model right now. Please try again in a moment.";
const WRAP_UP: &str = "Tool limit for this turn reached. Do not call more tools: wrap up and answer with what you have.";

pub struct Agent {
    provider: RwLock<Arc<dyn ModelProvider>>,
    tools: Arc<dyn ToolRuntime>,
    sections: Vec<Arc<dyn PromptSection>>,
    history: Mutex<Vec<Message>>,
    state: Mutex<AgentState>,
    events: mpsc::UnboundedSender<AgentEvent>,
    cfg: RwLock<AgentConfig>,
    turn_lock: tokio::sync::Mutex<()>,
    cancel: watch::Sender<u64>,
    next_turn: AtomicU64,
}

enum RoundEnd {
    Done {
        partial: bool,
    },
    Tools(Vec<ToolCall>),
    /// A server-side tool loop paused; send the same messages again to resume.
    Paused,
    Cancelled,
    Failed(ProviderError),
}

impl Agent {
    pub fn new(
        provider: Arc<dyn ModelProvider>,
        tools: Arc<dyn ToolRuntime>,
        cfg: AgentConfig,
    ) -> (Self, mpsc::UnboundedReceiver<AgentEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let agent = Self {
            provider: RwLock::new(provider),
            tools,
            sections: Vec::new(),
            history: Mutex::new(Vec::new()),
            state: Mutex::new(AgentState::Idle),
            events: tx,
            cfg: RwLock::new(cfg),
            turn_lock: tokio::sync::Mutex::new(()),
            cancel: watch::channel(0).0,
            next_turn: AtomicU64::new(1),
        };
        (agent, rx)
    }

    pub fn add_section(&mut self, s: Arc<dyn PromptSection>) {
        self.sections.push(s);
    }

    /// Takes effect on the next turn, no restart.
    pub fn set_provider(&self, p: Arc<dyn ModelProvider>) {
        *self.provider.write().expect("provider lock") = p;
    }

    pub fn provider(&self) -> Arc<dyn ModelProvider> {
        self.provider.read().expect("provider lock").clone()
    }

    pub fn set_model(&self, model: Option<String>) {
        self.cfg.write().expect("cfg lock").model = model;
    }

    pub fn history(&self) -> Vec<Message> {
        self.history.lock().expect("history lock").clone()
    }

    pub fn state(&self) -> AgentState {
        *self.state.lock().expect("state lock")
    }

    fn emit(&self, e: AgentEvent) {
        let _ = self.events.send(e);
    }

    /// Move the state machine. Returns false (and emits nothing) for a repeat or an illegal move.
    pub fn set_state(&self, next: AgentState) -> bool {
        use AgentState::*;
        let mut s = self.state.lock().expect("state lock");
        let ok = match (*s, next) {
            (a, b) if a == b => false,
            (_, Idle) => true,
            (Idle | Speaking, Listening) => true,
            (Idle | Listening, Thinking) => true,
            (Thinking, Speaking) => true,
            _ => false,
        };
        if ok {
            *s = next;
            self.emit(AgentEvent::StateChanged(next));
        }
        ok
    }

    /// Cancel the running turn, if any.
    pub fn cancel(&self) {
        self.cancel.send_modify(|v| *v += 1);
    }

    pub async fn submit(&self, turn: UserTurn) {
        if self.cfg.read().expect("cfg lock").concurrency == Concurrency::CancelFirst {
            self.cancel();
        }
        let _guard = self.turn_lock.lock().await;
        let turn_id = self.next_turn.fetch_add(1, Ordering::Relaxed);
        let mut cancelled = self.cancel.subscribe();
        cancelled.mark_unchanged();
        self.set_state(AgentState::Thinking);
        let started = Instant::now();
        let cfg = self.cfg.read().expect("cfg lock").clone();
        let provider = self.provider();
        let current_model = || {
            self.cfg
                .read()
                .expect("cfg lock")
                .model
                .clone()
                .unwrap_or_else(|| provider.default_model())
        };
        let system = self
            .sections
            .iter()
            .filter_map(|s| s.render(&turn))
            .collect::<Vec<_>>()
            .join("\n\n");
        let tools = self.tools.specs();
        // Work on a copy: history is only committed when the turn finishes, so a cancel leaves nothing behind.
        let mut msgs = self.history();
        msgs.push(Message::user_text(&turn.text));
        let mut text = String::new();
        let mut usage = (0, 0);
        let mut rounds = 0;
        let mut partial = false;

        loop {
            let req = ChatRequest {
                model: current_model(),
                system: system.clone(),
                messages: msgs.clone(),
                tools: tools.clone(),
                max_tokens: cfg.max_tokens,
            };
            let end = self
                .round_with_retry(
                    &*provider,
                    req,
                    &cfg,
                    turn_id,
                    &mut msgs,
                    &mut text,
                    &mut usage,
                    &mut cancelled,
                )
                .await;
            match end {
                RoundEnd::Cancelled => return self.cancelled(turn_id),
                RoundEnd::Failed(e) => {
                    self.emit(AgentEvent::Error(e.to_string()));
                    if text.is_empty() {
                        text = APOLOGY.into();
                    } else {
                        partial = true;
                    }
                    break;
                }
                RoundEnd::Done { partial: p } => {
                    partial = p;
                    break;
                }
                RoundEnd::Paused => {
                    rounds += 1;
                    if rounds > cfg.max_rounds || started.elapsed() >= cfg.turn_limit {
                        partial = true;
                        break;
                    }
                }
                RoundEnd::Tools(calls) => {
                    let over = rounds >= cfg.max_rounds || started.elapsed() >= cfg.turn_limit;
                    if over {
                        let mut results = Message::tool_results(
                            calls
                                .iter()
                                .map(|c| ToolResult {
                                    id: c.id.clone(),
                                    content: WRAP_UP.into(),
                                    is_error: true,
                                })
                                .collect(),
                        );
                        results
                            .content
                            .push(json!({"type": "text", "text": WRAP_UP}));
                        msgs.push(results);
                        let req = ChatRequest {
                            model: current_model(),
                            system: system.clone(),
                            messages: msgs.clone(),
                            tools: tools.clone(),
                            max_tokens: cfg.max_tokens,
                        };
                        match self
                            .round_with_retry(
                                &*provider,
                                req,
                                &cfg,
                                turn_id,
                                &mut msgs,
                                &mut text,
                                &mut usage,
                                &mut cancelled,
                            )
                            .await
                        {
                            RoundEnd::Cancelled => return self.cancelled(turn_id),
                            RoundEnd::Done { partial: p } => partial = p,
                            RoundEnd::Failed(_) | RoundEnd::Tools(_) | RoundEnd::Paused => {
                                partial = true
                            }
                        }
                        break;
                    }
                    rounds += 1;
                    let results = tokio::select! {
                        r = self.run_tools(calls, &tools, cfg.max_tool_output, cfg.tool_limit) => r,
                        _ = cancelled.changed() => return self.cancelled(turn_id),
                    };
                    msgs.push(Message::tool_results(results));
                }
            }
        }
        if partial && !text.is_empty() && msgs.last().map(|m| m.role) == Some(Role::User) {
            msgs.push(Message {
                role: Role::Assistant,
                content: vec![json!({"type": "text", "text": text})],
            });
        }
        *self.history.lock().expect("history lock") = msgs;
        self.emit(AgentEvent::TurnFinished(TurnSummary {
            turn_id,
            user: turn.text,
            text,
            partial,
            usage,
            provider: provider.route().into(),
            model: current_model(),
        }));
    }

    fn cancelled(&self, turn_id: u64) {
        self.emit(AgentEvent::TurnCancelled { turn_id });
        // The user interrupted to say something new: the app is already listening, keep it that way.
        if self.state() != AgentState::Listening {
            self.set_state(AgentState::Idle);
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn round_with_retry(
        &self,
        provider: &dyn ModelProvider,
        req: ChatRequest,
        cfg: &AgentConfig,
        turn_id: u64,
        msgs: &mut Vec<Message>,
        text: &mut String,
        usage: &mut (u64, u64),
        cancelled: &mut watch::Receiver<u64>,
    ) -> RoundEnd {
        let mut attempt = 0;
        let mut req = req;
        loop {
            let before = text.len();
            let end = self
                .round(
                    provider,
                    req.clone(),
                    turn_id,
                    msgs,
                    text,
                    usage,
                    cancelled,
                    cfg.idle_limit,
                )
                .await;
            match end {
                // A deprecated model falls back to the route's default, and the user is told.
                RoundEnd::Failed(ProviderError::ModelUnavailable(_))
                    if req.model != provider.default_model() =>
                {
                    let fallback = provider.default_model();
                    self.emit(AgentEvent::Error(format!(
                        "{} is no longer available; using {fallback} instead.",
                        req.model
                    )));
                    self.cfg.write().expect("cfg lock").model = Some(fallback.clone());
                    req.model = fallback;
                }
                RoundEnd::Failed(e)
                    if e.retryable() && attempt + 1 < cfg.tries && text.len() == before =>
                {
                    attempt += 1;
                    let wait = match e {
                        ProviderError::RateLimited {
                            retry_after_s: Some(s),
                        } => Duration::from_secs(s).max(cfg.retry_base),
                        _ => cfg.retry_base * 2u32.pow(attempt - 1),
                    };
                    tokio::select! {
                        _ = tokio::time::sleep(wait) => {}
                        _ = cancelled.changed() => return RoundEnd::Cancelled,
                    }
                }
                other => return other,
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn round(
        &self,
        provider: &dyn ModelProvider,
        req: ChatRequest,
        turn_id: u64,
        msgs: &mut Vec<Message>,
        text: &mut String,
        usage: &mut (u64, u64),
        cancelled: &mut watch::Receiver<u64>,
        idle: Duration,
    ) -> RoundEnd {
        let mut rx = provider.stream(req);
        let mut blocks = Vec::new();
        let mut calls = Vec::new();
        let mut stop = None;
        let mut fresh = true;
        loop {
            let d = tokio::select! {
                d = rx.recv() => d,
                _ = cancelled.changed() => return RoundEnd::Cancelled,
                // A stalled connection would otherwise leave Markon thinking forever.
                _ = tokio::time::sleep(idle) => {
                    return RoundEnd::Failed(ProviderError::Network(format!(
                        "no response for {} s",
                        idle.as_secs()
                    )));
                }
            };
            match d {
                None => break,
                Some(Ok(ChatDelta::Text(mut t))) => {
                    // A round's text follows the previous round's with no separator ("now.In").
                    if std::mem::take(&mut fresh)
                        && !text.is_empty()
                        && !text.ends_with(char::is_whitespace)
                        && !t.starts_with(char::is_whitespace)
                    {
                        t.insert(0, ' ');
                    }
                    text.push_str(&t);
                    self.emit(AgentEvent::TextDelta { turn_id, text: t });
                }
                Some(Ok(ChatDelta::Block(b))) => blocks.push(b),
                Some(Ok(ChatDelta::ToolCall { id, name, input })) => calls.push(ToolCall {
                    id,
                    name,
                    input,
                    turn_id,
                }),
                Some(Ok(ChatDelta::Usage {
                    input_tokens,
                    output_tokens,
                })) => {
                    usage.0 += input_tokens;
                    usage.1 += output_tokens;
                }
                Some(Ok(ChatDelta::Stop(s))) => {
                    stop = Some(s);
                    break;
                }
                Some(Err(e)) => {
                    if !blocks.is_empty() {
                        msgs.push(Message {
                            role: Role::Assistant,
                            content: blocks,
                        });
                    }
                    return RoundEnd::Failed(e);
                }
            }
        }
        let Some(stop) = stop else {
            return RoundEnd::Done { partial: true };
        };
        if !blocks.is_empty() {
            msgs.push(Message {
                role: Role::Assistant,
                content: blocks,
            });
        }
        if stop == StopReason::ToolUse && !calls.is_empty() {
            return RoundEnd::Tools(calls);
        }
        if stop == StopReason::Paused {
            return RoundEnd::Paused;
        }
        if stop == StopReason::Refusal && text.is_empty() {
            text.push_str("I can't help with that one.");
        }
        RoundEnd::Done {
            partial: stop == StopReason::MaxTokens,
        }
    }

    async fn run_tools(
        &self,
        calls: Vec<ToolCall>,
        specs: &[ToolSpec],
        cap: usize,
        limit: Duration,
    ) -> Vec<ToolResult> {
        let deadline = tokio::time::Instant::now() + limit;
        let known: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
        let mut handles = Vec::new();
        for c in calls {
            self.emit(AgentEvent::ToolCallStarted {
                turn_id: c.turn_id,
                id: c.id.clone(),
                name: c.name.clone(),
                input: c.input.clone(),
            });
            let fut = if known.contains(&c.name.as_str()) {
                self.tools.call(c.clone())
            } else {
                let msg = format!(
                    "Unknown tool {}. Available tools: {}",
                    c.name,
                    known.join(", ")
                );
                Box::pin(async move { Err(msg) })
            };
            handles.push((c, tokio::spawn(fut)));
        }
        let mut out = Vec::new();
        for (c, mut h) in handles {
            let r = match tokio::time::timeout_at(deadline, &mut h).await {
                Ok(r) => r.unwrap_or_else(|e| Err(format!("tool crashed: {e}"))),
                Err(_) => {
                    h.abort();
                    Err(format!(
                        "{} took too long (over {} s), so it was stopped",
                        c.name,
                        limit.as_secs()
                    ))
                }
            };
            let (ok, full) = match r {
                Ok(s) => (true, s),
                Err(s) => (false, s),
            };
            self.emit(AgentEvent::ToolCallFinished {
                turn_id: c.turn_id,
                id: c.id.clone(),
                name: c.name.clone(),
                ok,
                output: full.clone(),
            });
            out.push(ToolResult {
                id: c.id,
                content: cap_utf8(&full, cap),
                is_error: !ok,
            });
        }
        out
    }
}

/// Cap text at `cap` bytes on a char boundary, saying how much was cut.
pub fn cap_utf8(s: &str, cap: usize) -> String {
    if s.len() <= cap {
        return s.to_string();
    }
    // Leave room for the note so the whole result stays within `cap`.
    let mut end = cap.saturating_sub(96);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n[truncated: {} of {} bytes shown; the full output is on the card]",
        &s[..end],
        end,
        s.len()
    )
}
