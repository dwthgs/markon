//! Talking over Markon: is the user speaking in the echo-cancelled microphone signal?
//!
//! Silero voice detection with a four-state machine, a noise floor learned only while Markon is
//! silent, and a grace window when its voice starts. All audio here is mono 16 kHz.
use crate::AudioError;
use ort::session::Session;
use ort::value::Tensor;
use std::collections::VecDeque;
use std::path::Path;

pub const RATE: u32 = 16_000;
/// Silero's window at 16 kHz, and the samples of the previous window prepended to it.
pub const WINDOW: usize = 512;
const CONTEXT: usize = 64;
const WINDOW_MS: f32 = WINDOW as f32 * 1000.0 / RATE as f32;

// Voice detection parameters.
const CONFIDENCE: f32 = 0.7;
const SMOOTHING: f32 = 0.2;
const START_SECS: f32 = 0.2;
const STOP_SECS: f32 = 0.2;
// The recurrent state is reset every 2 s of audio, once 5 windows in a row
// score under 0.1 while the detector is quiet.
const RESET_SECS: f32 = 2.0;
const RESET_MAX_CONFIDENCE: f32 = 0.1;
const RESET_LOW_WINDOWS: usize = 5;

// Floor = 90th percentile of the last 100 quiet blocks, trigger = floor x 3,
// 500 ms of grace when playback starts after more than about a second of silence.
const AMBIENT_BLOCKS: usize = 100;
const MULTIPLIER: f32 = 3.0;
const GRACE_MS: f32 = 500.0;
const GAP_MS: f32 = 1000.0;
// After AEC3 Markon's leftover echo sits near 0.002 RMS and the user's voice peaks near 0.05 with
// syllable dips to 0.01 (measured 2026-11-12, echo.rs history). The trigger never drops under 0.006
// (three times that echo) and a noisy room never lifts it over 0.015. With 0.008
// the real model confirmed speech at 0.015 RMS only after 640 ms, with 0.006 after 220 ms.
const FLOOR_MIN: f32 = 0.002;
const TRIGGER_MIN: f32 = 0.006;
const TRIGGER_MAX: f32 = 0.015;
// Turned up, the Mac's speakers left Markon's voice at 0.1 RMS after AEC3 (2026-11-20) and it cut
// itself off. The grace window measures how much of what Markon plays comes back (echo / played,
// both at their peak). The bar then follows what is playing now: high on a loud word, back to the
// room's trigger in the gaps between words, where people cut in. A fixed bar at twice the peak echo
// let the user through only after seconds of shouting.
const ECHO_MARGIN: f32 = 1.5;
/// Per 32 ms window: the played level fades over about 150 ms, as the echo's tail does.
const PLAYED_DECAY: f32 = 0.8;
const PLAYED_MIN: f32 = 0.005;

fn windows(ms: f32) -> usize {
    (ms / WINDOW_MS).round() as usize
}

pub fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

/// Speech probability of one 512-sample window. The model is recurrent: windows must come in order.
pub trait Scorer: Send {
    fn score(&mut self, window: &[f32]) -> f32;
    fn reset(&mut self);
}

/// Silero VAD (v6.2, pinned in the manifest).
pub struct Silero {
    session: Session,
    state: Vec<f32>,
    context: Vec<f32>,
}

impl Silero {
    /// The path must already be verified against the manifest.
    pub fn load(model: &Path) -> Result<Self, AudioError> {
        let env = crate::onnx_env().map_err(AudioError::Model)?;
        let fail = |e: String| AudioError::Model(format!("silero: {e}"));
        let session = Session::builder(env)
            .map_err(|e| fail(e.to_string()))?
            .with_intra_threads(1)
            .map_err(|e| fail(e.to_string()))?
            .commit_from_file(model)
            .map_err(|e| fail(e.to_string()))?;
        let mut s = Self {
            session,
            state: Vec::new(),
            context: Vec::new(),
        };
        s.reset();
        Ok(s)
    }

    fn run(&mut self, window: &[f32]) -> Result<f32, ort::Error> {
        let mut input = Vec::with_capacity(CONTEXT + WINDOW);
        input.extend_from_slice(&self.context);
        input.extend_from_slice(window);
        self.context = input[input.len() - CONTEXT..].to_vec();
        let out = self.session.run(ort::inputs![
            "input" => Tensor::from_array(([1usize, CONTEXT + WINDOW], input))?,
            "state" => Tensor::from_array(([2usize, 1, 128], std::mem::take(&mut self.state)))?,
            "sr" => Tensor::from_array(([0usize; 0], vec![RATE as i64]))?,
        ])?;
        let (_, p) = out["output"].try_extract_tensor::<f32>()?;
        let (_, s) = out["stateN"].try_extract_tensor::<f32>()?;
        self.state = s.to_vec();
        Ok(p.first().copied().unwrap_or(0.0))
    }
}

