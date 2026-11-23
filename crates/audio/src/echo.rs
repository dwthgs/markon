//! Talking over Markon: WebRTC's echo canceller removes Markon's own voice from the
//! microphone, and `barge` decides whether what is left is the user speaking.
use webrtc_audio_processing::Processor;
use webrtc_audio_processing::config::{Config, EchoCanceller, HighPassFilter};

/// Echo cancellation on 10 ms mono frames at one sample rate.
pub struct Echo {
    /// None: the microphone arrives already clean (Apple's voice processing).
    ap: Option<Processor>,
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
            ap: Some(ap),
            frame,
            render: Vec::new(),
        })
    }

    /// For a microphone the system has already cleaned: `played` and `clean` do nothing.
    pub fn done_by_system(rate: u32) -> Self {
        Self {
            ap: None,
            frame: rate as usize / 100,
            render: Vec::new(),
        }
    }

    pub fn frame_len(&self) -> usize {
        self.frame
    }

    /// What the speakers just played (mono, same rate).
    pub fn played(&mut self, pcm: &[f32]) {
        let Some(ap) = &self.ap else { return };
        self.render.extend_from_slice(pcm);
        let whole = self.render.len() / self.frame * self.frame;
        for f in self.render[..whole].chunks_exact(self.frame) {
            let _ = ap.analyze_render_frame([f]);
        }
        self.render.drain(..whole);
    }

    /// One microphone frame of `frame_len()` samples, cleaned in place.
    pub fn clean(&mut self, frame: &mut [f32]) {
        if let Some(ap) = &self.ap {
            let _ = ap.process_capture_frame([frame]);
        }
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

    /// Echo left in each 250 ms of a reply: a 30 ms, half-level echo of what was played.
    fn reply(e: &mut Echo, rate: usize, seed: u32, secs: usize) -> Vec<f32> {
        let f = e.frame_len();
        let played = noise(rate * secs, seed);
        let delay = rate * 30 / 1000;
        let (mut raw, mut left, mut out) = (0.0f32, 0.0f32, Vec::new());
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
            raw += rms(&mic);
            e.clean(&mut mic);
            left += rms(&mic);
            if (i + 1) % 25 == 0 {
                out.push(left / raw.max(1e-9));
                (raw, left) = (0.0, 0.0);
            }
        }
        out
    }

    /// Ten seconds of silence between two replies, with or without the microphone fed meanwhile.
    fn second_reply(mic_fed: bool) -> Vec<f32> {
        let rate = 16_000usize;
        let mut e = Echo::new(rate as u32).unwrap();
        let f = e.frame_len();
        reply(&mut e, rate, 1, 6);
        for _ in 0..1000 {
            e.played(&vec![0.0; f]);
            if mic_fed {
                e.clean(&mut vec![0.0; f]);
            }
        }
        reply(&mut e, rate, 2, 3)
    }

    #[test]
    fn the_next_reply_starts_clean_when_the_microphone_was_fed_between_replies() {
        let left = second_reply(true);
        assert!(
            left.iter().all(|x| *x < 0.05),
            "echo left per 250 ms: {left:?}"
        );
        // Why wake.rs feeds it: starved of the microphone, the canceller starts the reply deaf.
        assert!(second_reply(false)[0] > 0.5);
    }
}
