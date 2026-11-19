//! Whole-buffer sample-rate conversion (mic → 16 kHz for whisper, 24 kHz Kokoro → output device).
use audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};

pub fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    // A NaN or infinity (Kokoro can emit them) makes the FFT panic, which killed the speaker thread.
    let clean: Vec<f32>;
    let input = if input.iter().all(|x| x.is_finite()) {
        input
    } else {
        clean = input
            .iter()
            .map(|x| if x.is_finite() { *x } else { 0.0 })
            .collect();
        &clean
    };
    if from == to || input.is_empty() {
        return input.to_vec();
    }
    let Ok(mut r) = Fft::<f32>::new(from as usize, to as usize, 1024, 1, FixedSync::Input) else {
        return Vec::new();
    };
    let needed = r.process_all_needed_output_len(input.len());
    let mut out = vec![0.0f32; needed];
    let (Ok(src), Ok(mut dst)) = (
        InterleavedSlice::new(input, 1, input.len()),
        InterleavedSlice::new_mut(&mut out, 1, needed),
    ) else {
        return Vec::new();
    };
    match r.process_all_into_buffer(&src, &mut dst, input.len(), None) {
        Ok((_, produced)) => {
            out.truncate(produced);
            out
        }
        Err(_) => Vec::new(),
    }
}

/// Chunk-by-chunk conversion that keeps its filter state between calls, for the wake thread's 10 ms
/// frames (resampling each frame alone would put a click at every frame edge).
pub struct Streaming {
    r: Option<Fft<f32>>,
    chunk: usize,
    out: Vec<f32>,
}

impl Streaming {
    /// `chunk` is the exact number of samples every `push` gets.
    pub fn new(from: u32, to: u32, chunk: usize) -> Self {
        let r = (from != to)
            .then(|| Fft::<f32>::new(from as usize, to as usize, chunk, 1, FixedSync::Input).ok())
            .flatten();
        let out = vec![0.0; r.as_ref().map_or(0, |r| r.output_frames_max())];
        Self { r, chunk, out }
    }

    pub fn push(&mut self, input: &[f32]) -> Vec<f32> {
        let Some(r) = self.r.as_mut() else {
            return input.to_vec();
        };
        if input.len() != self.chunk || !input.iter().all(|x| x.is_finite()) {
            return Vec::new();
        }
        let n = self.out.len();
        let (Ok(src), Ok(mut dst)) = (
            InterleavedSlice::new(input, 1, input.len()),
            InterleavedSlice::new_mut(&mut self.out, 1, n),
        ) else {
            return Vec::new();
        };
        match r.process_into_buffer(&src, &mut dst, None) {
            Ok((_, produced)) => self.out[..produced].to_vec(),
            Err(_) => Vec::new(),
        }
    }
}

/// Average interleaved channels to mono.
pub fn downmix(data: &[f32], channels: usize) -> impl Iterator<Item = f32> + '_ {
    data.chunks(channels.max(1))
        .map(|f| f.iter().sum::<f32>() / f.len() as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn survives_nan_and_infinity() {
        let mut v = vec![0.1f32; 24_000];
        v[100] = f32::NAN;
        v[200] = f32::INFINITY;
        let out = resample(&v, 24_000, 48_000);
        assert!(out.len() > 40_000);
        assert!(out.iter().all(|x| x.is_finite()));
        assert!(resample(&v, 16_000, 16_000).iter().all(|x| x.is_finite()));
    }

    #[test]
    fn keeps_duration_and_pitch() {
        let sine: Vec<f32> = (0..48_000)
            .map(|i| (std::f32::consts::TAU * 440.0 * i as f32 / 48_000.0).sin())
            .collect();
        let out = resample(&sine, 48_000, 16_000);
        assert!(
            (out.len() as i64 - 16_000).abs() < 50,
            "{} samples",
            out.len()
        );
        // Count zero crossings over the middle half: about 2 × 440 × 0.5 s.
        let mid = &out[4000..12000];
        let zc = mid
            .windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count();
        assert!((430..=450).contains(&zc), "{zc} crossings");
    }

    #[test]
    fn streams_10_ms_frames_without_losing_time() {
        for from in [48_000u32, 44_100] {
            let chunk = from as usize / 100;
            let mut s = Streaming::new(from, 16_000, chunk);
            let sine: Vec<f32> = (0..from as usize)
                .map(|i| (std::f32::consts::TAU * 440.0 * i as f32 / from as f32).sin())
                .collect();
            let out: Vec<f32> = sine.chunks_exact(chunk).flat_map(|c| s.push(c)).collect();
            assert!(
                (out.len() as i64 - 16_000).abs() < 20,
                "{from}: {}",
                out.len()
            );
            let mid = &out[4000..12000];
            let zc = mid
                .windows(2)
                .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
                .count();
            assert!((430..=450).contains(&zc), "{from}: {zc} crossings");
        }
        assert_eq!(
            Streaming::new(16_000, 16_000, 160).push(&[0.5; 160]),
            vec![0.5; 160]
        );
    }

    #[test]
    fn downmixes_stereo() {
        assert_eq!(
            downmix(&[1.0, 0.0, 0.5, 0.5], 2).collect::<Vec<_>>(),
            vec![0.5, 0.5]
        );
    }
}
