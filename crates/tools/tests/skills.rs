//! A skill folder dropped into ~/.markon/skills works with no rebuild, and trust levels hold.
use markon_agent::{PromptSection, ToolCall, ToolRuntime, UserTurn};
use markon_tools::BoxFuture;
use markon_tools::permissions::{Decision, Permissions, Prompter, Request};
use markon_tools::skills::{self, CatalogSection, Library, Trust, WithSkills};
use serde_json::json;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Asker {
    answer: Mutex<Option<Decision>>,
    asked: Mutex<Vec<Request>>,
}
impl Prompter for Asker {
    fn ask(&self, req: Request) -> BoxFuture<'static, Decision> {
        self.asked.lock().unwrap().push(req);
        let d = self.answer.lock().unwrap().unwrap_or(Decision::Deny);
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

fn setup(name: &str) -> (WithSkills<None_>, Arc<Asker>, Library) {
    let root = std::env::temp_dir().join(format!("markon-skills-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let asker = Arc::new(Asker::default());
    let library = Library {
        root,
        bundled: skills::BUNDLED,
    };
    let perms = Arc::new(Permissions::new(vec![], asker.clone()));
    (
        WithSkills {
            inner: None_,
            library: library.clone(),
            perms,
        },
        asker,
        library,
    )
}

fn call(name: &str, input: serde_json::Value) -> ToolCall {
    ToolCall {
        id: "1".into(),
        name: name.into(),
        input,
        turn_id: 1,
        ..Default::default()
    }
}

const TRIP: &str = "---\nname: trip-planner\ndescription: Plan a trip with flights and hotels.\n---\nAsk for dates first.\n";

#[tokio::test]
async fn a_dropped_skill_is_in_the_catalog_and_asks_before_it_is_used() {
    let (rt, asker, lib) = setup("drop");
    let catalog = CatalogSection(lib.clone());
    assert!(
        !catalog
            .render(&UserTurn::text("x"))
            .unwrap()
            .contains("trip-planner")
    );
    std::fs::create_dir_all(lib.root.join("trip-planner")).unwrap();
    std::fs::write(lib.root.join("trip-planner/SKILL.md"), TRIP).unwrap();
    std::fs::write(lib.root.join("trip-planner/airlines.md"), "list").unwrap();
    let prompt = catalog.render(&UserTurn::text("x")).unwrap();
    assert!(
        prompt.contains("<name>trip-planner</name><trust>unreviewed</trust>"),
        "{prompt}"
    );
    assert!(prompt.contains("<name>markon-setup</name><trust>bundled</trust>"));

    let refused = rt
        .call(call("load_skill", json!({"name": "trip-planner"})))
        .await;
    assert!(refused.is_err(), "unreviewed and the user said no");
    assert!(
        asker.asked.lock().unwrap()[0]
            .detail
            .contains("unreviewed skill \"trip-planner\"")
    );

    lib.approve("trip-planner").unwrap();
    let ok = rt
        .call(call("load_skill", json!({"name": "trip-planner"})))
        .await
        .unwrap();
    assert!(
        ok.contains("Ask for dates first.") && ok.contains("airlines.md"),
        "{ok}"
    );
    assert_eq!(
        asker.asked.lock().unwrap().len(),
        1,
        "an approved skill does not ask"
    );

    std::fs::write(
        lib.root.join("trip-planner/SKILL.md"),
        TRIP.replace("dates", "budget"),
    )
    .unwrap();
    let (all, _) = lib.all();
    assert_eq!(
        all.iter().find(|s| s.name == "trip-planner").unwrap().trust,
        Trust::Unreviewed,
        "edited after approval"
    );
}

#[tokio::test]
async fn learn_this_shows_the_exact_text_and_saves_only_on_yes() {
    let (rt, asker, lib) = setup("learn");
    let input = json!({"name": "weekly-report", "description": "Write my weekly report.", "instructions": "1. Read GitHub.\n2. Write three lines."});
    assert!(rt.call(call("save_skill", input.clone())).await.is_err());
    assert!(!lib.root.join("weekly-report").exists());
    let shown = asker.asked.lock().unwrap()[0].detail.clone();
    assert!(
        shown.contains("name: weekly-report") && shown.contains("2. Write three lines."),
        "{shown}"
    );

    *asker.answer.lock().unwrap() = Some(Decision::Allow);
    let other = json!({"name": "weekly-report", "description": "Write my weekly report.", "instructions": "1. Read GitHub.\n2. Write four lines."});
    rt.call(call("save_skill", other)).await.unwrap();
    let saved = lib
        .all()
        .0
        .into_iter()
        .find(|s| s.name == "weekly-report")
        .unwrap();
    assert_eq!(saved.trust, Trust::Yours);
    assert!(
        rt.call(call(
            "save_skill",
            json!({"name": "Bad Name", "description": "x", "instructions": "y"})
        ))
        .await
        .is_err()
    );
}