impl Scorer for Silero {
    fn score(&mut self, window: &[f32]) -> f32 {
        self.run(window).unwrap_or_else(|e| {
            // A failed call scores 0. The state is gone with the failed call: start afresh.
            eprintln!("[barge] silero failed: {e}");
            self.reset();
            0.0
        })
    }

    fn reset(&mut self) {
        self.state = vec![0.0; 2 * 128];
        self.context = vec![0.0; CONTEXT];
    }
}

/// Voice detection state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadState {
    Quiet,
    Starting,
    Speaking,
    Stopping,
}

pub struct Detector<S> {
    scorer: S,
    pending: Vec<f32>,
    state: VadState,
    starting: usize,
    stopping: usize,
    since_reset: usize,
    low: usize,
    ambient: VecDeque<f32>,
    floor: f32,
    sounding_before: bool,
    sounded: bool,
    silent: usize,
    grace: usize,
    quiet_pending: Vec<f32>,
    volume: f32,
    /// Peaks in this reply's grace window: leftover echo, and Markon's own output.
    echo_peak: f32,
    played_peak: f32,
    /// Markon's output level now, with the echo's tail.
    played: f32,
}

impl<S: Scorer> Detector<S> {
    pub fn new(scorer: S) -> Self {
        Self {
            scorer,
            pending: Vec::new(),
            state: VadState::Quiet,
            starting: 0,
            stopping: 0,
            since_reset: 0,
            low: 0,
            ambient: VecDeque::new(),
            floor: FLOOR_MIN,
            sounding_before: false,
            sounded: false,
            silent: 0,
            grace: 0,
            quiet_pending: Vec::new(),
            volume: 0.0,
            echo_peak: 0.0,
            played_peak: 0.0,
            played: 0.0,
        }
    }

    pub fn state(&self) -> VadState {
        self.state
    }

    /// The room's level, learned only while Markon is silent.
    pub fn floor(&self) -> f32 {
        self.floor
    }

    /// The level a window must reach to count as speech.
    pub fn trigger(&self) -> f32 {
        (self.floor * MULTIPLIER).clamp(TRIGGER_MIN, TRIGGER_MAX)
    }

    /// The trigger, raised over the echo of what Markon is playing now.
    pub fn gate(&self) -> f32 {
        let ratio = if self.played_peak > PLAYED_MIN {
            self.echo_peak / self.played_peak
        } else {
            0.0
        };
        self.trigger().max(ratio * self.played * ECHO_MARGIN)
    }

    /// Cleaned microphone audio heard while no reply is in progress: it teaches the floor. A block
    /// is added only when it is under the trigger, so a person talking never raises it.
    pub fn quiet(&mut self, pcm: &[f32]) {
        self.quiet_pending.extend_from_slice(pcm);
        let (windows, _) = self.quiet_pending.as_chunks::<WINDOW>();
        let whole = windows.len() * WINDOW;
        for w in windows {
            let level = rms(w);
            if level < self.trigger() {
                if self.ambient.len() == AMBIENT_BLOCKS {
                    self.ambient.pop_front();
                }
                self.ambient.push_back(level);
                self.floor = percentile90(&self.ambient).max(FLOOR_MIN);
            }
        }
        self.quiet_pending.drain(..whole);
    }

    /// Cleaned audio while a reply is in progress. `sounding` is true while Markon's voice is leaving
    /// the speaker now, not merely while a reply is running; `played` is that voice's RMS level.
    /// Returns the state after the last window.
    pub fn push(&mut self, pcm: &[f32], sounding: bool, played: f32) -> VadState {
        self.pending.extend_from_slice(pcm);
        let pending = std::mem::take(&mut self.pending);
        let (windows, rest) = pending.as_chunks::<WINDOW>();
        for w in windows {
            self.window(w, sounding, played);
        }
        self.pending = rest.to_vec();
        self.state
    }

