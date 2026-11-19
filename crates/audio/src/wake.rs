//! "Hey Markon" / "Hey Markan" / "Hey Mark" without a dedicated wake-word model: an energy detector cuts
//! short bursts of speech from an always-open microphone, whisper transcribes each burst on device, and
//! the text is matched against the wake phrase. Audio is never stored or sent anywhere.
use crate::barge::{self, Detector, Silero, VadState};
use crate::echo::Echo;
use crate::resample::{Streaming, downmix, resample};
use crate::service::{AudioEvent, EventSink, Playback, WHISPER_RATE};
use crate::vad::{EndOfSpeech, Vad};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Bursts shorter than this are clicks and coughs; longer ones are conversation, not a wake phrase.
/// A brisk "Hi Mark" measures about 290 ms of voiced sound, so the floor sits well under that.
const MIN_SEGMENT_MS: usize = 200;
const MAX_SEGMENT_MS: usize = 8000;
/// Unbroken sound (music, a film) has no pauses to cut at: whisper gets the latest few seconds
/// instead, with enough overlap that a wake phrase is never split across two windows.
const DRONE_WINDOW_MS: usize = 3000;
const DRONE_OVERLAP_MS: usize = 1000;
/// Audio kept from before the detector fires, so the "h" of "hey" is not lost.
const PRE_ROLL_MS: usize = 300;
/// Speech kept from before a barge-in, handed to the listening session so the user's first words
/// reach it (confirmation alone takes about 200 ms).
const BARGE_LEAD_MS: usize = 1500;
/// Markon's own voice still rings in the room just after it finishes: not the user.
const AFTER_SPEECH_MS: u64 = 400;

/// Whisper's guesses for noise or a mumble: not a person talking to Markon.
const FILLERS: &[&str] = &[
    "you",
    "thank you",
    "thanks",
    "bye",
    "uh",
    "um",
    "hmm",
    "mm",
    "oh",
    "ah",
    "huh",
    "so",
    "okay",
];

/// Real words in a transcript of audio heard over Markon.
pub fn is_words(text: &str) -> bool {
    let t: String = text
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace() || *c == '\'')
        .collect();
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    t.chars().filter(|c| c.is_alphabetic()).count() >= 2 && !FILLERS.contains(&t.as_str())
}

/// Jarvis `CANCEL_WORDS`: said on their own, always the user.
const CANCEL_WORDS: &[&str] = &["wait", "no", "stop", "cancel", "hold on"];

fn tokens(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '\''))
        .filter(|t| !t.is_empty())
        .map(String::from)
        .collect()
}

/// Jarvis `_is_stem_echo`: every word is a clipped form of one Markon said ("files" heard as
/// "file", "stopped" as "stop"). Four letters at least, and the two differ by at most four.
fn is_stem_echo(heard: &[String], said: &[String]) -> bool {
    heard.iter().all(|t| {
        said.iter().any(|r| {
            t.len() >= 4
                && r.len().abs_diff(t.len()) <= 4
                && (r.starts_with(t.as_str()) || t.starts_with(r.as_str()))
        })
    })
}

/// Markon hearing its own voice in what the microphone caught after a barge-in (Jarvis
/// `SpeechScheduler.classify`): no words at all, or 70% or more of the words are ones Markon was
/// saying with fewer than two new ones. A cancel word on its own is always the user. A single
/// other word that is only a clipped form of one of Markon's counts as its own (ASR drops endings).
pub fn is_own_words(heard: &str, said: &str) -> bool {
    let toks = tokens(heard);
    if toks.is_empty() {
        return true;
    }
    if CANCEL_WORDS.contains(&toks.join(" ").as_str()) {
        return false;
    }
    let said = tokens(said);
    if said.is_empty() {
        return false;
    }
    let matching = toks.iter().filter(|t| said.contains(t)).count();
    (matching * 10 >= toks.len() * 7 && toks.len() - matching < 2)
        || (toks.len() == 1 && is_stem_echo(&toks, &said))
}

/// Microphone audio (rate, mono samples) said over Markon, waiting for the listening session.
pub type Handoff = Arc<Mutex<(u32, Vec<f32>)>>;

