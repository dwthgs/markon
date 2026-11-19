//! Route E: the Claude Code CLI already installed and signed in on this Mac. Markon runs
//! `claude -p` with its own tools off and its settings ignored, so the reply is billed to the user's
//! Claude subscription and no API key is needed. Markon's tools travel as text (`text_tools`).
use crate::text_tools::{parse_calls, speakable, tools_prompt};
use crate::*;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

pub const DEFAULT_MODEL: &str = "sonnet";

const TRANSCRIPT_RULES: &str = "\n\n# Conversation\n\nThe conversation so far arrives as <user> and \
<assistant> turns. Write only the assistant's next reply: plain text, plus <tool_call> tags when you call \
a tool. Never write <user> or <assistant> tags yourself.";

/// Where `claude` is: on PATH, else the places its installers put it. An app opened from the Finder
/// does not get the shell's PATH, hence the fixed list.
pub fn find() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let path_dirs = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .unwrap_or_default();
    path_dirs
        .into_iter()
        .chain(
            home.iter()
                .flat_map(|h| [h.join(".local/bin"), h.join(".claude/local")]),
        )
        .chain(["/opt/homebrew/bin".into(), "/usr/local/bin".into()])
        .map(|d| d.join("claude"))
        .find(|p| p.is_file())
}

pub struct ClaudeCode {
    bin: PathBuf,
}

impl ClaudeCode {
    pub fn new(bin: PathBuf) -> Self {
        Self { bin }
    }

    /// A clean environment: only what `claude` needs to find its login in the keychain. Inherited
    /// `ANTHROPIC_*` / `CLAUDE_*` variables would switch it to a key or another host.
    fn command(&self) -> Command {
        let mut c = Command::new(&self.bin);
        c.env_clear();
        for k in ["HOME", "USER", "LOGNAME", "LANG", "TMPDIR"] {
            if let Some(v) = std::env::var_os(k) {
                c.env(k, v);
            }
        }
        let dir = self.bin.parent().unwrap_or(Path::new("/usr/bin"));
        c.env(
            "PATH",
            format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", dir.display()),
        );
        // No project CLAUDE.md or settings from wherever Markon happened to start.
        c.current_dir(std::env::temp_dir());
        c.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        c
    }
}

/// The conversation as tagged turns, for the CLI's single prompt.
pub fn transcript(req: &ChatRequest) -> String {
    let mut s = String::new();
    for m in &req.messages {
        let tag = match m.role {
            Role::User => "user",
            Role::Assistant => "assistant",
        };
        s.push_str(&format!("<{tag}>\n"));
        for b in &m.content {
            match b["type"].as_str().unwrap_or("") {
                "text" => s.push_str(b["text"].as_str().unwrap_or("")),
                "tool_use" => s.push_str(&format!(
                    "\n<tool_call>\n{}\n</tool_call>",
                    json!({"name": b["name"], "arguments": b["input"]})
                )),
                "tool_result" => {
                    let r = match &b["content"] {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    s.push_str(&format!("<tool_response>\n{r}\n</tool_response>\n"));
                }
                _ => {}
            }
        }
        s.push_str(&format!("\n</{tag}>\n"));
    }
    s
}

fn error_from(result: &Value) -> ProviderError {
    let msg: String = result["result"]
        .as_str()
        .unwrap_or("Claude Code failed")
        .chars()
        .take(300)
        .collect();
    match result["api_error_status"].as_u64() {
        Some(401 | 403) => ProviderError::Auth,
        Some(429) => ProviderError::RateLimited {
            retry_after_s: None,
        },
        Some(529 | 503) => ProviderError::Overloaded,
        _ if msg.contains("/login") || msg.to_lowercase().contains("not logged in") => {
            ProviderError::Auth
        }
        Some(s) => ProviderError::Http {
            status: s as u16,
            message: msg,
        },
        None => ProviderError::Network(msg),
    }
}

/// Tool-call ids, unique for this run of the app.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

type Tx = mpsc::Sender<Result<ChatDelta, ProviderError>>;

async fn run(mut cmd: Command, prompt: String, tx: &Tx) -> Result<(), ProviderError> {
    let mut child = cmd
        .spawn()
        .map_err(|e| ProviderError::Network(format!("cannot start Claude Code: {e}")))?;
    let mut stdin = child.stdin.take().expect("piped stdin");
    tokio::spawn(async move {
        let _ = stdin.write_all(prompt.as_bytes()).await;
    });
    let mut stderr = child.stderr.take().expect("piped stderr");
    let err_text = tokio::spawn(async move {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s).await;
        s
    });
    let mut lines = BufReader::new(child.stdout.take().expect("piped stdout")).lines();
    let (mut out, mut sent) = (String::new(), 0);
    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|e| ProviderError::Network(e.to_string()))?
    {
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        match v["type"].as_str() {
            Some("stream_event") => {
                let d = &v["event"]["delta"];
                if v["event"]["type"] == "content_block_delta" && d["type"] == "text_delta" {
                    out.push_str(d["text"].as_str().unwrap_or(""));
                    let safe = speakable(&out);
                    if safe > sent {
                        // Receiver gone (barge-in): dropping the child kills it.
                        if tx
                            .send(Ok(ChatDelta::Text(out[sent..safe].into())))
                            .await
                            .is_err()
                        {
                            return Ok(());
                        }
                        sent = safe;
                    }
                }
            }
            Some("result") if v["is_error"] == true => return Err(error_from(&v)),
            Some("result") => {
                let (text, mut calls) = parse_calls(&out);
                // Seen in practice: the same call written twice in one reply. Run it once.
                let mut seen = Vec::new();
                calls.retain(|c| {
                    let new = !seen.contains(c);
                    seen.push(c.clone());
                    new
                });
                if !text.is_empty() {
                    let _ = tx
                        .send(Ok(ChatDelta::Block(json!({"type": "text", "text": text}))))
                        .await;
                }
                for (name, input) in &calls {
                    let id = format!("cc-{}", NEXT_ID.fetch_add(1, Ordering::Relaxed));
                    let _ = tx
                        .send(Ok(ChatDelta::Block(
                            json!({"type": "tool_use", "id": id, "name": name, "input": input}),
                        )))
                        .await;
                    let _ = tx
                        .send(Ok(ChatDelta::ToolCall {
                            id,
                            name: name.clone(),
                            input: input.clone(),
                        }))
                        .await;
                }
                let u = &v["usage"];
                let n = |k: &str| u[k].as_u64().unwrap_or(0);
                let _ = tx
                    .send(Ok(ChatDelta::Usage {
                        input_tokens: n("input_tokens")
                            + n("cache_read_input_tokens")
                            + n("cache_creation_input_tokens"),
                        output_tokens: n("output_tokens"),
                    }))
                    .await;
                let stop = if calls.is_empty() {
                    StopReason::EndTurn
                } else {
                    StopReason::ToolUse
                };
                let _ = tx.send(Ok(ChatDelta::Stop(stop))).await;
                return Ok(());
            }
            _ => {}
        }
    }
    let status = child.wait().await.ok();
    let err = err_text.await.unwrap_or_default();
    Err(ProviderError::Network(format!(
        "Claude Code stopped without answering ({}): {}",
        status.map(|s| s.to_string()).unwrap_or_default(),
        err.trim().chars().take(300).collect::<String>()
    )))
}

