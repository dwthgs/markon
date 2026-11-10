//! Every browser step is asked and shown on a card, the stop command ends it, secrets
//! are never typed, and programs run sandboxed after a yes.
use markon_agent::{ToolCall, ToolRuntime};
use markon_hands::{BoxFuture, Driver, WithHands};
use markon_tools::cards::{Card, CardStatus};
use markon_tools::permissions::{Decision, Permissions, Prompter, Request};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct FakeChrome {
    did: Mutex<Vec<String>>,
}
impl Driver for FakeChrome {
    fn open(self: Arc<Self>, url: String) -> BoxFuture<'static, Result<String, String>> {
        self.did.lock().unwrap().push(format!("open {url}"));
        Box::pin(async { Ok("Shop".into()) })
    }
    fn tree(self: Arc<Self>) -> BoxFuture<'static, Result<(String, Vec<Value>), String>> {
        let n = |id: &str, p: Option<&str>, role: &str, name: &str, kids: &[&str], b: i64| json!({"nodeId": id, "parentId": p, "ignored": false, "role": {"value": role}, "name": {"value": name}, "childIds": kids, "backendDOMNodeId": b});
        let nodes = vec![
            n("1", None, "RootWebArea", "Shop", &["2", "3", "4"], 1),
            n("2", Some("1"), "textbox", "Search", &[], 20),
            n("3", Some("1"), "textbox", "Password", &[], 30),
            n("4", Some("1"), "button", "Buy now", &[], 40),
        ];
        Box::pin(async move { Ok(("https://shop.example/".into(), nodes)) })
    }
    fn click(self: Arc<Self>, b: i64) -> BoxFuture<'static, Result<(), String>> {
        self.did.lock().unwrap().push(format!("click {b}"));
        Box::pin(async { Ok(()) })
    }
    fn type_text(self: Arc<Self>, b: i64, t: String) -> BoxFuture<'static, Result<(), String>> {
        self.did.lock().unwrap().push(format!("type {b} {t}"));
        Box::pin(async { Ok(()) })
    }
}

struct Asker {
    answers: Mutex<Vec<Decision>>,
    asked: Mutex<Vec<Request>>,
}
impl Prompter for Asker {
    fn ask(&self, r: Request) -> BoxFuture<'static, Decision> {
        self.asked.lock().unwrap().push(r);
        let d = self.answers.lock().unwrap().pop().unwrap_or(Decision::Deny);
        Box::pin(async move { d })
    }
}

struct None_;
impl ToolRuntime for None_ {
    fn specs(&self) -> Vec<markon_provider::ToolSpec> {
        vec![]
    }
    fn call(&self, _: ToolCall) -> BoxFuture<'static, Result<String, String>> {
        Box::pin(async { Err("none".into()) })
    }
}

type Rig = (
    WithHands<None_>,
    Arc<FakeChrome>,
    Arc<Asker>,
    Arc<Mutex<Vec<Card>>>,
);

fn hands(answers: Vec<Decision>) -> Rig {
    let chrome = Arc::new(FakeChrome::default());
    let asker = Arc::new(Asker {
        answers: Mutex::new(answers),
        asked: Mutex::new(vec![]),
    });
    let cards = Arc::new(Mutex::new(Vec::new()));
    let c = cards.clone();
    let h = WithHands {
        inner: None_,
        browser: Some(chrome.clone()),
        perms: Arc::new(Permissions::new(vec![], asker.clone())),
        cards: Arc::new(move |card| c.lock().unwrap().push(card)),
        refs: Default::default(),
    };
    (h, chrome, asker, cards)
}

fn call(id: &str, name: &str, input: Value) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: name.into(),
        input,
        turn_id: 1,
        ..Default::default()
    }
}

#[tokio::test]
async fn every_browser_step_is_asked_and_shown_on_its_own_card() {
    let (h, chrome, asker, cards) = hands(vec![Decision::Allow, Decision::Allow, Decision::Allow]);
    h.call(call(
        "1",
        "browser_open",
        json!({"url": "https://shop.example/"}),
    ))
    .await
    .unwrap();
    let page = h.call(call("2", "browser_read", json!({}))).await.unwrap();
    assert!(
        page.contains("[1] [textbox] \"Search\"") && page.contains("[3] [button] \"Buy now\""),
        "{page}"
    );
    h.call(call(
        "3",
        "browser_type",
        json!({"ref": 1, "text": "running shoes"}),
    ))
    .await
    .unwrap();
    h.call(call("4", "browser_click", json!({"ref": 3})))
        .await
        .unwrap();
    assert_eq!(
        *chrome.did.lock().unwrap(),
        [
            "open https://shop.example/",
            "type 20 running shoes",
            "click 40"
        ]
    );
    let asked: Vec<String> = asker
        .asked
        .lock()
        .unwrap()
        .iter()
        .map(|r| r.detail.clone())
        .collect();
    assert_eq!(
        asked.len(),
        3,
        "open, type and click asked; reading does not: {asked:?}"
    );
    assert!(
        asked[2].contains("[3] [button] \"Buy now\""),
        "the exact element is shown"
    );
    let ids: std::collections::HashSet<String> =
        cards.lock().unwrap().iter().map(|c| c.id.clone()).collect();
    assert_eq!(ids.len(), 4, "one card per step");
}