/// A wake phrase is short: end the burst after this much silence (a request uses the longer pause).
/// 400 ms, down from 600: the answer to "Hi Mark" came too late. A comma pause in "Hey Markon, play
/// jazz" is usually shorter; a longer one wakes alone and the request follows "Yes?".
pub const WAKE_END_MS: usize = 400;

const GREETINGS: &[&str] = &["hey", "hi", "hay", "a", "okay", "ok", "hello"];
/// Whisper's usual spellings of the two names (its prompt already biases it towards them).
const NAMES: &[&str] = &[
    "markon", "markan", "marcon", "marcan", "markin", "marken", "markham", "markhan", "markun",
];

/// If `text` starts with the wake phrase, the rest of the utterance (possibly empty).
pub fn match_wake(text: &str) -> Option<String> {
    // A stray comma on its own (whisper writes ", Hey Mark." over music) is not a word.
    let raw: Vec<&str> = text
        .split_whitespace()
        .filter(|w| w.chars().any(char::is_alphanumeric))
        .collect();
    let words: Vec<String> = raw
        .iter()
        .map(|w| {
            w.to_lowercase()
                .trim_matches(|c: char| !c.is_alphanumeric())
                .to_string()
        })
        .collect();
    // The greeting is optional ("Markon, what's the time?"), but the name must come first or second.
    for start in 0..words.len().min(2) {
        if start == 1 && !GREETINGS.contains(&words[0].as_str()) {
            break;
        }
        let w = &words[start..];
        let first = w.first().map(String::as_str).unwrap_or("");
        let used = if NAMES.contains(&first) {
            1
        } else if start == 1 && ["mark", "mak", "mack", "mac"].contains(&first) {
            // Plain "Mark" or "Mak" (or whisper's "Mark on") only after a greeting: "Hey Mark", not "Mark said".
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

/// In a window of unbroken sound the phrase can follow other words (a lyric, a line of film):
/// a greeting and the name anywhere in it.
pub fn match_wake_within(text: &str) -> bool {
    let words: Vec<&str> = text.split_whitespace().collect();
    (0..words.len()).any(|i| {
        let first = words[i]
            .to_lowercase()
            .trim_matches(|c: char| !c.is_alphanumeric())
            .to_string();
        GREETINGS.contains(&first.as_str()) && match_wake(&words[i..].join(" ")).is_some()
    })
}

/// Cuts speech bursts out of a stream of mono samples.
pub struct Segmenter {
    rate: usize,
    vad: Vad,
    pre: Vec<f32>,
    cur: Option<Vec<f32>>,
    /// Samples of pre-roll at the start of `cur` (not speech).
    lead: usize,
    /// The sound has gone on too long to be one utterance: hand it over in windows.
    droning: bool,
}

impl Segmenter {
    pub fn new(rate: u32) -> Self {
        Self {
            rate: rate as usize,
            vad: Vad::with_end(rate, WAKE_END_MS),
            pre: Vec::new(),
            cur: None,
            lead: 0,
            droning: false,
        }
    }

    /// True while the bursts are windows of unbroken sound, where anything after the wake phrase is cut off.
    pub fn droning(&self) -> bool {
        self.droning
    }

    /// Feed about 10 ms. Returns a finished burst of plausible wake-phrase length, or a window of unbroken sound.
    pub fn push(&mut self, chunk: &[f32]) -> Option<Vec<f32>> {
        let ev = self.vad.push(chunk, false);
        if let Some(cur) = self.cur.as_mut() {
            cur.extend_from_slice(chunk);
        } else {
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
                let seg = self.cur.take()?;
                if std::mem::take(&mut self.droning) {
                    // What came since the last window: the sound may have ended with the wake phrase.
                    return Some(seg);
                }
                // Speech only: without the pre-roll and the closing silence.
                let tail = self.rate * WAKE_END_MS / 1000;
                let ms = seg.len().saturating_sub(self.lead + tail) * 1000 / self.rate;
                (MIN_SEGMENT_MS..=MAX_SEGMENT_MS)
                    .contains(&ms)
                    .then_some(seg)
            }
            None => {
                let limit = if self.droning {
                    DRONE_WINDOW_MS
                } else {
                    MAX_SEGMENT_MS
                };
                let cur = self.cur.as_mut()?;
                if cur.len() < self.rate * limit / 1000 {
                    return None;
                }
                self.droning = true;
                let window = cur[cur.len() - self.rate * DRONE_WINDOW_MS / 1000..].to_vec();
                cur.drain(..cur.len() - self.rate * DRONE_OVERLAP_MS / 1000);
                Some(window)
            }
        }
    }

    pub fn reset(&mut self) {
        self.vad = Vad::with_end(self.rate as u32, WAKE_END_MS);
        self.pre.clear();
        self.cur = None;
        self.droning = false;
    }
}

/// Runs until `stop`. Three modes: a listening session has the mic (do nothing); Markon is speaking
/// (cancel its echo and stop the moment the user talks over it); otherwise wait for the
/// wake phrase. `vad` is the verified Silero model; without it talking over Markon is off.
#[allow(clippy::too_many_arguments)]
pub(crate) fn wake_thread(
    stop: Arc<AtomicBool>,
    listening: Arc<AtomicBool>,
    attentive: Arc<AtomicBool>,
    play: Arc<Playback>,
    handoff: Handoff,
    sink: EventSink,
    vad: Option<PathBuf>,
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
    let mut detector = vad.filter(|_| echo.is_some()).and_then(|p| {
        Silero::load(&p)
            .map(Detector::new)
            .inspect_err(|e| {
                eprintln!("[barge] Silero did not load ({e}): talking over Markon is off")
            })
            .ok()
    });
    eprintln!(
        "[wake] listening at {rate} Hz, echo canceller {}, talking over Markon {}",
        if echo.is_some() { "on" } else { "off" },
        if detector.is_some() { "on" } else { "off" }
    );
    let chunk = echo
        .as_ref()
        .map(Echo::frame_len)
        .unwrap_or(rate as usize / 100);
    let mut to16 = Streaming::new(rate, barge::RATE, chunk);
    // Diagnostics for one reply: frames heard, false starts, the loudest frame.
    let (mut diag_frames, mut diag_starts, mut diag_rms) = (0usize, 0usize, 0f32);
    // Set from the barge-in until the listening session takes the microphone.
    let mut barged: Option<std::time::Instant> = None;
    let mut spoke: Option<std::time::Instant> = None;
    let mut recent: Vec<f32> = Vec::new();
    let keep = rate as usize * BARGE_LEAD_MS / 1000;
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
        // The canceller must hear the microphone between replies too. Fed only the speaker side, it
        // loses its alignment and leaves 70 to 98% of the echo in the first half second of the next
        // reply (echo::tests). A copy: the wake listener keeps the untouched signal. The cleaned
        // copy also teaches the detector the room's noise floor, only while Markon is silent.
        if !speaking && let Some(e) = echo.as_mut() {
            for c in pending[..whole].chunks_exact(chunk) {
                let mut c = c.to_vec();
                e.clean(&mut c);
                if let Some(d) = detector.as_mut() {
                    d.quiet(&to16.push(&c));
                }
            }
        }
        if speaking {
            // Markon's own voice must never wake it; only a person talking over it counts.
            seg.reset();
            spoke = Some(std::time::Instant::now());
            let sounding = play.sounding.load(Ordering::SeqCst);
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
                    let Some(d) = detector.as_mut() else {
                        continue;
                    };
                    diag_frames += 1;
                    diag_rms = diag_rms.max(barge::rms(c));
                    let before = d.state();
                    match d.push(&to16.push(c), sounding) {
                        // About 200 ms of speech: stop in this same tick, never resume.
                        VadState::Speaking => {
                            play.cut();
                            // The words that triggered it belong to the user's request: hand them over.
                            *handoff.lock().expect("handoff") = (rate, std::mem::take(&mut recent));
                            barged = Some(std::time::Instant::now());
                            eprintln!(
                                "[barge] stop: speech over Markon (trigger {:.4}, floor {:.4})",
                                d.trigger(),
                                d.floor()
                            );
                            sink(AudioEvent::BargeIn);
                        }
                        // Falter at once, like a person who hears someone start to speak.
                        VadState::Starting => {
                            diag_starts += usize::from(before == VadState::Quiet);
                            play.duck.store(true, Ordering::SeqCst);
                        }
                        _ => play.duck.store(false, Ordering::SeqCst),
                    }
                }
            }
            pending.drain(..whole);
            continue;
        }
        if diag_frames > 0 {
            eprintln!(
                "[barge] reply: {diag_frames} frames, {diag_starts} false starts, loudest {diag_rms:.3}"
            );
            (diag_frames, diag_starts, diag_rms) = (0, 0, 0.0);
        }
        if let Some(d) = detector.as_mut() {
            d.end_reply();
        }
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
        if spoke.is_some_and(|t| t.elapsed() < Duration::from_millis(AFTER_SPEECH_MS)) {
            seg.reset();
            pending.drain(..whole);
            continue;
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
        // Over music the window may end mid-request: wake only, and hear the request afresh.
        let woke = if seg.droning() {
            match_wake_within(&text).then(String::new)
        } else {
            // Mid-turn (thinking, or between sentences) the user need not say the name again.
            match_wake(&text).or_else(|| {
                (attentive.load(Ordering::SeqCst) && is_words(&text)).then(|| text.clone())
            })
        };
        if let Some(rest) = woke
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

    #[test]
    fn only_real_words_count_as_talking_over_markon() {
        assert!(super::is_words("wait, stop"));
        assert!(super::is_words("No."));
        assert!(!super::is_words(""));
        assert!(!super::is_words(" you "));
        assert!(!super::is_words("Thank you."));
        assert!(!super::is_words("Hmm"));
        assert!(!super::is_words("..."));
    }

    use super::*;

    const SAID: &str =
        "Your meeting with Sara is on Thursday at three. Deleting the staging files now.";

    #[test]
    fn markons_own_words_are_not_the_user() {
        assert!(is_own_words("", SAID));
        assert!(is_own_words("...", SAID));
        assert!(is_own_words("meeting with Sara is on Thursday", SAID));
        // One misheard word among its own is still its own.
        assert!(is_own_words("your meeting with Sarah is on Thursday", SAID));
        assert!(is_own_words("Thursday.", SAID));
        // A clipped form of one of its words.
        assert!(is_own_words("File.", SAID));
    }

    #[test]
    fn new_words_are_the_user() {
        assert!(!is_own_words("no, I meant Friday", SAID));
        assert!(!is_own_words("what about Friday instead", SAID));
        // Shares roots with what Markon is saying, but more than one word: a real request (Jarvis).
        assert!(!is_own_words("delete the file", SAID));
        // Nothing to compare with.
        assert!(!is_own_words("what time is it", ""));
    }

    #[test]
    fn a_cancel_word_is_always_the_user() {
        for w in ["Stop.", "wait", "No!", "cancel", "Hold on."] {
            assert!(
                !is_own_words(w, "No, stop, wait, cancel that, hold on."),
                "{w}"
            );
        }
    }

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
        assert_eq!(match_wake("Hi Mark."), Some(String::new()));
        assert_eq!(match_wake(", Hey Mark."), Some(String::new()));
        assert!(match_wake_within("dancing in the dark. Hey, Mark."));
        assert!(!match_wake_within("I told Mark on Monday, hey"));
        assert_eq!(match_wake("Hey Mak."), Some(String::new()));
        assert_eq!(match_wake("Hey, Mac!"), Some(String::new()));
    }

    #[test]
    fn ignores_ordinary_talk() {
        assert_eq!(match_wake("I told Mark on Monday"), None);
        assert_eq!(match_wake("Mark said he's coming"), None);
        assert_eq!(match_wake("Mac and cheese"), None);
        assert_eq!(match_wake("What did Markon say?"), None);
        assert_eq!(match_wake(""), None);
    }

    fn tone(ms: usize) -> Vec<f32> {
        (0..16 * ms)
            .map(|i| 0.2 * (i as f32 * 0.07).sin())
            .collect()
    }

    #[test]
    fn cuts_short_bursts_and_windows_unbroken_sound() {
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
        // A brisk "Hi Mark" is about 300 ms of sound: it must reach whisper.
        feed(&mut s, &[tone(300), hush.repeat(80)].concat());
        assert!((900..=1900).contains(&got[0]), "the 0.9 s burst: {got:?}");
        // Twelve seconds of music: three-second windows from 8 s on, every two seconds, then the tail.
        assert_eq!(got[1..4], [3000, 3000, 3000], "{got:?}");
        assert_eq!(got.len(), 6, "{got:?}");
        assert!(
            (300..=1300).contains(&got[5]),
            "the quick wake phrase: {got:?}"
        );
    }
}
