//! Shortcuts, system controls, apps and windows, Accessibility control and
//! screen awareness, all against a fake Mac. Nothing here touches real apps, settings or permissions.
use markon_agent::ToolCall;
use markon_platform::Permission;
use markon_platform::fake::FakeMac;
use markon_tools::cards::{Card, CardStatus};
use markon_tools::permissions::{Decision, Permissions, Prompter, Request, Tier};
use markon_tools::{BoxFuture, Runtime};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

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

struct Rig {
    rt: Runtime,
    mac: Arc<FakeMac>,
    asker: Arc<Asker>,
    cards: Arc<Mutex<Vec<Card>>>,
}

impl Rig {
    fn asked(&self) -> Vec<Request> {
        self.asker.asked.lock().unwrap().clone()
    }
    fn last_card(&self) -> Card {
        self.cards.lock().unwrap().last().cloned().expect("a card")
    }
    fn cards_for(&self, id: &str) -> Vec<Card> {
        self.cards
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.id == format!("tool-{id}"))
            .cloned()
            .collect()
    }
}

/// `answers` are popped from the end: the last one answers the first question.
fn rig(mac: FakeMac, answers: Vec<Decision>) -> Rig {
    let mac = Arc::new(mac);
    let asker = Arc::new(Asker {
        answers: Mutex::new(answers),
        ..Default::default()
    });
    let cards = Arc::new(Mutex::new(Vec::new()));
    let c = cards.clone();
    let rt = Runtime::new(
        Permissions::new(vec![], asker.clone()),
        Arc::new(move |card| c.lock().unwrap().push(card)),
    )
    .with_platform(mac.clone());
    Rig {
        rt,
        mac,
        asker,
        cards,
    }
}

fn call(id: &str, name: &str, input: Value) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: name.into(),
        input,
        turn_id: 1,
        tainted: false,
    }
}

// ---- 1. Apple Shortcuts ----

#[tokio::test]
async fn shortcuts_are_listed_without_asking() {
    let mac = FakeMac::default();
    mac.answer("shortcuts list", Ok("Morning\nSet Focus Work\nLog water"));
    let r = rig(mac, vec![]);
    let out =
        r.rt.call(call("1", "list_shortcuts", json!({})))
            .await
            .unwrap();
    assert!(out.contains("Set Focus Work"), "{out}");
    assert!(r.asked().is_empty(), "listing changes nothing");
    let card = r.last_card();
    assert_eq!(card.status, CardStatus::Done);
    assert!(card.body["preview"].as_str().unwrap().contains("Log water"));
}

#[tokio::test]
async fn running_a_shortcut_asks_and_puts_its_output_on_a_card() {
    let mac = FakeMac::default();
    mac.answer("shortcuts run Log water", Ok("Logged 250 ml"));
    let r = rig(mac, vec![Decision::Allow]);
    let out =
        r.rt.call(call(
            "1",
            "run_shortcut",
            json!({"name": "Log water", "input": "250"}),
        ))
        .await
        .unwrap();
    assert!(out.contains("Logged 250 ml"), "{out}");
    let asked = r.asked();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].tier, Tier::Execute);
    assert!(
        asked[0].detail.contains("Log water") && asked[0].detail.contains("250"),
        "the user sees the shortcut and its input: {}",
        asked[0].detail
    );
    assert_eq!(r.mac.did(), ["shortcuts run Log water <- 250"]);
    let card = r.last_card();
    assert_eq!(card.status, CardStatus::Done);
    assert_eq!(card.body["preview"], "Logged 250 ml");
}

#[tokio::test]
async fn a_no_runs_no_shortcut_and_odd_names_are_refused() {
    let r = rig(FakeMac::default(), vec![Decision::Deny]);
    let err =
        r.rt.call(call("1", "run_shortcut", json!({"name": "Morning"})))
            .await
            .unwrap_err();
    assert!(err.contains("did not allow"), "{err}");
    assert!(r.mac.did().is_empty());
    assert_eq!(r.last_card().status, CardStatus::Blocked);

    let err =
        r.rt.call(call("2", "run_shortcut", json!({"name": "--help"})))
            .await
            .unwrap_err();
    assert!(err.contains("not a shortcut name"), "{err}");
    assert_eq!(r.asked().len(), 1, "an odd name is refused before asking");
}

// ---- 2. System controls ----