#[tokio::test]
async fn a_no_stops_the_step_and_passwords_are_never_typed() {
    let (h, chrome, asker, cards) = hands(vec![]);
    let err = h
        .call(call(
            "1",
            "browser_open",
            json!({"url": "https://shop.example/"}),
        ))
        .await
        .unwrap_err();
    assert!(err.contains("did not allow"), "{err}");
    assert!(chrome.did.lock().unwrap().is_empty());
    assert_eq!(
        cards.lock().unwrap().last().unwrap().status,
        CardStatus::Blocked
    );
    h.call(call("2", "browser_read", json!({}))).await.unwrap();
    let err = h
        .call(call(
            "3",
            "browser_type",
            json!({"ref": 2, "text": "hunter2"}),
        ))
        .await
        .unwrap_err();
    assert!(err.contains("never types"), "{err}");
    let err = h
        .call(call(
            "4",
            "browser_type",
            json!({"ref": 1, "text": "4111 1111 1111 1111"}),
        ))
        .await
        .unwrap_err();
    assert!(
        err.contains("never types"),
        "a card number in any field: {err}"
    );
    assert_eq!(
        asker.asked.lock().unwrap().len(),
        1,
        "refused before asking"
    );
    assert!(
        h.call(call("5", "browser_click", json!({"ref": 9})))
            .await
            .unwrap_err()
            .contains("no element 9")
    );
}

#[tokio::test]
async fn the_stop_command_ends_a_step_that_is_waiting() {
    struct Hangs;
    impl Prompter for Hangs {
        fn ask(&self, _: Request) -> BoxFuture<'static, Decision> {
            Box::pin(std::future::pending())
        }
    }
    let (mut h, chrome, _, _) = hands(vec![]);
    h.perms = Arc::new(Permissions::new(vec![], Arc::new(Hangs)));
    let step = tokio::spawn(h.call(call(
        "1",
        "browser_open",
        json!({"url": "https://shop.example/"}),
    )));
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    step.abort(); // what the agent does on "Hey Markon, stop"
    assert!(step.await.unwrap_err().is_cancelled());
    assert!(chrome.did.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_program_runs_in_the_sandbox_only_after_a_yes() {
    let (h, _, asker, _) = hands(vec![Decision::Allow]);
    let out = h
        .call(call(
            "1",
            "run_js",
            json!({"code": "console.log(2 ** 10); 'done'"}),
        ))
        .await
        .unwrap();
    assert_eq!(out, "1024\ndone");
    assert!(
        asker.asked.lock().unwrap()[0].detail.contains("2 ** 10"),
        "the code is shown"
    );
    assert!(
        h.call(call("2", "run_js", json!({"code": "1+1"})))
            .await
            .is_err(),
        "no answer is a no"
    );
}

/// Launches the real Chrome, headless, on a page held in memory. `cargo test -p markon-hands -- --ignored`.
#[tokio::test]
#[ignore]
async fn the_real_chrome_opens_reads_and_types() {
    let exe =
        std::path::PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome");
    let profile = std::env::temp_dir().join(format!("markon-chrome-test-{}", std::process::id()));
    let mut c = markon_hands::chrome::Chrome::new(exe, profile);
    c.headless = true;
    let c = Arc::new(c);
    c.clone().open("data:text/html,<title>T</title><h1>Hello</h1><input aria-label='Name'><button>Send</button>".into()).await.unwrap();
    let (_, nodes) = c.clone().tree().await.unwrap();
    let (text, refs) = markon_hands::outline::outline(&nodes, 100);
    assert!(
        text.contains("[heading] \"Hello\"") && text.contains("[button] \"Send\""),
        "{text}"
    );
    let (field, _) = refs
        .values()
        .find(|(_, l)| l.contains("Name"))
        .cloned()
        .unwrap();
    c.clone().type_text(field, "Sara".into()).await.unwrap();
    let (_, nodes) = c.clone().tree().await.unwrap();
    assert!(
        markon_hands::outline::outline(&nodes, 100)
            .0
            .contains("value=Sara")
    );
}
