//! Acceptance tests: permission tiers, cards, output caps, MCP restart and offline.
use markon_agent::ToolCall;
use markon_tools::cards::{Card, CardStatus};
use markon_tools::mcp::{Connector, McpServer, ServerStatus};
use markon_tools::permissions::{Decision, Permissions, Prompter, Request, Tier};
use markon_tools::{BoxFuture, Runtime};
use serde_json::json;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Asker {
    answers: Mutex<Vec<Decision>>,
    asked: Mutex<Vec<Request>>,
}
impl Prompter for Asker {
    fn ask(&self, req: Request) -> BoxFuture<'static, Decision> {
        self.asked.lock().unwrap().push(req);
        let d = self.answers.lock().unwrap().pop().unwrap_or(Decision::Deny);
        Box::pin(async move { d })
    }
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("markon-tools-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d.canonicalize().unwrap()
}

struct Setup {
    rt: Runtime,
    asker: Arc<Asker>,
    cards: Arc<Mutex<Vec<(Instant, Card)>>>,
    dir: PathBuf,
}

fn setup(name: &str, answers: Vec<Decision>) -> Setup {
    let dir = tmp(name);
    let asker = Arc::new(Asker {
        answers: Mutex::new(answers),
        ..Default::default()
    });
    let cards = Arc::new(Mutex::new(Vec::new()));
    let c = cards.clone();
    let perms = Permissions::new(vec![dir.clone()], asker.clone());
    let rt = Runtime::new(
        perms,
        Arc::new(move |card| c.lock().unwrap().push((Instant::now(), card))),
    );
    Setup {
        rt,
        asker,
        cards,
        dir,
    }
}

fn call(name: &str, input: serde_json::Value) -> ToolCall {
    ToolCall {
        id: format!("id-{name}"),
        name: name.into(),
        input,
        turn_id: 1,
    }
}

#[tokio::test]
async fn read_is_allowed_inside_approved_folders_without_asking() {
    let s = setup("read", vec![]);
    std::fs::write(s.dir.join("notes.txt"), "hello").unwrap();
    let out =
        s.rt.call(call("read_file", json!({"path": s.dir.join("notes.txt")})))
            .await
            .unwrap();
    assert!(out.contains("hello"));
    assert!(s.asker.asked.lock().unwrap().is_empty());
}

#[tokio::test]
async fn reads_and_writes_outside_approved_folders_are_blocked_and_shown_on_the_card() {
    let s = setup("outside", vec![Decision::Allow]);
    let err =
        s.rt.call(call("read_file", json!({"path": "/etc/hosts"})))
            .await
            .unwrap_err();
    assert!(err.contains("outside the folders"), "{err}");
    let err =
        s.rt.call(call(
            "write_file",
            json!({"path": "/tmp/../etc/x", "content": "x"}),
        ))
        .await
        .unwrap_err();
    assert!(err.contains("outside the folders"), "{err}");
    // Escaping through .. is resolved before the check.
    let sneaky = s.dir.join("../escape.txt");
    assert!(
        s.rt.call(call("write_file", json!({"path": sneaky, "content": "x"})))
            .await
            .is_err()
    );
    let cards = s.cards.lock().unwrap();
    assert!(cards.iter().any(|(_, c)| c.status == CardStatus::Blocked
        && c.body["reason"].as_str().unwrap_or("").contains("outside")));
    assert!(
        s.asker.asked.lock().unwrap().is_empty(),
        "blocked paths never prompt"
    );
}

#[tokio::test]
async fn write_asks_once_per_folder() {
    let s = setup("write", vec![Decision::Allow]);
    for i in 0..3 {
        s.rt.call(call(
            "write_file",
            json!({"path": s.dir.join(format!("f{i}.txt")), "content": "x"}),
        ))
        .await
        .unwrap();
    }
    let asked = s.asker.asked.lock().unwrap();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].tier, Tier::Write);
}

#[tokio::test]
async fn execute_asks_every_time_with_the_exact_command_and_denials_are_remembered() {
    let s = setup(
        "exec",
        vec![Decision::Deny, Decision::Allow, Decision::Allow],
    );
    let out =
        s.rt.call(call("run_command", json!({"command": "echo hi"})))
            .await
            .unwrap();
    assert!(out.contains("hi"));
    s.rt.call(call("run_command", json!({"command": "echo hi"})))
        .await
        .unwrap();
    assert!(
        s.rt.call(call(
            "run_command",
            json!({"command": "rm -rf /tmp/nothing"})
        ))
        .await
        .is_err()
    );
    // Denied once: the same command is refused again without a prompt.
    assert!(
        s.rt.call(call(
            "run_command",
            json!({"command": "rm -rf /tmp/nothing"})
        ))
        .await
        .is_err()
    );
    let asked = s.asker.asked.lock().unwrap();
    assert_eq!(asked.len(), 3);
    assert!(asked.iter().all(|r| r.tier == Tier::Execute));
    assert_eq!(asked[2].detail, "rm -rf /tmp/nothing");
}

