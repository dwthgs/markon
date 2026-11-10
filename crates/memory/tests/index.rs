//! Chunks, keyword and meaning search fused, one person across sources.
use markon_memory::Memory;
use markon_memory::index::{MAX_CHARS, OVERLAP_CHARS, chunk, rrf};

#[test]
fn long_text_is_cut_into_overlapping_chunks_under_the_cap() {
    let para = "The quarterly report covers revenue, costs and hiring. ".repeat(60);
    let text = format!("Intro line.\n\n{para}\n\nClosing line about the dentist on Thursday.");
    let c = chunk(&text, MAX_CHARS, OVERLAP_CHARS);
    assert!(c.len() >= 3, "{}", c.len());
    assert!(c.iter().all(|x| x.len() <= MAX_CHARS + OVERLAP_CHARS));
    assert!(c.last().unwrap().contains("dentist"));
    let tail = &c[1][c[1].len() - 40..];
    assert!(
        c[2].starts_with(tail.split_whitespace().nth(1).unwrap()) || c[2].contains(tail.trim()),
        "the next chunk starts with the last one's tail"
    );
}

#[test]
fn rrf_rewards_agreement_between_the_two_rankings() {
    let fused = rrf(&[(0.5, vec![1, 2, 3]), (0.5, vec![3, 4, 1])], 60.0);
    assert_eq!(fused[0].0, 1, "{fused:?}");
    assert_eq!(fused[1].0, 3);
}

/// A toy embedder: "dental" and "dentist" land together, which keyword search alone would miss.
fn toy(text: &str) -> Option<Vec<f32>> {
    let t = text.to_lowercase();
    Some(vec![
        f32::from(t.contains("dentist") || t.contains("dental") || t.contains("teeth")),
        f32::from(t.contains("budget") || t.contains("revenue")),
        0.1,
    ])
}

#[test]
fn meaning_search_finds_what_keywords_miss_and_both_are_fused() {
    let m = Memory::open_in_memory().unwrap();
    m.index(
        "files",
        "/n/teeth.md",
        "Check-up",
        1,
        "Dental check-up booked for Thursday at nine.",
        Some(&toy),
    )
    .unwrap();
    m.index(
        "files",
        "/n/money.md",
        "Q3",
        1,
        "The budget grew; revenue is up.",
        Some(&toy),
    )
    .unwrap();
    m.index(
        "imessage",
        "imessage:7",
        "Sara",
        1,
        "Can you bring the dentist forms?",
        Some(&toy),
    )
    .unwrap();
    let keyword = m.search("dentist appointment", None, 5).unwrap();
    assert_eq!(keyword.len(), 1, "keywords alone find one: {keyword:?}");
    let q = toy("dentist appointment").unwrap();
    let hybrid = m.search("dentist appointment", Some(&q), 5).unwrap();
    let refs: Vec<&str> = hybrid.iter().map(|h| h.reference.as_str()).collect();
    assert_eq!(
        refs[0], "imessage:7",
        "found by both rankings, so first: {refs:?}"
    );
    assert!(refs.contains(&"/n/teeth.md"), "found by meaning only");
    // Re-indexing replaces, it does not duplicate; taking a folder away removes it.
    m.index(
        "files",
        "/n/teeth.md",
        "Check-up",
        2,
        "Moved to Friday.",
        Some(&toy),
    )
    .unwrap();
    assert_eq!(m.indexed_mtime("/n/teeth.md").unwrap(), Some(2));
    assert_eq!(m.unindex_where("/n/").unwrap(), 2);
    assert_eq!(m.chunk_count().unwrap(), 1);
}

#[test]
fn one_person_across_every_source() {
    let m = Memory::open_in_memory().unwrap();
    m.link_person("Sara Tesfaye", "phone", "+1 (415) 555-0100")
        .unwrap();
    m.link_person("Sara Tesfaye", "email", "Sara@Work.example")
        .unwrap();
    m.link_person("sara tesfaye", "slack", "@sarat").unwrap();
    assert_eq!(
        m.person_for("phone", "4155550100").unwrap().as_deref(),
        Some("Sara Tesfaye")
    );
    assert_eq!(
        m.person_for("email", "sara@work.example")
            .unwrap()
            .as_deref(),
        Some("Sara Tesfaye")
    );
    assert_eq!(
        m.identities("Sara Tesfaye").unwrap().len(),
        3,
        "the same Sara, whatever the case"
    );
    assert_eq!(m.person_for("slack", "@someone").unwrap(), None);
}

#[test]
fn wiping_clears_the_index_and_people_too() {
    let m = Memory::open_in_memory().unwrap();
    m.index("files", "/a", "a", 1, "alpha", None).unwrap();
    m.link_person("Sam", "email", "sam@example.com").unwrap();
    m.wipe().unwrap();
    assert_eq!(m.chunk_count().unwrap(), 0);
    assert_eq!(m.person_for("email", "sam@example.com").unwrap(), None);
}
