//! Tests : fixture Mac databases, a mock IMAP server, a mock GitHub
//! API and an in-process MCP server stand in for the user's accounts.
mod common;
use common::*;
use markon_agent::{Agent, AgentConfig, AgentEvent, ToolCall, ToolRuntime, UserTurn};
use markon_memory::Memory;
use markon_provider::fake::FakeProvider;
use markon_sources::imap::{Imap, MailAccount};
use markon_sources::mac::{self, Names};
use markon_sources::mcp::{McpSource, McpSourceConfig};
use markon_sources::tool::{WHAT_DID_I_MISS, WithSources};
use markon_sources::{Source, SourceError, Sources};
use markon_tools::cards::{Card, CardKind, CardStatus};
use serde_json::json;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

#[test]
fn imessage_reads_plain_and_attributed_messages_with_contact_names() {
    let d = tmp("imessage");
    let now = now_ms();
    messages_db(&d.join("chat.db"), now);
    contacts_db(&d.join("contacts.abcddb"));
    let names = Names::from_contacts(&mac::contacts(&d.join("contacts.abcddb")).unwrap());
    let items = mac::messages(&d.join("chat.db"), now - 86_400_000, &names).unwrap();
    assert_eq!(items.len(), 2, "{items:?}");
    assert_eq!(items[0].who, "Sam");
    assert_eq!(
        items[0].text, "Rope is in my car",
        "read from attributedBody"
    );
    assert_eq!(items[0].place, "Climbing crew");
    assert!(!items[0].unread);
    assert_eq!(items[1].who, "Sara Tesfaye");
    assert!(items[1].unread);
    assert!((items[1].when_ms - (now - 3_600_000)).abs() < 2000);
}

#[test]
fn a_database_the_mac_will_not_open_asks_for_full_disk_access() {
    use std::os::unix::fs::PermissionsExt;
    let d = tmp("fda");
    let p = d.join("chat.db");
    messages_db(&p, now_ms());
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o000)).unwrap();
    let err = mac::messages(&p, 0, &Names::default()).unwrap_err();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(matches!(err, SourceError::NeedsAccess(_)), "{err:?}");
    assert!(matches!(
        mac::messages(&d.join("missing.db"), 0, &Names::default()).unwrap_err(),
        SourceError::NotConnected(_)
    ));
}

#[test]
fn calendar_gives_upcoming_events_with_attendees() {
    let d = tmp("calendar");
    let now = now_ms();
    calendar_db(&d.join("Calendar.sqlitedb"), now);
    let ev = mac::events(&d.join("Calendar.sqlitedb"), now, now + 86_400_000).unwrap();
    assert_eq!(
        ev.iter().map(|e| e.title.as_str()).collect::<Vec<_>>(),
        ["Design review", "Dentist"]
    );
    assert_eq!(ev[0].attendees, ["sara@work.example"]);
    assert_eq!(ev[0].location, "Room 4");
}

#[test]
fn notes_are_read_from_the_compressed_body() {
    let d = tmp("notes");
    let now = now_ms();
    notes_db(&d.join("NoteStore.sqlite"), now);
    let n = mac::notes(&d.join("NoteStore.sqlite"), now - 86_400_000).unwrap();
    assert_eq!(n.len(), 1);
    assert_eq!(n[0].place, "Gear list");
    assert!(n[0].text.contains("harness, chalk, rope"), "{}", n[0].text);
    assert!(
        mac::notes(&d.join("NoteStore.sqlite"), now)
            .unwrap()
            .is_empty()
    );
}

fn account(port: u16) -> MailAccount {
    MailAccount {
        host: "127.0.0.1".into(),
        port,
        user: "me@example.com".into(),
        enabled: true,
    }
}