#[tokio::test]
async fn every_call_creates_a_card_within_100_ms() {
    let s = setup("cards", vec![]);
    std::fs::write(s.dir.join("a.txt"), "x").unwrap();
    let t = Instant::now();
    s.rt.call(call("list_files", json!({"path": s.dir})))
        .await
        .unwrap();
    let cards = s.cards.lock().unwrap();
    let (at, first) = cards.first().unwrap();
    assert!(*at - t < Duration::from_millis(100));
    assert_eq!(first.status, CardStatus::Running);
    assert!(
        cards
            .iter()
            .any(|(_, c)| c.id == first.id && c.status == CardStatus::Done)
    );
}

#[tokio::test]
async fn the_full_output_stays_on_the_card() {
    let s = setup("big", vec![]);
    std::fs::write(s.dir.join("big.txt"), "y".repeat(50_000)).unwrap();
    let out =
        s.rt.call(call("read_file", json!({"path": s.dir.join("big.txt")})))
            .await
            .unwrap();
    assert!(
        out.len() >= 50_000,
        "the runtime returns everything; the agent caps it at 32 KB"
    );
    let cards = s.cards.lock().unwrap();
    let done = cards
        .iter()
        .rev()
        .find(|(_, c)| c.status == CardStatus::Done)
        .unwrap();
    assert_eq!(done.1.body["preview"].as_str().unwrap().len(), 50_000);
}

#[tokio::test]
async fn a_page_that_never_loads_times_out() {
    let mut s = setup("page", vec![]);
    s.rt.allow_private_hosts = true;
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        let (_c, _) = l.accept().await.unwrap();
        tokio::time::sleep(Duration::from_secs(60)).await;
    });
    let t = Instant::now();
    let err =
        s.rt.fetch_with_timeout(&format!("http://{addr}/"), Duration::from_millis(300))
            .await
            .unwrap_err();
    assert!(err.contains("did not load"), "{err}");
    assert!(t.elapsed() < Duration::from_secs(2));
}

#[tokio::test]
async fn pages_on_this_mac_or_its_network_are_refused() {
    let s = setup("ssrf", vec![]);
    for url in [
        "http://127.0.0.1:9/",
        "http://localhost/",
        "http://169.254.169.254/latest/meta-data/",
        "http://192.168.1.1/",
        "http://[::1]/",
    ] {
        let err = s.rt.call(call("fetch_page", json!({"url": url}))).await.unwrap_err();
        assert!(err.contains("does not fetch"), "{url}: {err}");
    }
}

#[tokio::test]
async fn a_redirect_to_a_private_address_is_refused() {
    let mut s = setup("ssrf-redirect", vec![]);
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut c, _) = l.accept().await.unwrap();
        let mut buf = [0u8; 4096];
        let _ = c.read(&mut buf).await;
        let _ = c
            .write_all(b"HTTP/1.1 302 Found\r\nlocation: http://169.254.169.254/\r\ncontent-length: 0\r\n\r\n")
            .await;
    });
    // The first hop is the local mock (allowed by the test hook); the redirect itself is checked.
    s.rt.allow_private_hosts = true;
    let err = s.rt.fetch_with_timeout(&format!("http://{addr}/"), Duration::from_secs(5)).await.unwrap_err();
    assert!(err.contains("private") || err.contains("redirect"), "{err}");
}

#[tokio::test]
async fn secret_files_are_never_read_even_in_an_approved_folder() {
    let s = setup("secrets", vec![]);
    std::fs::write(s.dir.join(".env"), "ANTHROPIC_API_KEY=sk-ant-x").unwrap();
    std::fs::create_dir_all(s.dir.join(".ssh")).unwrap();
    std::fs::write(s.dir.join(".ssh/id_ed25519"), "-----BEGIN OPENSSH PRIVATE KEY-----").unwrap();
    for p in [".env", ".ssh/id_ed25519"] {
        let path = s.dir.join(p);
        let err = s.rt.call(call("read_file", json!({"path": path}))).await.unwrap_err();
        assert!(err.contains("holds secrets"), "{p}: {err}");
    }
    std::fs::write(s.dir.join("notes.txt"), "fine").unwrap();
    let ok = s.rt.call(call("read_file", json!({"path": s.dir.join("notes.txt")}))).await;
    assert_eq!(ok.unwrap(), "fine");
}

// ---------- MCP ----------

mod server {
    use rmcp::handler::server::router::tool::ToolRouter;
    use rmcp::handler::server::wrapper::Parameters;
    use rmcp::{ServerHandler, schemars, tool, tool_handler, tool_router};

