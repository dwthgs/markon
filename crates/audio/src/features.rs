//! Audio features for the voice core: loudness, pitch and 48 log-spaced bands, 30 times a second.
//! The scaling matches a Web Audio analyser, so the core looks the same in the app and on the web.
use realfft::{RealFftPlanner, RealToComplex};
use serde::Serialize;
use std::sync::Arc;

pub const NB: usize = 48;
pub const FEATURE_HZ: u32 = 30;
const FLO: f32 = 90.0;
const FHI: f32 = 7000.0;

/// Centre frequency of band `k` (90 Hz to 7 kHz, log spaced).
pub fn band_hz(k: usize) -> f32 {
    FLO * (FHI / FLO).powf(k as f32 / (NB - 1) as f32)
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Features {
    /// Loudness normalised to 0..1.
    pub rms: f32,
    /// Fundamental frequency in Hz, 0 when unvoiced or quiet.
    pub pitch: f32,
    /// 48 band levels, 0..1.
    pub bands: Vec<f32>,
}

impl Features {
    pub fn silent() -> Self {
        Self {
            rms: 0.0,
            pitch: 0.0,
            bands: vec![0.0; NB],
        }
    }
}

pub struct FeatureExtractor {
    sr: u32,
    hop: usize,
    since: usize,
    ring: Vec<f32>,
    pos: usize,
    window: Vec<f32>,
    frame: Vec<f32>,
    fft: Arc<dyn RealToComplex<f32>>,
    spec: Vec<realfft::num_complex::Complex<f32>>,
    scratch: Vec<realfft::num_complex::Complex<f32>>,
}

impl FeatureExtractor {
    pub fn new(sr: u32) -> Self {
        // About 64 ms of audio: long enough for two periods of a 70 Hz voice.
        let n = ((sr as f32 * 0.064) as usize).next_power_of_two();
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(n);
        let window = (0..n)
            .map(|i| {
                // Blackman, as Web Audio's AnalyserNode uses.
                let x = std::f32::consts::TAU * i as f32 / n as f32;
                0.42 - 0.5 * x.cos() + 0.08 * (2.0 * x).cos()
            })
            .collect();
        Self {
            sr,
            hop: (sr / FEATURE_HZ) as usize,
            since: 0,
            ring: vec![0.0; n],
            pos: 0,
            window,
            frame: vec![0.0; n],
            spec: fft.make_output_vec(),
            scratch: fft.make_scratch_vec(),
            fft,
        }
    }

    /// Worst-case delay between a sound arriving and its first appearance in a published frame.
    pub fn latency_ms(&self) -> f32 {
        self.hop as f32 * 1000.0 / self.sr as f32
    }

    /// Feed mono samples. Returns a frame each time 1/30 s of new audio has arrived.
    pub fn push(&mut self, samples: &[f32]) -> Option<Features> {
        let n = self.ring.len();
        let mut out = None;
        for &s in samples {
            self.ring[self.pos] = s;
            self.pos = (self.pos + 1) % n;
            self.since += 1;
            if self.since >= self.hop {
                self.since = 0;
                out = Some(self.analyse());
            }
        }
        out
    }

    fn analyse(&mut self) -> Features {
        let n = self.ring.len();
        for i in 0..n {
            self.frame[i] = self.ring[(self.pos + i) % n];
        }
        let rms = (self.frame.iter().map(|x| x * x).sum::<f32>() / n as f32).sqrt();
        let e = ((rms - 0.008) * 7.0).clamp(0.0, 1.0);
        if e == 0.0 {
            return Features::silent();
        }
        let pitch = if e > 0.05 { self.pitch() } else { 0.0 };
        let mut buf: Vec<f32> = self
            .frame
            .iter()
            .zip(&self.window)
            .map(|(x, w)| x * w)
            .collect();
        if self
            .fft
            .process_with_scratch(&mut buf, &mut self.spec, &mut self.scratch)
            .is_err()
        {
            return Features::silent();
        }
        let bin = self.sr as f32 / n as f32;
        // Window gain for Blackman is 0.42, so a full-scale sine reads about 0 dB.
        let norm = 2.0 / (n as f32 * 0.42);
        let mut bands = vec![0.0; NB];
        for (k, b) in bands.iter_mut().enumerate() {
            let f = band_hz(k);
            let lo = ((f * 0.94 / bin).floor() as usize).max(1);
            let hi = ((f * 1.06 / bin).ceil() as usize).max(lo + 1);
            let m = self.spec[lo..hi.min(self.spec.len())]
                .iter()
                .map(|c| c.norm())
                .fold(0.0, f32::max)
                * norm;
            // Same mapping as a Web Audio analyser: bytes span -100..-30 dB.
            let db = 20.0 * m.max(1e-9).log10();
            *b = (((db + 100.0) / 70.0 - 0.25) * 1.6).clamp(0.0, 1.0);
        }
        Features {
            rms: e,
            pitch,
            bands,
        }
    }

    /// Autocorrelation pitch between 70 and 350 Hz.
    fn pitch(&self) -> f32 {
        let x = &self.frame;
        let (lo, hi) = ((self.sr / 350) as usize, (self.sr / 70) as usize);
        let n = x.len() - hi;
        let ac = |lag: usize| (0..n).map(|i| x[i] * x[i + lag]).sum::<f32>();
        let vals: Vec<f32> = (lo..=hi).map(ac).collect();
        let max = vals.iter().cloned().fold(0.0, f32::max);
        if max <= 0.0 {
            return 0.0;
        }
        // First peak within 90% of the best, so octave multiples do not win.
        let Some(i) = (1..vals.len() - 1)
            .find(|&i| vals[i] >= 0.9 * max && vals[i] >= vals[i - 1] && vals[i] >= vals[i + 1])
        else {
            return 0.0;
        };
        let (a, b, c) = (vals[i - 1], vals[i], vals[i + 1]);
        let shift = 0.5 * (a - c) / (a - 2.0 * b + c);
        let lag = (lo + i) as f32 + if shift.is_finite() { shift } else { 0.0 };
        self.sr as f32 / lag
    }
}