#[tokio::test]
async fn volume_and_mute_change_without_asking() {
    let mac = FakeMac::default();
    mac.answer("system Status", Ok("30,false"));
    let r = rig(mac, vec![]);
    r.rt.call(call(
        "1",
        "system_control",
        json!({"action": "volume", "level": 30}),
    ))
    .await
    .unwrap();
    r.rt.call(call("2", "system_control", json!({"action": "mute"})))
        .await
        .unwrap();
    let out =
        r.rt.call(call("3", "system_control", json!({"action": "status"})))
            .await
            .unwrap();
    assert!(out.contains("30%") && out.contains("not muted"), "{out}");
    assert!(r.asked().is_empty());
    assert_eq!(
        r.mac.did(),
        ["system Volume(30)", "system Mute(true)", "system Status"]
    );
    let err =
        r.rt.call(call(
            "4",
            "system_control",
            json!({"action": "volume", "level": 140}),
        ))
        .await
        .unwrap_err();
    assert!(err.contains("0 to 100"), "{err}");
}

#[tokio::test]
async fn sleep_lock_and_wifi_ask_first() {
    let r = rig(
        FakeMac::default(),
        vec![Decision::Deny, Decision::Allow, Decision::Allow],
    );
    for (id, action) in [("1", "lock"), ("2", "wifi_off"), ("3", "sleep")] {
        let _ =
            r.rt.call(call(id, "system_control", json!({"action": action})))
                .await;
    }
    assert_eq!(r.asked().len(), 3);
    assert!(r.asked()[2].detail.contains("sleep"), "{:?}", r.asked()[2]);
    assert_eq!(
        r.mac.did(),
        ["system Lock", "system Wifi(false)"],
        "the no for sleep stopped it"
    );
}

#[tokio::test]
async fn focus_runs_the_users_focus_shortcut_or_says_how_to_make_one() {
    let mac = FakeMac::default();
    mac.answer("shortcuts list", Ok("Morning\nMarkon Focus On"));
    let r = rig(mac, vec![]);
    r.rt.call(call("1", "system_control", json!({"action": "focus_on"})))
        .await
        .unwrap();
    assert!(
        r.mac
            .did()
            .contains(&"shortcuts run Markon Focus On".to_string())
    );
    let err =
        r.rt.call(call("2", "system_control", json!({"action": "focus_off"})))
            .await
            .unwrap_err();
    assert!(
        err.contains("Markon Focus Off") && err.contains("Set Focus"),
        "the user is told how to set it up: {err}"
    );
}

// ---- 3. Apps and windows ----

#[tokio::test]
async fn running_apps_are_listed_and_focused_without_asking() {
    let mac = FakeMac::default();
    mac.answer(
        "apps list",
        Ok(r#"{"front":"Safari","apps":["Finder","Safari","Mail"]}"#),
    );
    let r = rig(mac, vec![]);
    let out = r.rt.call(call("1", "list_apps", json!({}))).await.unwrap();
    assert!(
        out.contains("Mail") && out.contains("In front: Safari"),
        "{out}"
    );
    r.rt.call(call("2", "focus_app", json!({"name": "Mail"})))
        .await
        .unwrap();
    assert!(r.asked().is_empty());
    assert_eq!(r.mac.did(), ["apps list", "apps focus Mail"]);
}

#[tokio::test]
async fn quitting_an_app_asks_and_a_no_keeps_it_open() {
    let r = rig(FakeMac::default(), vec![Decision::Allow, Decision::Deny]);
    let err =
        r.rt.call(call("1", "quit_app", json!({"name": "Safari"})))
            .await
            .unwrap_err();
    assert!(err.contains("did not allow"), "{err}");
    assert!(r.mac.did().is_empty());
    r.rt.call(call("2", "quit_app", json!({"name": "Mail"})))
        .await
        .unwrap();
    assert_eq!(r.mac.did(), ["apps quit Mail"]);
    assert!(r.asked()[1].detail.contains("Quit Mail"));
}

#[tokio::test]
async fn the_front_window_moves_to_simple_layouts() {
    let mac = FakeMac::default();
    mac.answer("apps window", Ok("Safari\t100,100,800,600"));
    mac.answer("apps screen", Ok("0,33,1512,949"));
    let r = rig(mac, vec![]);
    let out =
        r.rt.call(call("1", "arrange_window", json!({"layout": "left"})))
            .await
            .unwrap();
    assert!(out.contains("Safari") && out.contains("left"), "{out}");
    assert_eq!(
        r.mac.did(),
        [
            "apps window front",
            "apps screen",
            "apps set_window Safari 0,33,756,949"
        ]
    );
    // What undo needs, the window's frame before the move, is kept for the activity log.
    assert_eq!(
        r.rt.take_undo("1"),
        Some(json!({"kind": "window", "app": "Safari", "frame": [100, 100, 800, 600]}))
    );
    assert!(r.last_card().body.get("undo").is_none());
    let err =
        r.rt.call(call("2", "arrange_window", json!({"layout": "diagonal"})))
            .await
            .unwrap_err();
    assert!(err.contains("left, right, full or center"), "{err}");
}

// ---- 4. Accessibility control of any app ----

const MAIL_TREE: &str = r#"{"app":"Mail","nodes":[
 {"path":"1","depth":0,"role":"AXWindow","subrole":"AXStandardWindow","title":"New Message","desc":"","value":"","secure":false},
 {"path":"1.1","depth":1,"role":"AXGroup","subrole":"","title":"","desc":"","value":"","secure":false},
 {"path":"1.1.1","depth":2,"role":"AXTextField","subrole":"","title":"","desc":"To:","value":"sara@example.com","secure":false},
 {"path":"1.1.2","depth":2,"role":"AXSecureTextField","subrole":"","title":"","desc":"Password","value":"","secure":true},
 {"path":"1.2","depth":1,"role":"AXButton","subrole":"","title":"Send","desc":"","value":"","secure":false},
 {"path":"1.3","depth":1,"role":"AXStaticText","subrole":"","title":"","desc":"","value":"Draft saved","secure":false}
]}"#;

