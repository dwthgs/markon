//! "Hey Markon" / "Hey Markan" / "Hey Mark" without a dedicated wake-word model: an energy detector cuts
//! short bursts of speech from an always-open microphone, whisper transcribes each burst on device, and
//! the text is matched against the wake phrase. Audio is never stored or sent anywhere.
use crate::echo::{Barge, BargeIn, Echo};
use crate::resample::{downmix, resample};
use crate::service::{AudioEvent, EventSink, Playback, WHISPER_RATE};
use crate::vad::{EndOfSpeech, Vad};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Bursts shorter than this are clicks and coughs; longer ones are conversation, not a wake phrase.
const MIN_SEGMENT_MS: usize = 400;
const MAX_SEGMENT_MS: usize = 8000;
/// Audio kept from before the detector fires, so the "h" of "hey" is not lost.
const PRE_ROLL_MS: usize = 300;
/// Speech kept from before a barge-in is confirmed (confirmation itself takes about 150 ms).
const BARGE_LEAD_MS: usize = 700;

/// Microphone audio (rate, mono samples) said over Markon, waiting for the listening session.
pub type Handoff = Arc<Mutex<(u32, Vec<f32>)>>;

/// A wake phrase is short: end the burst after this much silence (a request uses the longer pause).
pub const WAKE_END_MS: usize = 600;

const GREETINGS: &[&str] = &["hey", "hi", "hay", "a", "okay", "ok", "hello"];
/// Whisper's usual spellings of the two names (its prompt already biases it towards them).
const NAMES: &[&str] = &[
    "markon", "markan", "marcon", "marcan", "markin", "marken", "markham", "markhan", "markun",
];

/// If `text` starts with the wake phrase, the rest of the utterance (possibly empty).
pub fn match_wake(text: &str) -> Option<String> {
    let words: Vec<String> = text
        .split_whitespace()
        .map(|w| {
            w.to_lowercase()
                .trim_matches(|c: char| !c.is_alphanumeric())
                .to_string()
        })
        .collect();
    let raw: Vec<&str> = text.split_whitespace().collect();
    // The greeting is optional ("Markon, what's the time?"), but the name must come first or second.
    for start in 0..words.len().min(2) {
        if start == 1 && !GREETINGS.contains(&words[0].as_str()) {
            break;
        }
        let w = &words[start..];
        let first = w.first().map(String::as_str).unwrap_or("");
        let used = if NAMES.contains(&first) {
            1
        } else if start == 1 && first == "mark" {
            // Plain "Mark" (or whisper's "Mark on") only after a greeting: "Hey Mark", not "Mark said".
            if w.get(1)
                .is_some_and(|x| ["on", "an", "in", "un"].contains(&x.as_str()))
            {
                2
            } else {
                1
            }
        } else {
            continue;
        };
        let rest = raw[start + used..].join(" ");
        return Some(
            rest.trim_start_matches(|c: char| c.is_ascii_punctuation() || c.is_whitespace())
                .to_string(),
        );
    }
    None
}

/// Cuts speech bursts out of a stream of mono samples.
pub struct Segmenter {
    rate: usize,
    vad: Vad,
    pre: Vec<f32>,
    cur: Option<Vec<f32>>,
    /// Samples of pre-roll at the start of `cur` (not speech).
    lead: usize,
    /// A burst ran too long: ignore the rest of it.
    skipping: bool,
}

impl Segmenter {
    pub fn new(rate: u32) -> Self {
        Self {
            rate: rate as usize,
            vad: Vad::with_end(rate, WAKE_END_MS),
            pre: Vec::new(),
            cur: None,
            lead: 0,
            skipping: false,
        }
    }