    /// One window through the detector, with the grace and floor as the volume gate.
    fn window(&mut self, w: &[f32], sounding: bool, played: f32) {
        self.track_playback(sounding);
        self.played = played.max(self.played * PLAYED_DECAY);
        let confidence = self.scorer.score(w);
        self.maybe_reset_model(confidence);
        // The volume is smoothed (factor 0.2) so the dip at the end of a word does not reset STARTING.
        self.volume += SMOOTHING * (rms(w) - self.volume);
        let in_grace = self.grace > 0;
        if in_grace && sounding {
            self.echo_peak = self.echo_peak.max(self.volume);
            self.played_peak = self.played_peak.max(self.played);
        }
        let loud = self.volume >= self.gate();
        self.grace = self.grace.saturating_sub(1);
        let speaking = confidence >= CONFIDENCE && loud && !in_grace;
        use VadState::*;
        if speaking {
            match self.state {
                Quiet => {
                    self.state = Starting;
                    self.starting = 1;
                }
                Starting => self.starting += 1,
                Stopping => {
                    self.state = Speaking;
                    self.stopping = 0;
                }
                Speaking => {}
            }
        } else {
            match self.state {
                Starting => {
                    self.state = Quiet;
                    self.starting = 0;
                }
                Speaking => {
                    self.state = Stopping;
                    self.stopping = 1;
                }
                Stopping => self.stopping += 1,
                Quiet => {}
            }
        }
        if self.state == Starting && self.starting >= windows(START_SECS * 1000.0) {
            self.state = Speaking;
            self.starting = 0;
        }
        if self.state == Stopping && self.stopping >= windows(STOP_SECS * 1000.0) {
            self.state = Quiet;
            self.stopping = 0;
        }
    }

    /// Grace only when the voice starts after a real gap, so the pauses
    /// between sentences cannot chain grace windows and swallow an interruption.
    fn track_playback(&mut self, sounding: bool) {
        if sounding && !self.sounding_before && (!self.sounded || self.silent > windows(GAP_MS)) {
            self.grace = windows(GRACE_MS);
            (self.echo_peak, self.played_peak) = (0.0, 0.0);
        }
        self.sounded |= sounding;
        self.sounding_before = sounding;
        self.silent = if sounding { 0 } else { self.silent + 1 };
    }

    /// Resets the model's state when due. `self.state` is still the state before this window.
    fn maybe_reset_model(&mut self, confidence: f32) {
        self.since_reset += 1;
        if confidence < RESET_MAX_CONFIDENCE {
            self.low += 1;
        } else {
            self.low = 0;
        }
        if self.since_reset * WINDOW >= (RESET_SECS * RATE as f32) as usize
            && self.state == VadState::Quiet
            && self.low >= RESET_LOW_WINDOWS
        {
            self.scorer.reset();
            self.since_reset = 0;
        }
    }

    /// The reply is over: the next one starts quiet, and its first sound gets the grace window.
    /// The learned floor is kept.
    pub fn end_reply(&mut self) {
        self.pending.clear();
        self.volume = 0.0;
        (self.echo_peak, self.played_peak, self.played) = (0.0, 0.0, 0.0);
        self.state = VadState::Quiet;
        (self.starting, self.stopping) = (0, 0);
        (self.sounding_before, self.sounded, self.silent, self.grace) = (false, false, 0, 0);
    }
}

