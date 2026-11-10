//! Talking over Markon: WebRTC's echo canceller removes Markon's own voice from the
//! microphone, and what is left is checked for sustained speech.
use webrtc_audio_processing::Processor;
use webrtc_audio_processing::config::{Config, EchoCanceller, HighPassFilter};

/// Echo cancellation on 10 ms mono frames at one sample rate.
pub struct Echo {
    ap: Processor,
    frame: usize,
    render: Vec<f32>,
}

impl Echo {
    pub fn new(rate: u32) -> Option<Self> {
        let ap = Processor::new(rate).ok()?;
        ap.set_config(Config {
            echo_canceller: Some(EchoCanceller::default()),
            high_pass_filter: Some(HighPassFilter::default()),
            ..Default::default()
        });
        let frame = ap.num_samples_per_frame();
        Some(Self {
            ap,
            frame,
            render: Vec::new(),
        })
    }

    pub fn frame_len(&self) -> usize {
        self.frame
    }

    /// What the speakers just played (mono, same rate).
    pub fn played(&mut self, pcm: &[f32]) {
        self.render.extend_from_slice(pcm);
        let whole = self.render.len() / self.frame * self.frame;
        for f in self.render[..whole].chunks_exact(self.frame) {
            let _ = self.ap.analyze_render_frame([f]);
        }
        self.render.drain(..whole);
    }

    /// One microphone frame of `frame_len()` samples, cleaned in place.
    pub fn clean(&mut self, frame: &mut [f32]) {
        let _ = self.ap.process_capture_frame([frame]);
    }
}

/// Speech in the cleaned signal, in two stages: a hint (duck Markon's voice at once) and a confirmed
/// barge-in (stop). Judged over the last 350 ms.
#[derive(Default)]
pub struct BargeIn {
    window: std::collections::VecDeque<bool>,
    noise: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Barge {
    Quiet,
    /// A few voiced frames: maybe the user, maybe a noise.
    Hint,
    /// Sustained speech: the user is talking over Markon.
    Confirmed,
}

const WINDOW_FRAMES: usize = 35;
const HINT_FRAMES: usize = 5;
// A cough, a click or a burst of echo is shorter than this; a person starting to talk is not.
const CONFIRM_FRAMES: usize = 25;
const MIN_LEVEL: f32 = 0.02;

impl BargeIn {
    /// Feed one cleaned 10 ms frame.
    pub fn push(&mut self, frame: &[f32]) -> Barge {
        let rms = (frame.iter().map(|x| x * x).sum::<f32>() / frame.len().max(1) as f32).sqrt();
        let voiced = rms > MIN_LEVEL.max(self.noise * 4.0);
        if !voiced {
            self.noise += (rms - self.noise) * 0.05;
        }
        if self.window.len() == WINDOW_FRAMES {
            self.window.pop_front();
        }
        self.window.push_back(voiced);
        match self.window.iter().filter(|v| **v).count() {
            n if n >= CONFIRM_FRAMES => Barge::Confirmed,
            n if n >= HINT_FRAMES => Barge::Hint,
            _ => Barge::Quiet,
        }
    }

    pub fn reset(&mut self) {
        self.window.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(n: usize, seed: u32) -> Vec<f32> {
        let mut s = seed;
        (0..n)
            .map(|_| {
                s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (s >> 8) as f32 / (1u32 << 24) as f32 - 0.5
            })
            .collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn removes_markons_own_voice_from_the_microphone() {
        let rate = 16_000;
        let mut e = Echo::new(rate).unwrap();
        let f = e.frame_len();
        let played = noise(rate as usize * 6, 1);
        // The microphone hears the speaker 30 ms later, at half the level.
        let delay = rate as usize * 30 / 1000;
        let mut before = Vec::new();
        let mut after = Vec::new();
        for (i, chunk) in played.chunks_exact(f).enumerate() {
            e.played(chunk);
            let start = i * f;
            let mut mic: Vec<f32> = (start..start + f)
                .map(|k| {
                    if k >= delay {
                        0.5 * played[k - delay]
                    } else {
                        0.0
                    }
                })
                .collect();
            let raw = rms(&mic);
            e.clean(&mut mic);
            // Judge the last two seconds, after the canceller has converged.
            if start > rate as usize * 4 {
                before.push(raw);
                after.push(rms(&mic));
            }
        }
        let gain = after.iter().sum::<f32>() / before.iter().sum::<f32>();
        assert!(gain < 0.1, "echo left at {:.1}% of its level", gain * 100.0);
    }

    #[test]
    fn hints_within_50_ms_and_confirms_within_250_ms() {
        let mut b = BargeIn::default();
        let quiet = vec![0.005f32; 160];
        let loud: Vec<f32> = (0..160).map(|i| 0.2 * (i as f32 * 0.3).sin()).collect();
        assert!(
            (0..100).all(|_| b.push(&quiet) == Barge::Quiet),
            "residual echo"
        );
        let states: Vec<Barge> = (0..30).map(|_| b.push(&loud)).collect();
        assert_eq!(
            states.iter().position(|s| *s == Barge::Hint),
            Some(HINT_FRAMES - 1)
        );
        assert_eq!(
            states.iter().position(|s| *s == Barge::Confirmed),
            Some(CONFIRM_FRAMES - 1)
        );
    }

    #[test]
    fn a_click_only_hints_and_then_fades() {
        let mut b = BargeIn::default();
        let quiet = vec![0.005f32; 160];
        let loud: Vec<f32> = (0..160).map(|i| 0.2 * (i as f32 * 0.3).sin()).collect();
        (0..50).for_each(|_| {
            b.push(&quiet);
        });
        let click: Vec<Barge> = (0..20).map(|_| b.push(&loud)).collect();
        assert!(!click.contains(&Barge::Confirmed));
        assert!(click.contains(&Barge::Hint));
        let after: Vec<Barge> = (0..40).map(|_| b.push(&quiet)).collect();
        assert_eq!(after.last(), Some(&Barge::Quiet));
    }
}