    /// Feed about 10 ms. Returns a finished burst of plausible wake-phrase length.
    pub fn push(&mut self, chunk: &[f32]) -> Option<Vec<f32>> {
        let ev = self.vad.push(chunk, false);
        if let Some(cur) = self.cur.as_mut() {
            cur.extend_from_slice(chunk);
        } else if !self.skipping {
            self.pre.extend_from_slice(chunk);
            let keep = self.rate * PRE_ROLL_MS / 1000;
            if self.pre.len() > keep {
                self.pre.drain(..self.pre.len() - keep);
            }
        }
        match ev {
            Some(EndOfSpeech::Started) => {
                self.lead = self.pre.len();
                self.cur = Some(std::mem::take(&mut self.pre));
                None
            }
            Some(EndOfSpeech::Ended) => {
                let seg = self.cur.take();
                if std::mem::take(&mut self.skipping) {
                    return None;
                }
                let seg = seg?;
                // Speech only: without the pre-roll and the closing silence.
                let tail = self.rate * WAKE_END_MS / 1000;
                let ms = seg.len().saturating_sub(self.lead + tail) * 1000 / self.rate;
                (MIN_SEGMENT_MS..=MAX_SEGMENT_MS)
                    .contains(&ms)
                    .then_some(seg)
            }
            None => {
                // Too long to be a wake phrase: drop it and ignore the rest of this burst.
                if self
                    .cur
                    .as_ref()
                    .is_some_and(|c| c.len() > self.rate * MAX_SEGMENT_MS / 1000)
                {
                    self.cur = None;
                    self.skipping = true;
                }
                None
            }
        }
    }

    pub fn reset(&mut self) {
        self.vad = Vad::with_end(self.rate as u32, WAKE_END_MS);
        self.pre.clear();
        self.cur = None;
        self.skipping = false;
    }
}