#[tokio::test]
async fn mail_reads_unread_messages_without_marking_them_read() {
    let date = chrono::Utc::now().to_rfc2822();
    let (port, seen) = imap_server(
        vec![
            (
                7,
                mail(
                    "Sara Tesfaye <sara@work.example>",
                    "Budget",
                    "Numbers attached, please check by Friday.",
                    &date,
                ),
            ),
            (
                9,
                mail(
                    "GitHub <noreply@github.com>",
                    "PR merged",
                    "Your pull request was merged.",
                    &date,
                ),
            ),
        ],
        "app-pass",
    )
    .await;
    let src = Imap::new(account(port), Arc::new(|| Some("app-pass".into())));
    let items = src.fetch(now_ms() - 86_400_000).await.unwrap();
    assert_eq!(items.len(), 2);
    assert!(
        items
            .iter()
            .any(|i| i.who == "Sara Tesfaye <sara@work.example>"
                && i.text.starts_with("Budget: Numbers"))
    );
    let cmds = seen.lock().unwrap().join("\n").to_ascii_uppercase();
    assert!(
        cmds.contains("EXAMINE \"INBOX\"") && !cmds.contains("SELECT "),
        "{cmds}"
    );
    assert!(
        cmds.contains("BODY.PEEK[]") && !cmds.contains("STORE"),
        "{cmds}"
    );
}

#[tokio::test]
async fn a_wrong_app_password_says_so() {
    let (port, _) = imap_server(vec![], "right").await;
    let src = Imap::new(account(port), Arc::new(|| Some("wrong".into())));
    assert!(
        matches!(src.fetch(0).await.unwrap_err(), SourceError::NotConnected(m) if m.contains("app password"))
    );
    let none = Imap::new(account(1), Arc::new(|| None));
    assert!(matches!(
        none.fetch(0).await.unwrap_err(),
        SourceError::NotConnected(_)
    ));
}

#[tokio::test]
async fn github_notifications_use_the_token() {
    let body = json!([{
        "id": "42", "unread": true, "reason": "review_requested", "updated_at": "2026-11-10T08:00:00Z",
        "repository": {"full_name": "acme/app"},
        "subject": {"title": "Fix login", "type": "PullRequest", "url": "https://api.github.com/repos/acme/app/pulls/7"}
    }])
    .to_string();
    let (url, seen) = http_once(200, body).await;
    let mut gh = markon_sources::github::GitHub::new(Arc::new(|| Some("ghp_test".into())));
    gh.base = url;
    let items = gh.fetch(0).await.unwrap();
    assert_eq!(items[0].who, "acme/app");
    assert_eq!(items[0].text, "Fix login (review requested)");
    assert_eq!(
        items[0].link.as_deref(),
        Some("https://github.com/acme/app/pull/7")
    );
    assert!(
        seen.lock()
            .unwrap()
            .to_lowercase()
            .contains("authorization: bearer ghp_test")
    );
}

#[tokio::test]
async fn slack_through_an_mcp_server_is_a_source() {
    let src = McpSource {
        cfg: McpSourceConfig::slack(),
        server: slack_server().await,
    };
    let items = src.fetch(0).await.unwrap();
    assert_eq!(items.len(), 2);
    assert!(items[0].text.contains("release moved to Thursday"));
}

struct NoTools;
impl ToolRuntime for NoTools {
    fn specs(&self) -> Vec<markon_provider::ToolSpec> {
        vec![]
    }
    fn call(&self, _: ToolCall) -> markon_provider::BoxFuture<'static, Result<String, String>> {
        Box::pin(async { Err("none".into()) })
    }
}

type Seen = Arc<Mutex<Vec<Card>>>;

