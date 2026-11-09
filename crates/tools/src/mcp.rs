//! MCP client on rmcp. Servers come from `~/.markon/mcp.json` in Claude Desktop's shape.
//! HTTP servers always work; stdio servers only when their runtime is already installed.
use crate::BoxFuture;
use markon_provider::ToolSpec;
use rmcp::model::CallToolRequestParams;
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokio::sync::Mutex as AsyncMutex;

pub type Client = RunningService<RoleClient, ()>;
pub type Connector = Arc<dyn Fn() -> BoxFuture<'static, Result<Client, String>> + Send + Sync>;

const MAX_RESTARTS: u32 = 3;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ServerStatus {
    Starting,
    Online { tools: usize },
    NeedsRuntime { command: String },
    Offline { reason: String },
}

/// One entry of `mcp.json`: either a command (stdio) or a URL (streamable HTTP).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(untagged)]
pub enum ServerConfig {
    Http {
        url: String,
    },
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct McpFile {
    #[serde(rename = "mcpServers", default)]
    pub servers: BTreeMap<String, ServerConfig>,
}

impl McpFile {
    pub fn load(path: &Path) -> Result<Self, String> {
        match std::fs::read_to_string(path) {
            Ok(s) => serde_json::from_str(&s).map_err(|e| format!("{}: {e}", path.display())),
            Err(_) => Ok(Self::default()),
        }
    }
}

/// Is the stdio command on PATH? Markon never bundles Node or Python.
pub fn check_runtime(command: &str) -> ServerStatus {
    let found = if command.contains('/') {
        Path::new(command).is_file()
    } else {
        std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).any(|d| d.join(command).is_file()))
            .unwrap_or(false)
    };
    if found {
        ServerStatus::Starting
    } else {
        ServerStatus::NeedsRuntime {
            command: command.into(),
        }
    }
}

pub fn connector_for(cfg: &ServerConfig) -> Connector {
    match cfg.clone() {
        ServerConfig::Http { url } => Arc::new(move || {
            let url = url.clone();
            Box::pin(async move {
                let t = rmcp::transport::StreamableHttpClientTransport::from_uri(url);
                ().serve(t).await.map_err(|e| e.to_string())
            })
        }),
        ServerConfig::Stdio { command, args, env } => Arc::new(move || {
            let (command, args, env) = (command.clone(), args.clone(), env.clone());
            Box::pin(async move {
                let mut cmd = tokio::process::Command::new(&command);
                cmd.args(&args).envs(&env);
                let t = rmcp::transport::TokioChildProcess::new(cmd).map_err(|e| e.to_string())?;
                ().serve(t).await.map_err(|e| e.to_string())
            })
        }),
    }
}

/// Model-facing tool names: `server__tool`, limited to the characters the API accepts.
fn tool_name(server: &str, tool: &str) -> String {
    let clean = |s: &str| {
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect::<String>()
    };
    let mut n = format!("{}__{}", clean(server), clean(tool));
    n.truncate(64);
    n
}

pub struct McpServer {
    pub name: String,
    connector: Connector,
    client: AsyncMutex<Option<Client>>,
    tools: Mutex<Vec<ToolSpec>>,
    status: Mutex<ServerStatus>,
    restarts: Mutex<u32>,
}

impl McpServer {
    pub fn new(name: &str, connector: Connector) -> Self {
        Self {
            name: name.into(),
            connector,
            client: AsyncMutex::new(None),
            tools: Mutex::new(Vec::new()),
            status: Mutex::new(ServerStatus::Starting),
            restarts: Mutex::new(0),
        }
    }

    pub fn from_config(name: &str, cfg: &ServerConfig) -> Self {
        let s = Self::new(name, connector_for(cfg));
        if let ServerConfig::Stdio { command, .. } = cfg {
            *s.status.lock().expect("status") = check_runtime(command);
        }
        s
    }