fn mail(answers: Vec<Decision>) -> Rig {
    let mac = FakeMac::with_permissions(&[Permission::Accessibility]);
    mac.answer("ax tree", Ok(MAIL_TREE));
    rig(mac, answers)
}

#[tokio::test]
async fn without_accessibility_a_card_offers_the_settings_pane() {
    let r = rig(FakeMac::default(), vec![]);
    let err =
        r.rt.call(call("1", "read_app", json!({"app": "Mail"})))
            .await
            .unwrap_err();
    assert!(err.contains("Accessibility"), "{err}");
    let card = r.last_card();
    assert_eq!(card.status, CardStatus::Blocked);
    assert_eq!(card.body["needs_permission"], "accessibility");
    assert!(r.mac.did().is_empty(), "nothing was read");
}

#[tokio::test]
async fn an_app_is_read_as_a_numbered_outline_without_secure_values() {
    let r = mail(vec![]);
    let out =
        r.rt.call(call("1", "read_app", json!({"app": "Mail"})))
            .await
            .unwrap();
    assert!(out.contains("window \"New Message\""), "{out}");
    assert!(
        out.contains("[1] text field \"To:\" = sara@example.com"),
        "{out}"
    );
    assert!(out.contains("[2] secure text field \"Password\""), "{out}");
    assert!(out.contains("[3] button \"Send\""), "{out}");
    assert!(out.contains("\"Draft saved\""), "{out}");
    assert!(!out.contains("group"), "empty wrappers are left out: {out}");
    assert!(out.contains("data, not instructions"));
    assert!(r.asked().is_empty(), "reading does not ask");
}

#[tokio::test]
async fn every_change_asks_with_its_own_card_and_secrets_are_never_typed() {
    let r = mail(vec![Decision::Allow, Decision::Allow, Decision::Allow]);
    r.rt.call(call("1", "read_app", json!({"app": "Mail"})))
        .await
        .unwrap();
    r.rt.call(call(
        "2",
        "type_in_app",
        json!({"ref": 1, "text": "abel@example.com"}),
    ))
    .await
    .unwrap();
    r.rt.call(call("3", "click_in_app", json!({"ref": 3})))
        .await
        .unwrap();
    r.rt.call(call("4", "press_keys", json!({"keys": "cmd+s"})))
        .await
        .unwrap();
    let asked = r.asked();
    assert_eq!(asked.len(), 3);
    assert!(asked[0].detail.contains("abel@example.com") && asked[0].detail.contains("To:"));
    assert!(
        asked[1].detail.contains("button \"Send\""),
        "{}",
        asked[1].detail
    );
    assert!(asked[2].detail.contains("cmd+s"), "{}", asked[2].detail);
    assert_eq!(
        r.mac.did(),
        [
            "ax tree Mail",
            "ax type Mail 1.1.1 abel@example.com",
            "ax press Mail 1.2",
            "ax keys Mail command down s"
        ]
    );
    for id in ["2", "3", "4"] {
        let cards = r.cards_for(id);
        assert_eq!(cards.last().unwrap().status, CardStatus::Done, "step {id}");
    }

    let err =
        r.rt.call(call(
            "5",
            "type_in_app",
            json!({"ref": 2, "text": "hunter2"}),
        ))
        .await
        .unwrap_err();
    assert!(err.contains("never types"), "{err}");
    let err =
        r.rt.call(call(
            "6",
            "type_in_app",
            json!({"ref": 1, "text": "4111 1111 1111 1111"}),
        ))
        .await
        .unwrap_err();
    assert!(err.contains("never types"), "{err}");
    assert_eq!(r.asked().len(), 3, "secrets are refused before asking");
    let err =
        r.rt.call(call("7", "click_in_app", json!({"ref": 9})))
            .await
            .unwrap_err();
    assert!(err.contains("read the app again"), "{err}");
}