impl ModelProvider for ClaudeCode {
    fn route(&self) -> &'static str {
        "claudecode"
    }
    fn stream(&self, req: ChatRequest) -> DeltaStream {
        let (tx, rx) = mpsc::channel(64);
        let mut system = req.system.trim().to_string();
        if !req.tools.is_empty() {
            system.push_str(&tools_prompt(&req.tools));
        }
        system.push_str(TRANSCRIPT_RULES);
        let model = if req.model.is_empty() {
            DEFAULT_MODEL.to_string()
        } else {
            req.model.clone()
        };
        let mut cmd = self.command();
        cmd.args(["-p", "--output-format", "stream-json", "--verbose"])
            .args(["--include-partial-messages", "--tools", ""])
            .args(["--setting-sources", "", "--strict-mcp-config"])
            .args(["--disable-slash-commands", "--no-session-persistence"])
            .args(["--model", &model, "--system-prompt", &system]);
        let prompt = transcript(&req);
        tokio::spawn(async move {
            if let Err(e) = run(cmd, prompt, &tx).await {
                let _ = tx.send(Err(e)).await;
            }
        });
        rx
    }
    fn models(&self) -> Vec<ModelInfo> {
        [
            ("sonnet", "Sonnet (Claude Code)"),
            ("opus", "Opus (Claude Code)"),
            ("haiku", "Haiku (Claude Code)"),
        ]
        .into_iter()
        .map(|(id, name)| ModelInfo {
            id: id.into(),
            name: name.into(),
        })
        .collect()
    }
    fn default_model(&self) -> String {
        DEFAULT_MODEL.into()
    }
    fn validate(&self) -> BoxFuture<'_, Result<Account, ProviderError>> {
        Box::pin(async move {
            let out = self
                .command()
                .args(["auth", "status"])
                .stdin(Stdio::null())
                .output()
                .await
                .map_err(|e| ProviderError::Network(format!("cannot start Claude Code: {e}")))?;
            let v: Value = serde_json::from_slice(&out.stdout)
                .map_err(|e| ProviderError::Parse(format!("claude auth status: {e}")))?;
            if v["loggedIn"] != true {
                return Err(ProviderError::Auth);
            }
            Ok(Account {
                route: "claudecode".into(),
                plan: v["subscriptionType"]
                    .as_str()
                    .map(|p| format!("Claude {p} plan, through Claude Code")),
                balance: None,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_transcript_keeps_turns_calls_and_results() {
        let req = ChatRequest {
            messages: vec![
                Message::user_text("weather?"),
                Message {
                    role: Role::Assistant,
                    content: vec![
                        json!({"type": "tool_use", "id": "1", "name": "fetch_page", "input": {"url": "x"}}),
                    ],
                },
                Message::tool_results(vec![ToolResult {
                    id: "1".into(),
                    content: "Sunny".into(),
                    is_error: false,
                }]),
            ],
            ..Default::default()
        };
        let t = transcript(&req);
        assert!(t.starts_with("<user>\nweather?\n</user>\n<assistant>\n"));
        assert!(t.contains(
            "<tool_call>\n{\"arguments\":{\"url\":\"x\"},\"name\":\"fetch_page\"}\n</tool_call>"
        ));
        assert!(t.ends_with("<user>\n<tool_response>\nSunny\n</tool_response>\n\n</user>\n"));
    }
}
