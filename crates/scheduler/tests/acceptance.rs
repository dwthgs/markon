//! Acceptance tests, with a controlled clock.
use chrono::{Local, TimeZone};
use markon_memory::Memory;
use markon_scheduler::{AgentSpec, Delivery, Scheduler};
use std::sync::Arc;

fn at(h: u32, m: u32, day: u32) -> chrono::DateTime<Local> {
    Local
        .with_ymd_and_hms(2026, 11, day, h, m, 0)
        .single()
        .unwrap()
}

fn briefing() -> AgentSpec {
    AgentSpec {
        prompt: "Morning briefing".into(),
        tools: vec!["fetch_page".into()],
        max_rounds: None,
    }
}

#[test]
fn a_schedule_fires_at_its_time_and_not_before() {
    let s = Scheduler::new(Arc::new(Memory::open_in_memory().unwrap()));
    s.ensure("Morning briefing", "30 7 * * *", &briefing(), at(6, 0, 10))
        .unwrap();
    assert!(s.take_due(at(7, 29, 10)).unwrap().is_empty());
    let due = s.take_due(at(7, 30, 10)).unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].name, "Morning briefing");
    assert!(s.take_due(at(7, 31, 10)).unwrap().is_empty(), "runs once");
}

#[test]
fn runs_missed_while_asleep_happen_once_on_wake_and_never_pile_up() {
    let s = Scheduler::new(Arc::new(Memory::open_in_memory().unwrap()));
    s.ensure("Morning briefing", "30 7 * * *", &briefing(), at(6, 0, 10))
        .unwrap();
    // Asleep for three mornings.
    let due = s.take_due(at(9, 0, 13)).unwrap();
    assert_eq!(due.len(), 1);
    assert!(s.take_due(at(9, 1, 13)).unwrap().is_empty());
    assert_eq!(
        s.list().unwrap()[0].next_run,
        Some(at(7, 30, 14).timestamp_millis())
    );
}

