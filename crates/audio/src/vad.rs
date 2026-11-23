//! Energy-based voice activity detection with its end-of-speech timing.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndOfSpeech {
    Started,
    Ended,
}

pub struct Vad {
    sr: u32,
    end_ms: usize,
    end_mid_ms: usize,
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
            end_ms: END_MS,
            end_mid_ms: END_MID_CLAUSE_MS,
            ..Self::with_end(sr, END_MS)
        }
    }

    /// A fixed end-of-speech pause (the wake listener wants short bursts).
    pub fn with_end(sr: u32, end_ms: usize) -> Self {
        Self {
            sr,
            end_ms,
            end_mid_ms: end_ms,
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
            self.end_mid_ms
        } else {
            self.end_ms
        };
        if self.silent_samples * 1000 >= wait * self.sr as usize {
            self.speaking = false;
            self.silent_samples = 0;
            return Some(EndOfSpeech::Ended);
        }
        None
    }
}

/// Last words of someone still thinking: whisper puts a full stop after them all the same.
const HESITATIONS: &[&str] = &[
    "um", "umm", "uh", "uhh", "er", "erm", "hmm", "so", "and", "but", "or", "like", "the", "a",
    "to", "because", "well",
];

/// True when the transcript so far ends a clause or sentence, and not on a hesitation
/// ("I want to, um." or "So...").
pub fn ends_clause(text: &str) -> bool {
    let t = text.trim_end();
    let last = t
        .rsplit(|c: char| !c.is_alphanumeric())
        .find(|w| !w.is_empty())
        .unwrap_or("")
        .to_lowercase();
    t.ends_with(['.', '!', '?']) && !t.ends_with("...") && !HESITATIONS.contains(&last.as_str())
}

#[cfg(test)]
mod tests {
    use super::ends_clause;

    #[test]
    fn a_hesitation_is_not_the_end_of_a_sentence() {
        assert!(ends_clause("What's the weather tomorrow?"));
        assert!(ends_clause("Play some jazz."));
        assert!(!ends_clause("I want to, um."));
        assert!(!ends_clause("So..."));
        assert!(!ends_clause("Remind me to call Sara and"));
        assert!(!ends_clause("Open the."));
    }
}
