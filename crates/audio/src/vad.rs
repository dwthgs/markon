//! Energy-based voice activity detection with its end-of-speech timing.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndOfSpeech {
    Started,
    Ended,
}

pub struct Vad {
    sr: u32,
    speaking: bool,
    silent_samples: usize,
    noise: f32,
}

/// Silence that ends a turn, and the longer wait when the partial transcript ends mid-clause.
/// a 700 ms / 1.2 s pause cut people off during normal thinking pauses.
pub const END_MS: usize = 1200;
pub const END_MID_CLAUSE_MS: usize = 2000;
const MIN_LEVEL: f32 = 0.01;

impl Vad {
    pub fn new(sr: u32) -> Self {
        Self {
            sr,
            speaking: false,
            silent_samples: 0,
            noise: 0.0,
        }
    }

    pub fn is_speaking(&self) -> bool {
        self.speaking
    }

    /// Feed a short chunk (about 10 ms). `mid_clause` is true when the partial transcript so far does
    /// not end a sentence, which extends the wait.
    pub fn push(&mut self, chunk: &[f32], mid_clause: bool) -> Option<EndOfSpeech> {
        if chunk.is_empty() {
            return None;
        }
        let rms = (chunk.iter().map(|x| x * x).sum::<f32>() / chunk.len() as f32).sqrt();
        let voiced = rms > MIN_LEVEL.max(self.noise * 3.0);
        if !voiced {
            self.noise += (rms - self.noise) * 0.05;
        }
        if voiced {
            self.silent_samples = 0;
            if !self.speaking {
                self.speaking = true;
                return Some(EndOfSpeech::Started);
            }
            return None;
        }
        if !self.speaking {
            return None;
        }
        self.silent_samples += chunk.len();
        let wait = if mid_clause {
            END_MID_CLAUSE_MS
        } else {
            END_MS
        };
        if self.silent_samples * 1000 >= wait * self.sr as usize {
            self.speaking = false;
            self.silent_samples = 0;
            return Some(EndOfSpeech::Ended);
        }
        None
    }
}

/// True when the transcript so far ends a clause or sentence.
pub fn ends_clause(text: &str) -> bool {
    text.trim_end().ends_with(['.', '!', '?'])
}