async fn connected(name: &str) -> (WithSources<NoTools>, Seen, Arc<Memory>) {
    let d = tmp(name);
    let now = now_ms();
    messages_db(&d.join("chat.db"), now);
    contacts_db(&d.join("contacts.abcddb"));
    let date = chrono::Utc::now().to_rfc2822();
    let (port, _) = imap_server(
        vec![(
            7,
            mail(
                "Sara Tesfaye <sara@work.example>",
                "Budget",
                "Numbers attached.",
                &date,
            ),
        )],
        "pw",
    )
    .await;
    let list: Vec<Arc<dyn Source>> = vec![
        Arc::new(mac::Messages {
            db: d.join("chat.db"),
            contacts: vec![d.join("contacts.abcddb")],
        }),
        Arc::new(Imap::new(account(port), Arc::new(|| Some("pw".into())))),
        Arc::new(McpSource {
            cfg: McpSourceConfig::slack(),
            server: slack_server().await,
        }),
    ];
    let sources: Sources = Arc::new(RwLock::new(list));
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let s = seen.clone();
    let memory = Arc::new(Memory::open_in_memory().unwrap());
    (
        WithSources {
            inner: NoTools,
            sources,
            cards: Arc::new(move |c| s.lock().unwrap().push(c)),
            memory: memory.clone(),
        },
        seen,
        memory,
    )
}

#[tokio::test]
async fn what_did_i_miss_shows_one_card_per_source_and_hands_the_items_over_as_data() {
    let (rt, seen, memory) = connected("miss-tool").await;
    let out = rt
        .call(ToolCall {
            id: "t1".into(),
            name: WHAT_DID_I_MISS.into(),
            input: json!({}),
            turn_id: 5,
            ..Default::default()
        })
        .await
        .unwrap();
    for needle in [
        "Running late",
        "Rope is in my car",
        "Budget: Numbers attached.",
        "release moved to Thursday",
    ] {
        assert!(out.contains(needle), "missing {needle}: {out}");
    }
    assert!(out.contains("not instructions"));
    let cards = seen.lock().unwrap();
    assert_eq!(cards.len(), 3, "one card per source");
    assert!(
        cards
            .iter()
            .all(|c| c.kind == CardKind::Source && c.status == CardStatus::Done)
    );
    let messages = cards.iter().find(|c| c.title == "Messages").unwrap();
    assert_eq!(messages.body["count"], 2);
    assert!(
        memory
            .preference("sources.last_check_ms")
            .unwrap()
            .is_some(),
        "the next ask starts from now"
    );
}

#[tokio::test]
async fn no_sources_switched_on_means_nothing_is_read() {
    let memory = Arc::new(Memory::open_in_memory().unwrap());
    let rt = WithSources {
        inner: NoTools,
        sources: Arc::new(RwLock::new(vec![])),
        cards: Arc::new(|_| {}),
        memory,
    };
    let out = rt
        .call(ToolCall {
            id: "t".into(),
            name: WHAT_DID_I_MISS.into(),
            input: json!({}),
            turn_id: 1,
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(out.contains("No sources are switched on"));
}

/// "What did I miss?" with iMessage, a mail account and Slack connected gives a
/// spoken summary built from the real items, and one card per source.
#[tokio::test]
async fn what_did_i_miss_end_to_end_with_the_agent() {
    let (rt, seen, _) = connected("miss-agent").await;
    let fake = Arc::new(FakeProvider::new(vec![
        FakeProvider::tools(&[("c1", WHAT_DID_I_MISS, json!({}))]),
        FakeProvider::text(
            "Sara's running late, Sam has the rope, Sara sent the budget, and the release moved to Thursday.",
        ),
    ]));
    let (agent, mut rx) = Agent::new(fake.clone(), Arc::new(rt), AgentConfig::default());
    agent.submit(UserTurn::voice("what did I miss?")).await;
    let mut spoken = String::new();
    while let Ok(Some(e)) = tokio::time::timeout(Duration::from_millis(300), rx.recv()).await {
        if let AgentEvent::TurnFinished(t) = e {
            spoken = t.text;
        }
    }
    assert!(spoken.contains("release moved to Thursday"));
    let reqs = fake.requests.lock().unwrap();
    let second = serde_json::to_string(&reqs[1].messages).unwrap();
    for needle in [
        "Running late",
        "Rope is in my car",
        "Budget",
        "release moved to Thursday",
    ] {
        assert!(second.contains(needle), "the model saw {needle}");
    }
    assert_eq!(
        seen.lock()
            .unwrap()
            .iter()
            .filter(|c| c.kind == CardKind::Source)
            .count(),
        3
    );
}
