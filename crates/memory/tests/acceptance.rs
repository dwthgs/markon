//! Acceptance tests.
use markon_memory::{FactOrigin, Memory, Source};
use std::time::Instant;

fn mem() -> Memory {
    Memory::open_in_memory().unwrap()
}

#[test]
fn recall_returns_the_top_8_relevant_facts_in_under_20_ms() {
    let m = mem();
    let c = m.start_conversation(Source::Voice).unwrap();
    let t = m
        .add_turn(c, "user", "setup", 0, 0, "fake", "fake")
        .unwrap();
    for i in 0..2000 {
        m.add_fact(
            &format!("topic{i}"),
            &format!("Fact number {i} about gardening and tomatoes {i}"),
            t,
            0.9,
            FactOrigin::UserStated,
        )
        .unwrap();
    }
    for i in 0..20 {
        m.add_fact(
            &format!("coffee{i}"),
            &format!("Dev likes flat white coffee variant {i}"),
            t,
            0.9,
            FactOrigin::UserStated,
        )
        .unwrap();
    }
    // warm the statement cache, then measure
    m.recall("coffee", 8).unwrap();
    let t0 = Instant::now();
    let hits = m.recall("what coffee do I like?", 8).unwrap();
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(hits.len(), 8);
    assert!(hits.iter().all(|f| f.statement.contains("coffee")));
    assert!(ms < 20.0, "recall took {ms:.2} ms");
}

#[test]
fn model_guesses_cannot_be_stored_as_facts() {
    let m = mem();
    let c = m.start_conversation(Source::Text).unwrap();
    let t = m
        .add_turn(
            c,
            "assistant",
            "I think you live in Paris",
            0,
            0,
            "fake",
            "fake",
        )
        .unwrap();
    assert!(
        m.add_fact("home", "Lives in Paris", t, 0.5, FactOrigin::ModelGuess)
            .is_err()
    );
    assert!(m.recall("Paris", 8).unwrap().is_empty());
}

#[test]
fn forget_that_deletes_the_fact_and_anything_derived_only_from_it() {
    let m = mem();
    let c = m.start_conversation(Source::Voice).unwrap();
    let t = m
        .add_turn(c, "user", "my sister is Ana", 0, 0, "fake", "fake")
        .unwrap();
    let keep = m
        .add_fact(
            "dog",
            "Has a dog called Rex",
            t,
            1.0,
            FactOrigin::UserStated,
        )
        .unwrap();
    let f = m
        .add_fact(
            "sister",
            "Sister is called Ana",
            t,
            1.0,
            FactOrigin::UserStated,
        )
        .unwrap();
    let derived = m
        .add_derived_fact("birthday", "Ana's birthday card reminder", f)
        .unwrap();
    assert_eq!(m.forget_last(c).unwrap(), Some(f));
    assert!(m.fact(f).unwrap().is_none());
    assert!(m.fact(derived).unwrap().is_none());
    assert!(m.fact(keep).unwrap().is_some());
}

#[test]
fn newer_conflicting_facts_win_and_older_are_kept_as_superseded() {
    let m = mem();
    let c = m.start_conversation(Source::Voice).unwrap();
    let t = m.add_turn(c, "user", "x", 0, 0, "fake", "fake").unwrap();
    let old = m
        .add_fact("coffee", "Likes espresso", t, 1.0, FactOrigin::UserStated)
        .unwrap();
    let new = m
        .add_fact("coffee", "Likes flat white", t, 1.0, FactOrigin::UserStated)
        .unwrap();
    let hits = m.recall("likes", 8).unwrap();
    assert_eq!(hits.iter().map(|f| f.id).collect::<Vec<_>>(), vec![new]);
    let all = m.facts_about("coffee", true).unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(
        all.iter().find(|f| f.id == old).unwrap().superseded_by,
        Some(new)
    );
}