/// Runs until `stop`. Three modes: a listening session has the mic (do nothing); Markon is speaking
/// (cancel its echo and watch for the user talking over it); otherwise wait for the wake phrase.
pub(crate) fn wake_thread(
    stop: Arc<AtomicBool>,
    listening: Arc<AtomicBool>,
    play: Arc<Playback>,
    handoff: Handoff,
    sink: EventSink,
    transcribe: impl Fn(&[f32]) -> Option<String>,
) {
    let host = cpal::default_host();
    let Some(dev) = host.default_input_device() else {
        return;
    };
    let Ok(cfg) = dev.default_input_config() else {
        return;
    };
    let channels = cfg.channels() as usize;
    let rate = cfg.sample_rate();
    let shared = Arc::new(Mutex::new(Vec::<f32>::new()));
    let s = shared.clone();
    let stream = dev.build_input_stream(
        cfg.into(),
        move |data: &[f32], _: &cpal::InputCallbackInfo| {
            if let Ok(mut b) = s.try_lock() {
                b.extend(downmix(data, channels));
            }
        },
        |e| eprintln!("[wake] input stream error: {e}"),
        None,
    );
    let Ok(stream) = stream else {
        eprintln!("[wake] cannot open the microphone");
        return;
    };
    if stream.play().is_err() {
        return;
    }
    let mut seg = Segmenter::new(rate);
    let mut echo = Echo::new(rate);
    if echo.is_none() {
        eprintln!("[wake] echo canceller unavailable at {rate} Hz: talking over Markon is off");
    }
    let mut barge = BargeIn::default();
    // Set from the barge-in until the listening session takes the microphone.
    let mut barged: Option<std::time::Instant> = None;
    let mut recent: Vec<f32> = Vec::new();
    let keep = rate as usize * BARGE_LEAD_MS / 1000;
    let chunk = echo
        .as_ref()
        .map(Echo::frame_len)
        .unwrap_or(rate as usize / 100);
    let mut pending: Vec<f32> = Vec::new();
    while !stop.load(Ordering::SeqCst) {
        // Faster while Markon speaks, so an interruption is heard at once.
        let tick = if play.active.load(Ordering::SeqCst) {
            10
        } else {
            30
        };
        std::thread::sleep(Duration::from_millis(tick));
        let fresh = std::mem::take(&mut *shared.lock().expect("wake buf"));
        let played = std::mem::take(&mut *play.render.lock().expect("render"));
        if listening.load(Ordering::SeqCst) {
            seg.reset();
            pending.clear();
            barged = None;
            continue;
        }
        let speaking = play.active.load(Ordering::SeqCst);
        if let Some(e) = echo.as_mut() {
            let out_rate = play.render_rate.load(Ordering::SeqCst);
            if out_rate == rate || out_rate == 0 {
                e.played(&played);
            } else {
                e.played(&resample(&played, out_rate, rate));
            }
        }
        pending.extend(fresh);
        let whole = pending.len() / chunk * chunk;
        if speaking {
            // Markon's own voice must never wake it; only a person talking over it counts.
            seg.reset();
            if let Some(e) = echo.as_mut() {
                for c in pending[..whole].chunks_exact_mut(chunk) {
                    e.clean(c);
                    if barged.is_some() {
                        handoff.lock().expect("handoff").1.extend_from_slice(c);
                        continue;
                    }
                    recent.extend_from_slice(c);
                    if recent.len() > keep {
                        recent.drain(..recent.len() - keep);
                    }
                    match barge.push(c) {
                        // Falter at once, like a person who hears someone start to speak.
                        Barge::Hint => play.duck.store(true, Ordering::SeqCst),
                        Barge::Quiet => play.duck.store(false, Ordering::SeqCst),
                        Barge::Confirmed => {
                            // Stop right here, without waiting for the app, then tell it.
                            play.cut();
                            // The words that triggered it belong to the user's request: hand them over.
                            *handoff.lock().expect("handoff") = (rate, std::mem::take(&mut recent));
                            barged = Some(std::time::Instant::now());
                            sink(AudioEvent::BargeIn);
                        }
                    }
                }
            }
            pending.drain(..whole);
            continue;
        }
        barge.reset();
        recent.clear();
        play.duck.store(false, Ordering::SeqCst);
        if let Some(at) = barged {
            // Markon has stopped; keep collecting until the listening session opens the mic.
            if at.elapsed() < Duration::from_secs(3) {
                handoff
                    .lock()
                    .expect("handoff")
                    .1
                    .extend_from_slice(&pending[..whole]);
                pending.drain(..whole);
                continue;
            }
            barged = None;
            handoff.lock().expect("handoff").1.clear();
        }
        let mut burst = None;
        for c in pending[..whole].chunks(chunk) {
            if let Some(b) = seg.push(c) {
                burst = Some(b);
            }
        }
        pending.drain(..whole);
        let Some(b) = burst else { continue };
        let Some(text) = transcribe(&resample(&b, rate, WHISPER_RATE)) else {
            continue;
        };
        if let Some(rest) = match_wake(&text)
            && !listening.load(Ordering::SeqCst)
            && !play.active.load(Ordering::SeqCst)
        {
            sink(AudioEvent::Wake { rest });
        }
    }
    drop(stream);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_wake_phrase_and_keeps_the_request() {
        assert_eq!(match_wake("Hey Markon."), Some(String::new()));
        assert_eq!(match_wake("Hey, Markan!"), Some(String::new()));
        assert_eq!(
            match_wake("Hey Markon, what's the weather?"),
            Some("what's the weather?".into())
        );
        assert_eq!(
            match_wake("hey mark on what time is it"),
            Some("what time is it".into())
        );
        assert_eq!(
            match_wake("Markon, set a timer"),
            Some("set a timer".into())
        );
        assert_eq!(match_wake("Hi Marcan."), Some(String::new()));
        assert_eq!(match_wake("Hey Mark."), Some(String::new()));
        assert_eq!(
            match_wake("Hey Mark, are you there?"),
            Some("are you there?".into())
        );
    }

    #[test]
    fn ignores_ordinary_talk() {
        assert_eq!(match_wake("I told Mark on Monday"), None);
        assert_eq!(match_wake("Mark said he's coming"), None);
        assert_eq!(match_wake("What did Markon say?"), None);
        assert_eq!(match_wake(""), None);
    }

    fn tone(ms: usize) -> Vec<f32> {
        (0..16 * ms)
            .map(|i| 0.2 * (i as f32 * 0.07).sin())
            .collect()
    }

    #[test]
    fn cuts_short_bursts_and_skips_long_talk() {
        let mut s = Segmenter::new(16_000);
        let hush = vec![0.0f32; 160];
        let mut got = Vec::new();
        let mut feed = |s: &mut Segmenter, pcm: &[f32]| {
            for c in pcm.chunks(160) {
                if let Some(b) = s.push(c) {
                    got.push(b.len() / 16);
                }
            }
        };
        feed(
            &mut s,
            &[hush.repeat(30), tone(900), hush.repeat(80)].concat(),
        );
        feed(&mut s, &[tone(100), hush.repeat(80)].concat());
        feed(&mut s, &[tone(12_000), hush.repeat(80)].concat());
        assert_eq!(got.len(), 1, "only the 0.9 s burst: {got:?}");
        assert!((900..=1900).contains(&got[0]), "{got:?}");
    }
}