    #[derive(Clone)]
    pub struct Echo {
        #[allow(dead_code)] // read by the #[tool_handler] expansion
        tool_router: ToolRouter<Self>,
    }
    #[derive(serde::Deserialize, schemars::JsonSchema)]
    pub struct Say {
        pub text: String,
    }
    #[tool_router]
    impl Echo {
        pub fn new() -> Self {
            Self {
                tool_router: Self::tool_router(),
            }
        }
        #[tool(description = "Echo text back")]
        fn echo(&self, Parameters(Say { text }): Parameters<Say>) -> String {
            format!("echo: {text}")
        }
    }
    #[tool_handler]
    impl ServerHandler for Echo {}
}

/// A connector that serves the Echo server over an in-memory pipe and can be made to fail.
fn echo_connector(fail_after: usize) -> (Connector, Arc<Mutex<usize>>) {
    use rmcp::ServiceExt;
    let count = Arc::new(Mutex::new(0usize));
    let c = count.clone();
    let connector: Connector = Arc::new(move || {
        let c = c.clone();
        Box::pin(async move {
            let n = {
                let mut n = c.lock().unwrap();
                *n += 1;
                *n
            };
            if n > fail_after {
                return Err("server process exited".to_string());
            }
            let (a, b) = tokio::io::duplex(65536);
            tokio::spawn(async move {
                if let Ok(s) = server::Echo::new().serve(b).await {
                    let _ = s.waiting().await;
                }
            });
            ().serve(a).await.map_err(|e| e.to_string())
        })
    });
    (connector, count)
}

#[tokio::test]
async fn mcp_tools_are_discovered_and_called() {
    let (conn, _) = echo_connector(usize::MAX);
    let srv = McpServer::new("demo", conn);
    srv.connect().await.unwrap();
    let specs = srv.specs();
    assert_eq!(specs[0].name, "demo__echo");
    let out = srv.call("echo", json!({"text": "hi"})).await.unwrap();
    assert!(out.contains("echo: hi"), "{out}");
}

#[tokio::test]
async fn a_crashing_mcp_server_is_restarted_three_times_then_marked_offline() {
    let (conn, count) = echo_connector(1);
    let srv = McpServer::new("flaky", conn);
    srv.connect().await.unwrap();
    srv.simulate_crash().await;
    let err = srv.call("echo", json!({"text": "hi"})).await.unwrap_err();
    assert!(err.contains("offline"), "{err}");
    assert_eq!(
        *count.lock().unwrap(),
        1 + 3,
        "first start plus three restarts"
    );
    assert!(matches!(srv.status(), ServerStatus::Offline { .. }));
}

#[test]
fn stdio_servers_need_their_runtime_on_path() {
    let st = markon_tools::mcp::check_runtime("definitely-not-a-real-runtime-xyz");
    assert!(matches!(st, ServerStatus::NeedsRuntime { .. }));
    assert!(matches!(
        markon_tools::mcp::check_runtime("sh"),
        ServerStatus::Starting
    ));
}

#[tokio::test]
async fn close_cards_asks_the_screen_to_close_and_shows_no_card_of_its_own() {
    let s = setup("close", vec![]);
    let out =
        s.rt.call(call("close_cards", json!({"which": "Summertime"})))
            .await
            .unwrap();
    assert!(!out.is_empty());
    let cards = s.cards.lock().unwrap();
    assert_eq!(
        cards.len(),
        1,
        "one close request, no running/done tool card"
    );
    assert_eq!(cards[0].1.status, CardStatus::Command);
    assert_eq!(cards[0].1.body["command"], "close");
    assert_eq!(cards[0].1.body["which"], "Summertime");
}

#[tokio::test]
async fn pause_media_pauses_without_closing() {
    let s = setup("pause", vec![]);
    s.rt.call(call("pause_media", json!({"action": "pause"})))
        .await
        .unwrap();
    let cards = s.cards.lock().unwrap();
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].1.status, CardStatus::Command);
    assert_eq!(
        cards[0].1.body,
        json!({"command": "pause", "which": "media"})
    );
}

#[tokio::test]
async fn every_spotify_call_updates_the_same_card() {
    // Invalid actions fail before the Spotify app is touched, but still report on the card.
    let s = setup("spotify", vec![]);
    for _ in 0..2 {
        let mut c = call("spotify", json!({"action": "dance"}));
        c.id = format!("{}-{}", c.id, s.cards.lock().unwrap().len());
        assert!(s.rt.call(c).await.is_err());
    }
    let cards = s.cards.lock().unwrap();
    assert!(!cards.is_empty());
    assert!(
        cards.iter().all(|(_, c)| c.id == "spotify"),
        "one card, updated in place"
    );
}