#[test]
fn schedules_survive_a_restart() {
    let dir = std::env::temp_dir().join(format!("markon-sched-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("markon.db");
    {
        let s = Scheduler::new(Arc::new(Memory::open(&path).unwrap()));
        s.ensure("Morning briefing", "30 7 * * *", &briefing(), at(6, 0, 10))
            .unwrap();
    }
    let s = Scheduler::new(Arc::new(Memory::open(&path).unwrap()));
    // ensure() again keeps the existing schedule instead of resetting it
    s.ensure("Morning briefing", "30 7 * * *", &briefing(), at(8, 0, 10))
        .unwrap();
    assert_eq!(
        s.take_due(at(8, 0, 10)).unwrap().len(),
        1,
        "07:30 was missed while the app was closed"
    );
}

#[test]
fn never_speaks_during_a_call_but_posts_a_card() {
    assert_eq!(Delivery::for_busy(true), Delivery::CardOnly);
    assert_eq!(Delivery::for_busy(false), Delivery::Speak);
}

#[test]
fn every_run_is_a_normal_conversation_in_history() {
    let mem = Arc::new(Memory::open_in_memory().unwrap());
    let s = Scheduler::new(mem.clone());
    s.ensure("Morning briefing", "30 7 * * *", &briefing(), at(6, 0, 10))
        .unwrap();
    let run = s.take_due(at(7, 30, 10)).unwrap().remove(0);
    let conv = s.open_run(&run).unwrap();
    let list = mem.conversations(5).unwrap();
    assert_eq!(list[0].id, conv);
    assert_eq!(list[0].source, "schedule");
    assert_eq!(list[0].title, "Morning briefing");
}

#[test]
fn a_bad_cron_line_is_refused() {
    let s = Scheduler::new(Arc::new(Memory::open_in_memory().unwrap()));
    assert!(
        s.ensure("x", "not a cron", &briefing(), at(6, 0, 10))
            .is_err()
    );
}

// ---------- Agents are files ----------

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("markon-agents-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

const STANDUP: &str = r#"
name = "Standup notes"
prompt = "Summarise yesterday's GitHub activity in two sentences."
tools = ["what_did_i_miss"]
max_rounds = 3
schedule = "0 9 * * 1-5"
"#;

#[test]
fn dropping_an_agent_file_schedules_it_and_removing_it_unschedules_it() {
    use markon_scheduler::files;
    let dir = tmp("drop");
    let s = Scheduler::new(Arc::new(Memory::open_in_memory().unwrap()));
    std::fs::write(dir.join("standup.toml"), STANDUP).unwrap();
    std::fs::write(
        dir.join("broken.toml"),
        "name = \"x\"\nprompt = \"y\"\nschedul = \"typo\"\n",
    )
    .unwrap();
    let (agents, problems) = files::load_dir(&dir);
    assert_eq!(agents.len(), 1);
    assert_eq!(problems.len(), 1, "unknown keys are reported: {problems:?}");
    s.sync_files(
        &agents.iter().map(|(_, f)| f.clone()).collect::<Vec<_>>(),
        at(8, 0, 10),
    )
    .unwrap();
    let list = s.list().unwrap();
    assert_eq!(list[0].name, "Standup notes");
    assert_eq!(list[0].agent.tools, ["what_did_i_miss"]);
    assert_eq!(list[0].agent.max_rounds, Some(3));
    assert_eq!(
        s.take_due(at(9, 0, 10)).unwrap().len(),
        1,
        "it runs at its time"
    );

    // Editing the file changes the schedule on the next sync.
    std::fs::write(dir.join("standup.toml"), STANDUP.replace("0 9", "30 9")).unwrap();
    let (agents, _) = files::load_dir(&dir);
    s.sync_files(
        &agents.iter().map(|(_, f)| f.clone()).collect::<Vec<_>>(),
        at(9, 5, 10),
    )
    .unwrap();
    assert_eq!(s.list().unwrap()[0].cron, "30 9 * * 1-5");

    std::fs::remove_file(dir.join("standup.toml")).unwrap();
    let (agents, _) = files::load_dir(&dir);
    s.sync_files(
        &agents.iter().map(|(_, f)| f.clone()).collect::<Vec<_>>(),
        at(9, 10, 10),
    )
    .unwrap();
    assert!(s.list().unwrap().is_empty());
}

#[test]
fn the_bundled_agents_parse_and_are_installed_once() {
    use markon_scheduler::{BUNDLED, files};
    for (name, text) in BUNDLED {
        files::parse(text).unwrap_or_else(|e| panic!("{name}: {e}"));
    }
    let dir = tmp("bundled");
    files::install_bundled(&dir, BUNDLED).unwrap();
    assert!(dir.join("morning-briefing.toml").exists());
    std::fs::remove_file(dir.join("morning-briefing.toml")).unwrap();
    files::install_bundled(&dir, BUNDLED).unwrap();
    assert!(
        !dir.join("morning-briefing.toml").exists(),
        "a file the user deleted stays deleted"
    );
}

#[test]
fn meeting_prep_is_due_about_ten_minutes_before_a_meeting_with_people() {
    use markon_scheduler::meeting_prep_due;
    let now = 1_000_000_000_000;
    let ev = |id: &str, mins: i64, people: bool| (id.to_string(), now + mins * 60_000, people);
    let events = [
        ev("a", 10, true),
        ev("b", 10, false),
        ev("c", 30, true),
        ev("d", 6, true),
    ];
    assert_eq!(meeting_prep_due(&events, now, &[]), ["a", "d"]);
    assert_eq!(meeting_prep_due(&events, now, &["a".into()]), ["d"]);
}

/// A dropped file runs with no rebuild, and cannot call a tool off its list.
#[tokio::test]
async fn a_file_agent_runs_and_cannot_call_a_tool_off_its_list() {
    use markon_agent::{Agent, AgentConfig, AgentEvent, ToolCall, ToolRuntime, UserTurn};
    use markon_provider::fake::FakeProvider;
    use markon_scheduler::files;
    struct Tools(std::sync::Mutex<Vec<String>>);
    impl ToolRuntime for Tools {
        fn specs(&self) -> Vec<markon_provider::ToolSpec> {
            ["what_did_i_miss", "run_command"]
                .iter()
                .map(|n| markon_provider::ToolSpec {
                    name: (*n).into(),
                    description: String::new(),
                    input_schema: serde_json::json!({"type": "object"}),
                })
                .collect()
        }
        fn call(&self, c: ToolCall) -> markon_provider::BoxFuture<'static, Result<String, String>> {
            self.0.lock().unwrap().push(c.name);
            Box::pin(async { Ok("ran".into()) })
        }
    }
    let dir = tmp("run");
    std::fs::write(dir.join("standup.toml"), STANDUP).unwrap();
    let s = Scheduler::new(Arc::new(Memory::open_in_memory().unwrap()));
    let (agents, _) = files::load_dir(&dir);
    s.sync_files(
        &agents.into_iter().map(|(_, f)| f).collect::<Vec<_>>(),
        at(8, 0, 10),
    )
    .unwrap();
    let due = s.take_due(at(9, 0, 10)).unwrap();
    let fake = Arc::new(FakeProvider::new(vec![
        FakeProvider::tools(&[
            ("1", "what_did_i_miss", serde_json::json!({})),
            (
                "2",
                "run_command",
                serde_json::json!({"command": "rm -rf ~"}),
            ),
        ]),
        FakeProvider::text("Two PRs merged."),
    ]));
    let tools = Arc::new(Tools(Default::default()));
    let (agent, mut rx) = Agent::new(fake, tools.clone(), AgentConfig::default());
    let spec = &due[0].agent;
    agent
        .submit(UserTurn::agent(
            &spec.prompt,
            spec.tools.clone(),
            spec.max_rounds,
        ))
        .await;
    while tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv())
        .await
        .is_ok_and(|e: Option<AgentEvent>| e.is_some())
    {}
    assert_eq!(
        *tools.0.lock().unwrap(),
        ["what_did_i_miss"],
        "run_command never reached the runtime"
    );
}

/// A proposal sits beside the original until the user approves it.
#[tokio::test]
async fn an_improvement_is_proposed_beside_the_original_and_applied_only_on_yes() {
    use markon_provider::fake::FakeProvider;
    use markon_scheduler::proposals;
    let dir = tmp("proposals");
    let file = dir.join("standup.toml");
    std::fs::write(&file, STANDUP).unwrap();
    let better = STANDUP.replace(
        "in two sentences",
        "in two sentences, naming each repository",
    );
    let fake = FakeProvider::new(vec![FakeProvider::text(&better)]);
    let diff = proposals::propose(&fake, "m", &file, "the user asked which repository twice")
        .await
        .unwrap()
        .unwrap();
    assert!(diff.contains("+ prompt = \"Summarise yesterday's GitHub activity in two sentences, naming each repository.\""), "{diff}");
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        STANDUP,
        "nothing changes before a yes"
    );
    assert_eq!(proposals::list(std::slice::from_ref(&dir)).len(), 1);
    proposals::approve(&file).unwrap();
    assert!(
        std::fs::read_to_string(&file)
            .unwrap()
            .contains("naming each repository")
    );
    assert!(
        dir.join("standup.toml.before").exists(),
        "the original is kept"
    );
    assert!(proposals::list(std::slice::from_ref(&dir)).is_empty());

    let broken = FakeProvider::new(vec![FakeProvider::text("name = \"x\"\nnot toml at all [")]);
    assert!(
        proposals::propose(&broken, "m", &file, "").await.is_err(),
        "a broken agent file is never proposed"
    );
    let same = FakeProvider::new(vec![FakeProvider::text(
        &std::fs::read_to_string(&file).unwrap(),
    )]);
    assert_eq!(
        proposals::propose(&same, "m", &file, "").await.unwrap(),
        None
    );
}