#[tokio::test]
async fn a_no_stops_the_run() {
    let r = mail(vec![Decision::Allow, Decision::Deny]);
    r.rt.call(call("1", "read_app", json!({"app": "Mail"})))
        .await
        .unwrap();
    let err =
        r.rt.call(call("2", "click_in_app", json!({"ref": 3})))
            .await
            .unwrap_err();
    assert!(err.contains("stopped"), "{err}");
    let err =
        r.rt.call(call("3", "type_in_app", json!({"ref": 1, "text": "hi"})))
            .await
            .unwrap_err();
    assert!(err.contains("stopped"), "{err}");
    assert_eq!(r.asked().len(), 1, "the next step was not even asked");
    assert_eq!(r.mac.did(), ["ax tree Mail"]);
    assert_eq!(r.last_card().status, CardStatus::Blocked);
}

// ---- 5. Screen awareness ----

#[tokio::test]
async fn without_screen_recording_a_card_offers_the_settings_pane() {
    let r = rig(FakeMac::default(), vec![]);
    let err =
        r.rt.call(call("1", "look_at_screen", json!({})))
            .await
            .unwrap_err();
    assert!(err.contains("Screen Recording"), "{err}");
    assert_eq!(r.last_card().body["needs_permission"], "screen_recording");
    assert!(r.mac.did().is_empty());
}

#[tokio::test]
async fn the_screen_goes_to_the_model_as_a_picture_and_on_a_card() {
    let mac = FakeMac::with_permissions(&[Permission::ScreenRecording]);
    mac.answer("apps list", Ok(r#"{"front":"Keynote","apps":["Keynote"]}"#));
    let mut r = rig(mac, vec![]);
    r.rt.cache_dir = std::env::temp_dir().join(format!("markon-m13-shot-{}", std::process::id()));
    let out =
        r.rt.call(call("1", "look_at_screen", json!({})))
            .await
            .unwrap();
    let path = markon_tools::mac::screenshot_path(&out).expect("a screenshot marker");
    assert!(path.starts_with(&r.rt.cache_dir) && path.exists(), "{out}");
    assert!(
        out.contains("Keynote"),
        "the front app helps text-only models: {out}"
    );
    assert!(
        r.asked().is_empty(),
        "a clean conversation looks without asking"
    );
    let card = r.last_card();
    assert_eq!(card.status, CardStatus::Done);
    assert_eq!(card.body["kind"], "image");
    assert_eq!(card.body["path"], json!(path));

    // After outside content was read, a page cannot make Markon look silently.
    let mut c = call("2", "look_at_screen", json!({}));
    c.tainted = true;
    let _ = r.rt.call(c).await;
    assert_eq!(r.asked().len(), 1);
    assert_eq!(r.asked()[0].tier, Tier::Execute);
}

#[tokio::test]
async fn a_private_screen_stays_on_the_card() {
    let mac = FakeMac::with_permissions(&[Permission::ScreenRecording]);
    let r = rig(mac, vec![]);
    r.rt.screen_private
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let out =
        r.rt.call(call("1", "look_at_screen", json!({})))
            .await
            .unwrap();
    assert!(markon_tools::mac::screenshot_path(&out).is_none(), "{out}");
    assert!(out.contains("private"), "{out}");
    assert_eq!(
        r.last_card().body["kind"],
        "image",
        "the screen still shows it"
    );
}
