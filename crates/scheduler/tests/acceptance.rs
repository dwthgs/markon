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
