//! Audio acceptance tests that run without a microphone, speakers or model files.
//! Latency that needs the real models is measured by `examples/latency.rs`.
use markon_audio::features::{FEATURE_HZ, FeatureExtractor, NB};
use markon_audio::manifest::{Manifest, ManifestError};
use markon_audio::sentences::SentenceQueue;
use markon_audio::vad::{EndOfSpeech, Vad};

const SR: u32 = 16_000;

fn sine(hz: f32, secs: f32, amp: f32) -> Vec<f32> {
    (0..(SR as f32 * secs) as usize)
        .map(|i| amp * (std::f32::consts::TAU * hz * i as f32 / SR as f32).sin())
        .collect()
}

#[test]
fn features_are_published_at_30_hz_with_48_bands() {
    let mut fx = FeatureExtractor::new(SR);
    let frames: Vec<_> = sine(200.0, 2.0, 0.3)
        .chunks(160)
        .filter_map(|c| fx.push(c))
        .collect();
    assert_eq!(FEATURE_HZ, 30);
    assert!(
        (58..=61).contains(&frames.len()),
        "{} frames in 2 s",
        frames.len()
    );
    assert!(frames.iter().all(|f| f.bands.len() == NB && NB == 48));
}

#[test]
fn feature_latency_is_under_50_ms() {
    // A frame covers at most one hop of new audio plus the analysis window.
    let fx = FeatureExtractor::new(SR);
    assert!(fx.latency_ms() < 50.0, "{} ms", fx.latency_ms());
}

#[test]
fn features_track_loudness_pitch_and_spectrum() {
    let mut fx = FeatureExtractor::new(SR);
    let f = sine(220.0, 0.5, 0.3)
        .chunks(160)
        .filter_map(|c| fx.push(c))
        .last()
        .unwrap();
    assert!(f.rms > 0.5, "rms {}", f.rms);
    assert!((f.pitch - 220.0).abs() < 8.0, "pitch {}", f.pitch);
    let near = |hz: f32| {
        (0..NB)
            .min_by(|a, b| {
                let d = |k: usize| (markon_audio::features::band_hz(k) / hz).log2().abs();
                d(*a).total_cmp(&d(*b))
            })
            .unwrap()
    };
    assert!(
        f.bands[near(220.0)] > 0.9,
        "220 Hz band {}",
        f.bands[near(220.0)]
    );
    assert!(
        f.bands[near(2000.0)] < 0.3,
        "2 kHz band {}",
        f.bands[near(2000.0)]
    );
    let mut fx = FeatureExtractor::new(SR);
    let hi = sine(2000.0, 0.5, 0.3)
        .chunks(160)
        .filter_map(|c| fx.push(c))
        .last()
        .unwrap();
    assert!(
        hi.bands[near(2000.0)] > 0.9 && hi.bands[near(220.0)] < 0.3,
        "2 kHz tone lights the 2 kHz band only"
    );

    let mut fx = FeatureExtractor::new(SR);
    let quiet = vec![0.0; 8000]
        .chunks(160)
        .filter_map(|c| fx.push(c))
        .last()
        .unwrap();
    assert_eq!(quiet.rms, 0.0);
    assert_eq!(quiet.pitch, 0.0);
    assert!(quiet.bands.iter().all(|b| *b == 0.0));
}

#[test]
fn end_of_speech_waits_700_ms_or_1_2_s_mid_clause() {
    let mut vad = Vad::new(SR);
    let talk = sine(180.0, 1.0, 0.2);
    let hush = vec![0.0f32; 160];
    let mut started = false;
    for c in talk.chunks(160) {
        match vad.push(c, false) {
            Some(EndOfSpeech::Started) => started = true,
            Some(EndOfSpeech::Ended) => panic!("ended while talking"),
            None => {}
        }
    }
    assert!(started);
    let mut ms = 0;
    let end = loop {
        ms += 10;
        if let Some(e) = vad.push(&hush, false) {
            break e;
        }
        assert!(ms < 2000);
    };
    assert_eq!(end, EndOfSpeech::Ended);
    assert!((690..=720).contains(&ms), "ended after {ms} ms");

    let mut vad = Vad::new(SR);
    for c in talk.chunks(160) {
        vad.push(c, true);
    }
    let mut ms = 0;
    while vad.push(&hush, true) != Some(EndOfSpeech::Ended) {
        ms += 10;
    }
    assert!(
        (1190..=1220).contains(&ms),
        "mid-clause ended after {ms} ms"
    );
}

#[test]
fn sentences_stream_out_as_soon_as_they_are_complete() {
    let mut q = SentenceQueue::new(8);
    assert!(q.push("Good morning, Dev").is_empty());
    assert_eq!(q.push(". You have three meet"), vec!["Good morning, Dev."]);
    assert_eq!(
        q.push("ings today. Want me to move the 2.5 pm one?"),
        vec!["You have three meetings today."]
    );
    assert_eq!(
        q.finish(),
        Some("Want me to move the 2.5 pm one?".to_string())
    );
}

#[test]
fn a_very_long_reply_caps_the_queue_and_keeps_the_oldest() {
    let mut q = SentenceQueue::new(3);
    let out = q.push("One. Two. Three. Four. Five. ");
    assert_eq!(out, vec!["One.", "Two.", "Three."]);
    assert!(q.dropped() >= 2);
}

#[test]
fn a_manifest_mismatch_refuses_to_speak() {
    let dir = std::env::temp_dir().join(format!("markon-manifest-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("voices")).unwrap();
    std::fs::write(dir.join("voices/v.bin"), b"not the pinned voice").unwrap();
    let m: Manifest = serde_json::from_str(
        r#"{"version":1,"files":[{"id":"voice-markon","path":"voices/v.bin","url":"https://example.invalid/v.bin",
           "size":20,"sha256":"0000000000000000000000000000000000000000000000000000000000000000","license":"x"}]}"#,
    )
    .unwrap();
    match m.verify(&dir) {
        Err(ManifestError::Mismatch { id, .. }) => assert_eq!(id, "voice-markon"),
        other => panic!("expected mismatch, got {other:?}"),
    }
    std::fs::remove_file(dir.join("voices/v.bin")).unwrap();
    assert!(matches!(m.verify(&dir), Err(ManifestError::Missing { .. })));
}