    pub fn status(&self) -> ServerStatus {
        self.status.lock().expect("status").clone()
    }

    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools.lock().expect("tools").clone()
    }

    /// Start the server and discover its tools.
    pub async fn connect(&self) -> Result<(), String> {
        if let ServerStatus::NeedsRuntime { command } = self.status() {
            return Err(format!(
                "{} needs `{command}`, which is not installed",
                self.name
            ));
        }
        let client = (self.connector)().await?;
        let tools = client.list_all_tools().await.map_err(|e| e.to_string())?;
        let specs: Vec<ToolSpec> = tools
            .into_iter()
            .map(|t| ToolSpec {
                name: tool_name(&self.name, &t.name),
                description: t.description.map(|d| d.to_string()).unwrap_or_default(),
                input_schema: Value::Object((*t.input_schema).clone()),
            })
            .collect();
        *self.status.lock().expect("status") = ServerStatus::Online { tools: specs.len() };
        *self.tools.lock().expect("tools") = specs;
        *self.client.lock().await = Some(client);
        Ok(())
    }

    /// Test hook: drop the connection as if the server process died.
    pub async fn simulate_crash(&self) {
        if let Some(c) = self.client.lock().await.take() {
            let _ = c.cancel().await;
        }
    }

    pub async fn call(&self, tool: &str, args: Value) -> Result<String, String> {
        loop {
            if let ServerStatus::Offline { reason } = self.status() {
                return Err(format!("{} is offline: {reason}", self.name));
            }
            let attempt = {
                let guard = self.client.lock().await;
                match guard.as_ref() {
                    Some(c) => {
                        let mut p = CallToolRequestParams::new(tool.to_string());
                        if let Value::Object(o) = &args {
                            p = p.with_arguments(o.clone());
                        }
                        Some(c.call_tool(p).await)
                    }
                    None => None,
                }
            };
            match attempt {
                Some(Ok(r)) => {
                    let content = serde_json::to_value(&r.content).unwrap_or(Value::Null);
                    let text = content
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|c| c["text"].as_str())
                                .collect::<Vec<_>>()
                                .join("\n")
                        })
                        .unwrap_or_default();
                    let text = if text.is_empty() {
                        r.structured_content
                            .map(|v| v.to_string())
                            .unwrap_or_default()
                    } else {
                        text
                    };
                    return if r.is_error == Some(true) {
                        Err(text)
                    } else {
                        Ok(text)
                    };
                }
                // Transport gone: the server crashed. Restart up to three times, then mark it offline.
                Some(Err(_)) | None => {
                    let n = {
                        let mut r = self.restarts.lock().expect("restarts");
                        *r += 1;
                        *r
                    };
                    *self.client.lock().await = None;
                    if n > MAX_RESTARTS {
                        *self.status.lock().expect("status") = ServerStatus::Offline {
                            reason: "crashed and could not be restarted".into(),
                        };
                        continue;
                    }
                    if let Err(e) = self.connect().await {
                        *self.status.lock().expect("status") = if n >= MAX_RESTARTS {
                            ServerStatus::Offline { reason: e }
                        } else {
                            ServerStatus::Starting
                        };
                    }
                }
            }
        }
    }
}

/// The verified list shown in setup.
pub const VERIFIED: &str = include_str!("../data/verified-mcp.json");

#[cfg(test)]
mod tests {
    #[test]
    fn verified_list_is_http_only() {
        let v: serde_json::Value = serde_json::from_str(super::VERIFIED).unwrap();
        for s in v["servers"].as_array().unwrap() {
            assert!(s["url"].is_null() || s["url"].as_str().unwrap().starts_with("https://"));
        }
    }

    #[test]
    fn parses_claude_desktop_config() {
        let f: super::McpFile = serde_json::from_str(
            r#"{"mcpServers":{"web":{"url":"https://x.example/mcp"},"fs":{"command":"npx","args":["-y","server"]}}}"#,
        )
        .unwrap();
        assert!(matches!(f.servers["web"], super::ServerConfig::Http { .. }));
        assert!(matches!(f.servers["fs"], super::ServerConfig::Stdio { .. }));
    }
}