fn percentile90(v: &VecDeque<f32>) -> f32 {
    // Linear interpolation between the two nearest values.
    let mut s: Vec<f32> = v.iter().copied().collect();
    s.sort_by(f32::total_cmp);
    let pos = 0.9 * (s.len().saturating_sub(1)) as f32;
    let (lo, hi) = (pos.floor() as usize, pos.ceil() as usize);
    s.get(lo)
        .map_or(0.0, |a| a + (s[hi] - a) * (pos - lo as f32))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stands in for Silero: anything louder than Markon's leftover echo sounds like a voice to it.
    struct Fake {
        resets: usize,
    }
    impl Scorer for Fake {
        fn score(&mut self, w: &[f32]) -> f32 {
            if w.iter().any(|x| x.abs() > 0.005) {
                0.95
            } else {
                0.02
            }
        }
        fn reset(&mut self) {
            self.resets += 1;
        }
    }

    fn det() -> Detector<Fake> {
        Detector::new(Fake { resets: 0 })
    }

    /// `ms` of a tone at an RMS level, in 10 ms frames (the wake thread's step).
    fn tone(level: f32, ms: usize) -> Vec<Vec<f32>> {
        let n = RATE as usize * ms / 1000;
        let all: Vec<f32> = (0..n)
            .map(|i| level * 1.414 * (i as f32 * 0.3).sin())
            .collect();
        all.chunks(160).map(<[f32]>::to_vec).collect()
    }

    /// Markon's output level while it sounds, in the tests.
    const PLAYED: f32 = 0.1;

    /// Milliseconds of audio fed until the detector first reports `want`, if ever.
    fn until(
        d: &mut Detector<Fake>,
        frames: &[Vec<f32>],
        sounding: bool,
        want: VadState,
    ) -> Option<usize> {
        until_over(
            d,
            frames,
            sounding,
            if sounding { PLAYED } else { 0.0 },
            want,
        )
    }

    /// `until`, with Markon playing at `played`.
    fn until_over(
        d: &mut Detector<Fake>,
        frames: &[Vec<f32>],
        sounding: bool,
        played: f32,
        want: VadState,
    ) -> Option<usize> {
        let mut ms = 0;
        for f in frames {
            ms += f.len() * 1000 / RATE as usize;
            if d.push(f, sounding, played) == want {
                return Some(ms);
            }
        }
        None
    }

    /// A reply already playing for a while: past the grace window.
    fn mid_reply() -> Detector<Fake> {
        let mut d = det();
        assert_eq!(
            until(&mut d, &tone(0.002, 1000), true, VadState::Starting),
            None
        );
        d
    }

    #[test]
    fn talking_over_markon_is_confirmed_at_about_200_ms() {
        let mut d = mid_reply();
        let at = until(&mut d, &tone(0.03, 1000), true, VadState::Speaking).expect("confirmed");
        assert!((190..=230).contains(&at), "confirmed after {at} ms");
    }

    #[test]
    fn a_click_under_200_ms_never_confirms() {
        let mut d = mid_reply();
        let click = [tone(0.2, 150), tone(0.002, 500)].concat();
        assert_eq!(until(&mut d, &click, true, VadState::Speaking), None);
        assert_eq!(d.state(), VadState::Quiet);
    }

    #[test]
    fn one_dip_while_starting_goes_back_to_quiet() {
        let mut d = mid_reply();
        let speech = [tone(0.03, 150), tone(0.002, 40), tone(0.03, 150)].concat();
        assert_eq!(until(&mut d, &speech, true, VadState::Speaking), None);
    }

    #[test]
    fn leftover_echo_under_the_trigger_is_not_the_user() {
        // Confident but quiet: the volume gate, not the model, keeps Markon's own voice out.
        let mut d = mid_reply();
        assert_eq!(
            until(&mut d, &tone(0.0045, 3000), true, VadState::Starting),
            None
        );
    }

    #[test]
    fn no_barge_in_for_500_ms_after_markon_starts_after_a_gap() {
        let mut d = det();
        assert_eq!(
            until(&mut d, &tone(0.03, 500), true, VadState::Speaking),
            None
        );
        // Quiet echo through the grace window (16 windows, 512 ms), then the user: the 200 ms start.
        let mut d = det();
        let speech = [tone(0.002, 520), tone(0.03, 1500)].concat();
        let at = until(&mut d, &speech, true, VadState::Speaking).expect("confirmed");
        assert!((690..=760).contains(&at), "confirmed after {at} ms");
    }

    #[test]
    fn loud_echo_after_turning_the_volume_up_is_not_the_user() {
        // Measured 2026-11-20: at high Mac volume Markon's voice reached 0.1 RMS after the echo
        // canceller and cut every reply off after its first word.
        let mut d = det();
        assert_eq!(
            until(&mut d, &tone(0.1, 3000), true, VadState::Starting),
            None
        );
        // Talking clearly over it still stops Markon.
        assert!(until(&mut d, &tone(0.3, 1000), true, VadState::Speaking).is_some());
    }

    #[test]
    fn after_loud_echo_a_normal_voice_gets_in_between_markons_words() {
        let mut d = det();
        until(&mut d, &tone(0.1, 1000), true, VadState::Starting);
        // Markon between words (output silent for 300 ms): the bar is back to the room's trigger.
        let at = until_over(&mut d, &tone(0.03, 1000), true, 0.0, VadState::Speaking)
            .expect("confirmed");
        assert!(at <= 400, "confirmed after {at} ms");
    }

    #[test]
    fn the_echo_level_is_relearned_each_reply() {
        let mut d = det();
        until(&mut d, &tone(0.1, 1000), true, VadState::Speaking);
        d.end_reply();
        // Volume turned down: the next reply's quiet echo sets the bar, and normal speech gets through.
        let speech = [tone(0.002, 520), tone(0.03, 1500)].concat();
        assert!(until(&mut d, &speech, true, VadState::Speaking).is_some());
    }

    #[test]
    fn pauses_between_sentences_do_not_chain_grace_windows() {
        let mut d = mid_reply();
        // 300 ms between two sentences: no new grace, the interruption is heard at once.
        assert_eq!(
            until(&mut d, &tone(0.002, 300), false, VadState::Starting),
            None
        );
        let at = until(&mut d, &tone(0.03, 1000), true, VadState::Speaking).expect("confirmed");
        assert!(at <= 230, "confirmed after {at} ms");
        // After more than a second of silence the next sentence does get one.
        tone(0.002, 1500).iter().for_each(|f| {
            d.push(f, false, 0.0);
        });
        assert_eq!(
            until(&mut d, &tone(0.03, 500), true, VadState::Speaking),
            None
        );
    }

    #[test]
    fn the_floor_is_learned_between_replies_and_frozen_during_them() {
        let mut d = det();
        assert_eq!(d.trigger(), TRIGGER_MIN);
        tone(0.004, 4000).iter().for_each(|f| d.quiet(f));
        assert!((d.floor() - 0.004).abs() < 0.0005, "floor {}", d.floor());
        assert!(
            (d.trigger() - 0.012).abs() < 0.0015,
            "trigger {}",
            d.trigger()
        );
        // Markon speaking at a level that would lift the floor: it does not move.
        let before = d.floor();
        until(&mut d, &tone(0.01, 3000), true, VadState::Speaking);
        assert_eq!(d.floor(), before);
        // A person talking between replies does not raise it either (over the trigger).
        tone(0.05, 2000).iter().for_each(|f| d.quiet(f));
        assert_eq!(d.floor(), before);
        // A loud room never pushes the trigger past the user's voice.
        let mut loud = det();
        tone(0.0055, 4000).iter().for_each(|f| loud.quiet(f));
        assert_eq!(loud.trigger(), TRIGGER_MAX);
    }

    #[test]
    fn the_model_resets_after_2_s_only_during_clear_quiet() {
        let mut d = mid_reply();
        // 1 s of mid-reply quiet (31 windows) has not reset yet; at 2 s it has.
        assert_eq!(d.scorer.resets, 0);
        until(&mut d, &tone(0.002, 1100), true, VadState::Speaking);
        assert_eq!(d.scorer.resets, 1);
        // Never while the user talks.
        tone(0.03, 3000).iter().for_each(|f| {
            d.push(f, true, PLAYED);
        });
        assert_eq!(d.state(), VadState::Speaking);
        assert_eq!(d.scorer.resets, 1);
    }

    #[test]
    fn stopping_returns_to_quiet_after_200_ms() {
        let mut d = mid_reply();
        until(&mut d, &tone(0.03, 300), true, VadState::Speaking).expect("speaking");
        let at = until(&mut d, &tone(0.002, 500), true, VadState::Quiet).expect("quiet");
        assert!((190..=260).contains(&at), "quiet after {at} ms");
    }

    #[test]
    fn percentile_matches_numpy() {
        let v: VecDeque<f32> = (1..=10).map(|x| x as f32).collect();
        assert!((percentile90(&v) - 9.1).abs() < 1e-5);
    }

    /// The real model, when a verified copy is at `MARKON_SILERO` (not in CI: models are never committed).
    #[test]
    fn silero_scores_a_voice_like_signal_above_silence() {
        let Ok(path) = std::env::var("MARKON_SILERO") else {
            return;
        };
        let mut s = Silero::load(Path::new(&path)).expect("load");
        let silence = vec![0.0f32; WINDOW];
        let quiet: Vec<f32> = (0..20).map(|_| s.score(&silence)).collect();
        assert!(quiet.iter().all(|p| *p < 0.1), "{quiet:?}");
        // Speech (raw f32 at 16 kHz, `say -o x.wav --data-format=LEF32@16000`) scaled to the user's
        // level after echo removal, over Markon's leftover echo: confirmed after its first 200 ms.
        let Ok(clip) = std::fs::read(std::env::var("MARKON_SPEECH").unwrap_or_default()) else {
            return;
        };
        let speech: Vec<f32> = clip
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect();
        let start = speech.iter().position(|x| x.abs() > 0.01).unwrap_or(0);
        let speech = &speech[start..];
        let gain = 0.03 / rms(speech);
        let mut d = Detector::new(s);
        let echo: Vec<f32> = (0..RATE as usize)
            .map(|i| 0.002 * (i as f32 * 0.3).sin())
            .collect();
        assert_eq!(d.push(&echo, true, 0.1), VadState::Quiet);
        let mut ms = 0;
        for c in speech.chunks(160) {
            let c: Vec<f32> = c.iter().map(|x| x * gain).collect();
            ms += 10;
            if d.push(&c, true, 0.1) == VadState::Speaking {
                break;
            }
        }
        eprintln!("silero confirmed speech at 0.03 RMS after {ms} ms");
        assert!(ms <= 400, "{ms} ms");
    }
}