/// The bundled morning briefing, scheduled and run like any agent, cannot call a tool off its
/// list: the model is not offered it, and a call to it is refused before it reaches the runtime.
#[tokio::test]
async fn the_morning_briefing_cannot_call_a_tool_off_its_list() {
    use markon_agent::{Agent, AgentConfig, AgentEvent, ToolCall, ToolRuntime, UserTurn};
    use markon_provider::fake::FakeProvider;
    use markon_scheduler::{BUNDLED, files};
    struct Tools(std::sync::Mutex<Vec<String>>);
    impl ToolRuntime for Tools {
        fn specs(&self) -> Vec<markon_provider::ToolSpec> {
            [
                "what_did_i_miss",
                "fetch_page",
                "remind_me",
                "send_imessage",
                "run_command",
                "quit_app",
            ]
            .iter()
            .map(|n| markon_provider::ToolSpec {
                name: (*n).into(),
                description: String::new(),
                input_schema: serde_json::json!({"type": "object"}),
            })
            .collect()
        }
        fn call(&self, c: ToolCall) -> markon_provider::BoxFuture<'static, Result<String, String>> {
            self.0.lock().unwrap().push(c.name);
            Box::pin(async { Ok("ran".into()) })
        }
    }
    let dir = tmp("briefing-allow-list");
    files::install_bundled(&dir, BUNDLED).unwrap();
    let s = Scheduler::new(Arc::new(Memory::open_in_memory().unwrap()));
    let (agents, _) = files::load_dir(&dir);
    s.sync_files(
        &agents.into_iter().map(|(_, f)| f).collect::<Vec<_>>(),
        at(6, 0, 10),
    )
    .unwrap();
    let due = s.take_due(at(7, 30, 10)).unwrap();
    let briefing = due
        .iter()
        .find(|d| d.name == "Morning briefing")
        .expect("the briefing is due at 07:30");
    let fake = Arc::new(FakeProvider::new(vec![
        FakeProvider::tools(&[
            ("1", "what_did_i_miss", serde_json::json!({})),
            (
                "2",
                "send_imessage",
                serde_json::json!({"to": "everyone", "text": "hi"}),
            ),
            ("3", "quit_app", serde_json::json!({"name": "Mail"})),
        ]),
        FakeProvider::text("Good morning."),
    ]));
    let tools = Arc::new(Tools(Default::default()));
    let (agent, mut rx) = Agent::new(fake.clone(), tools.clone(), AgentConfig::default());
    let spec = &briefing.agent;
    agent
        .submit(UserTurn::agent(
            &spec.prompt,
            spec.tools.clone(),
            spec.max_rounds,
        ))
        .await;
    let mut refused = Vec::new();
    while let Ok(Some(e)) =
        tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await
    {
        if let AgentEvent::ToolCallFinished {
            name, ok: false, ..
        } = e
        {
            refused.push(name);
        }
    }
    assert_eq!(*tools.0.lock().unwrap(), ["what_did_i_miss"]);
    assert_eq!(refused, ["send_imessage", "quit_app"]);
    let offered: Vec<String> = fake.requests.lock().unwrap()[0]
        .tools
        .iter()
        .map(|t| t.name.clone())
        .collect();
    assert_eq!(offered, ["what_did_i_miss", "fetch_page", "remind_me"]);
}