#[test]
fn history_reloads_cards_per_conversation() {
    let m = mem();
    let c1 = m.start_conversation(Source::Voice).unwrap();
    m.set_title(c1, "State of VR").unwrap();
    m.save_card(
        c1,
        "agent-1",
        "agent",
        "State of VR",
        &serde_json::json!({"status": "done"}),
    )
    .unwrap();
    m.save_card(
        c1,
        "result-1",
        "result",
        "State of VR",
        &serde_json::json!({"text": "42 blocks"}),
    )
    .unwrap();
    m.save_card(
        c1,
        "agent-1",
        "agent",
        "State of VR",
        &serde_json::json!({"status": "done", "v": 2}),
    )
    .unwrap();
    let c2 = m.start_conversation(Source::Schedule).unwrap();
    m.set_title(c2, "Morning briefing").unwrap();
    let list = m.conversations(10).unwrap();
    assert_eq!(list[0].id, c2);
    assert_eq!(list[1].title, "State of VR");
    let cards = m.cards(c1).unwrap();
    assert_eq!(cards.len(), 2);
    assert_eq!(cards[0].payload["v"], 2);
    assert_eq!(m.find_conversation("vr report").unwrap(), Some(c1));
}

#[test]
fn turns_and_usage_are_recorded() {
    let m = mem();
    let c = m.start_conversation(Source::Text).unwrap();
    m.add_turn(
        c,
        "user",
        "hello there",
        0,
        0,
        "anthropic",
        "claude-opus-5-5",
    )
    .unwrap();
    m.add_turn(c, "assistant", "hi", 120, 8, "anthropic", "claude-opus-5-5")
        .unwrap();
    m.record_usage("anthropic", "claude-opus-5-5", 120, 8)
        .unwrap();
    m.record_usage("anthropic", "claude-opus-5-5", 30, 2)
        .unwrap();
    let u = m.usage_today().unwrap();
    assert_eq!((u[0].tokens_in, u[0].tokens_out), (150, 10));
    assert_eq!(m.turns(c).unwrap().len(), 2);
}

#[test]
fn survives_a_crash_and_reopens_in_wal_mode() {
    let dir = std::env::temp_dir().join(format!("markon-mem-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("markon.db");
    {
        let m = Memory::open(&path).unwrap();
        let c = m.start_conversation(Source::Voice).unwrap();
        let t = m.add_turn(c, "user", "x", 0, 0, "f", "f").unwrap();
        m.add_fact("a", "Fact A", t, 1.0, FactOrigin::UserStated)
            .unwrap();
        std::mem::forget(m); // no clean close: like a crash
    }
    let m = Memory::open(&path).unwrap();
    assert_eq!(m.journal_mode().unwrap(), "wal");
    assert_eq!(m.recall("fact", 8).unwrap().len(), 1);
    assert_eq!(m.schema_version().unwrap(), markon_memory::SCHEMA_VERSION);
}

#[test]
fn export_and_full_wipe() {
    let m = mem();
    let c = m.start_conversation(Source::Voice).unwrap();
    let t = m.add_turn(c, "user", "x", 0, 0, "f", "f").unwrap();
    m.add_fact("a", "Fact A", t, 1.0, FactOrigin::UserStated)
        .unwrap();
    let dump = m.export().unwrap();
    assert_eq!(dump["facts"].as_array().unwrap().len(), 1);
    assert_eq!(dump["turns"].as_array().unwrap().len(), 1);
    m.wipe().unwrap();
    assert!(m.export().unwrap()["facts"].as_array().unwrap().is_empty());
    assert!(m.conversations(10).unwrap().is_empty());
}

#[test]
fn the_newest_current_facts_come_first_and_superseded_ones_are_left_out() {
    let m = mem();
    let c = m.start_conversation(Source::Voice).unwrap();
    let t = m.add_turn(c, "user", "x", 0, 0, "fake", "fake").unwrap();
    m.add_fact(
        "name",
        "The user's name is Dev",
        t,
        1.0,
        FactOrigin::UserStated,
    )
    .unwrap();
    m.add_fact("home", "Lives in Nairobi", t, 1.0, FactOrigin::UserStated)
        .unwrap();
    m.add_fact(
        "home",
        "Lives in Addis Ababa",
        t,
        1.0,
        FactOrigin::UserStated,
    )
    .unwrap();
    let got: Vec<String> = m
        .latest_facts(5)
        .unwrap()
        .into_iter()
        .map(|f| f.statement)
        .collect();
    assert_eq!(got, ["Lives in Addis Ababa", "The user's name is Dev"]);
}
