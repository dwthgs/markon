//! Measures the latency criteria with the real, checksum-verified models.
//! Usage: cargo run --release -p markon-audio --example latency -- <models dir> <16 kHz wav>
use markon_audio::service::{Audio, AudioEvent};
use markon_audio::stt::{SttEngine, Whisper};
use markon_audio::tts::{TtsEngine, split_first_chunk};
use markon_audio::{Voice, manifest::Manifest, resample::resample};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn read_wav16(path: &str) -> Vec<f32> {
    let b = std::fs::read(path).expect("wav");
    let data = b.windows(4).position(|w| w == b"data").expect("data chunk") + 8;
    b[data..]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| i16::from_le_bytes(*c) as f32 / 32768.0)
        .collect()
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let dir = PathBuf::from(&a[1]);
    let m = Manifest::bundled();

    let t = Instant::now();
    m.verify(&dir).expect("all files match the manifest");
    println!("manifest verify (sha256 of all files): {:?}", t.elapsed());

    // Final transcript: the clip as a 48 kHz mic would deliver it, resampled then transcribed.
    let clip = read_wav16(&a[2]);
    let mic = resample(&clip, 16_000, 48_000);
    let mut w = Whisper::load(
        &m.entry("whisper-base.en").unwrap().verify(&dir).unwrap(),
        "",
    )
    .unwrap();
    for run in 0..3 {
        let t = Instant::now();
        let text = w.transcribe(&resample(&mic, 48_000, 16_000)).unwrap();
        println!(
            "final transcript {:.2} s clip, run {run}: {:?} -> {text}",
            clip.len() as f32 / 16000.0,
            t.elapsed()
        );
    }

    let events: Arc<Mutex<Vec<(Instant, AudioEvent)>>> = Arc::new(Mutex::new(Vec::new()));
    let ev = events.clone();
    let audio = Audio::new(Arc::new(move |e| {
        if !matches!(e, AudioEvent::Features { .. }) {
            ev.lock().unwrap().push((Instant::now(), e));
        }
    }));
    let t = Instant::now();
    audio
        .load_models(&dir, Voice::Markon, "")
        .expect("models load");
    println!("load whisper + kokoro + voice: {:?}", t.elapsed());

    // First audio: from a complete first sentence to playback start (synthesis of the first chunk + device open).
    for sentence in [
        "Good morning, Dev. You have three meetings today and the second one clashes with lunch.",
        "Sure, I can put together a short report on the state of virtual reality headsets.",
    ] {
        events.lock().unwrap().clear();
        audio.begin_reply();
        let t = Instant::now();
        audio.speak(sentence).unwrap();
        let started = loop {
            if let Some((at, _)) = events
                .lock()
                .unwrap()
                .iter()
                .find(|(_, e)| *e == AudioEvent::PlaybackStarted)
            {
                break *at;
            }
            std::thread::sleep(Duration::from_millis(1));
        };
        println!(
            "first audio for {:?}: {:?} (first chunk {:?})",
            sentence,
            started - t,
            split_first_chunk(sentence).0
        );
        std::thread::sleep(Duration::from_millis(900));
        let c = Instant::now();
        audio.cancel_speech();
        println!(
            "cancel_speech returned in {:?}; the device stops at its next callback (one buffer)",
            c.elapsed()
        );
        std::thread::sleep(Duration::from_millis(400));
    }

    // Raw synthesis speed of the first chunk, for reference.
    let mut k = markon_audio::tts::Kokoro::load(
        &m.entry("kokoro-v1").unwrap().verify(&dir).unwrap(),
        &m.entry("voice-markon").unwrap().verify(&dir).unwrap(),
        markon_audio::g2p::G2p::load(
            &dir.join("g2p/gb_gold.json"),
            &dir.join("g2p/gb_silver.json"),
        )
        .unwrap(),
    )
    .unwrap();
    for run in 0..3 {
        let t = Instant::now();
        let s = k.synth("Good morning,").unwrap();
        println!(
            "synth 'Good morning,' run {run}: {:?} for {:.2} s",
            t.elapsed(),
            s.len() as f32 / 24000.0
        );
    }
}
