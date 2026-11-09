//! Text to speech with Kokoro-82M (ONNX). The voice file is pinned; there is no fallback voice.
use crate::AudioError;
use crate::g2p::{G2p, tokenize};
use ort::session::Session;
use ort::value::Tensor;
use std::path::Path;

pub const KOKORO_RATE: u32 = 24_000;
/// Kokoro accepts at most 510 tokens per call (plus the two pads).
const MAX_TOKENS: usize = 500;

pub trait TtsEngine: Send {
    /// Mono samples at `sample_rate()`.
    fn synth(&mut self, text: &str) -> Result<Vec<f32>, AudioError>;
    fn sample_rate(&self) -> u32;
    /// Switch to another pinned voice file (already verified against the manifest).
    fn set_voice(&mut self, _voice: &Path) -> Result<(), AudioError> {
        Err(AudioError::Model("this engine has one voice".into()))
    }
}

/// The two bundled voices. The product name follows the voice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Voice {
    Markon,
    Markan,
}

impl Voice {
    pub fn manifest_id(self) -> &'static str {
        match self {
            Voice::Markon => "voice-markon",
            Voice::Markan => "voice-markan",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Voice::Markon => "Markon",
            Voice::Markan => "Markan",
        }
    }
}

pub struct Kokoro {
    session: Session,
    /// Style vectors: 510 rows of 256 floats, row = token count.
    styles: Vec<f32>,
    g2p: G2p,
}

impl Kokoro {
    /// All paths must already be verified against the manifest.
    pub fn load(model: &Path, voice: &Path, g2p: G2p) -> Result<Self, AudioError> {
        let styles = read_styles(voice)?;
        // onnxruntime allows one environment per process.
        static ENV: std::sync::OnceLock<Result<ort::environment::Environment, String>> =
            std::sync::OnceLock::new();
        let env = ENV
            .get_or_init(|| {
                ort::init()
                    .with_telemetry(false)
                    .build()
                    .map_err(|e| e.to_string())
            })
            .as_ref()
            .map_err(|e| AudioError::Model(e.clone()))?;
        let fail = |e: String| AudioError::Model(format!("kokoro: {e}"));
        let session = Session::builder(env)
            .map_err(|e| fail(e.to_string()))?
            .with_intra_threads(4)
            .map_err(|e| fail(e.to_string()))?
            .commit_from_file(model)
            .map_err(|e| fail(e.to_string()))?;
        let mut k = Self {
            session,
            styles,
            g2p,
        };
        // The first inference is slow (graph set-up); pay for it at load, not on the first reply.
        k.synth("Ready.")?;
        Ok(k)
    }

    fn run(&mut self, ids: Vec<i64>) -> Result<Vec<f32>, AudioError> {
        let n = ids.len();
        let rows = self.styles.len() / 256;
        let row = (n - 2).min(rows - 1);
        let style = self.styles[row * 256..row * 256 + 256].to_vec();
        let err = |e: ort::Error| AudioError::Model(e.to_string());
        let out = self
            .session
            .run(ort::inputs![
                "input_ids" => Tensor::from_array(([1usize, n], ids)).map_err(err)?,
                "style" => Tensor::from_array(([1usize, 256], style)).map_err(err)?,
                "speed" => Tensor::from_array(([1usize], vec![1.0f32])).map_err(err)?,
            ])
            .map_err(err)?;
        let (_, wav) = out["waveform"].try_extract_tensor::<f32>().map_err(err)?;
        Ok(wav.to_vec())
    }
}

/// Split a phoneme string into pieces of at most `max` symbols, preferring punctuation, then spaces.
pub fn split_phonemes(ph: &str, max: usize) -> Vec<String> {
    let chars: Vec<char> = ph.chars().collect();
    let mut out = Vec::new();
    let mut start = 0;
    while chars.len() - start > max {
        let window = &chars[start..start + max];
        let cut = window
            .iter()
            .rposition(|c| ".,;:!?—".contains(*c))
            .or_else(|| window.iter().rposition(|c| *c == ' '))
            .map(|i| i + 1)
            .unwrap_or(max);
        out.push(
            chars[start..start + cut]
                .iter()
                .collect::<String>()
                .trim()
                .to_string(),
        );
        start += cut;
    }
    out.push(chars[start..].iter().collect::<String>().trim().to_string());
    out.retain(|s| !s.is_empty());
    out
}

impl TtsEngine for Kokoro {
    fn synth(&mut self, text: &str) -> Result<Vec<f32>, AudioError> {
        let ph = self.g2p.phonemize(text);
        let mut audio = Vec::new();
        for piece in split_phonemes(&ph, MAX_TOKENS) {
            audio.extend(self.run(tokenize(&piece))?);
        }
        Ok(audio)
    }
    fn sample_rate(&self) -> u32 {
        KOKORO_RATE
    }
    fn set_voice(&mut self, voice: &Path) -> Result<(), AudioError> {
        self.styles = read_styles(voice)?;
        Ok(())
    }
}

fn read_styles(voice: &Path) -> Result<Vec<f32>, AudioError> {
    let bytes = std::fs::read(voice)?;
    let styles: Vec<f32> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect();
    if styles.len() < 256 || !styles.len().is_multiple_of(256) {
        return Err(AudioError::Model(
            "voice file has an unexpected shape".into(),
        ));
    }
    Ok(styles)
}

/// speak the opening words of a reply on their own so first audio starts sooner.
pub fn split_first_chunk(sentence: &str) -> (String, Option<String>) {
    let words: Vec<&str> = sentence.split_whitespace().collect();
    if words.len() <= 5 {
        return (sentence.to_string(), None);
    }
    let comma = words.iter().take(6).position(|w| w.ends_with(','));
    let n = comma.map(|i| i + 1).unwrap_or(4);
    (words[..n].join(" "), Some(words[n..].join(" ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_phoneme_strings_split_at_punctuation() {
        let s = format!("{}, {}", "a".repeat(300), "b".repeat(300));
        let parts = split_phonemes(&s, 500);
        assert_eq!(parts.len(), 2);
        assert!(parts.iter().all(|p| p.chars().count() <= 500));
        assert!(parts[0].ends_with(','));
    }

    #[test]
    fn the_first_chunk_is_a_few_words() {
        assert_eq!(
            split_first_chunk("Good morning, Dev. You have three meetings."),
            (
                "Good morning,".into(),
                Some("Dev. You have three meetings.".into())
            )
        );
        assert_eq!(
            split_first_chunk("You have three meetings today, want me to move one?").0,
            "You have three meetings today,"
        );
        assert_eq!(
            split_first_chunk("Here is the plan for the whole week ahead.").0,
            "Here is the plan"
        );
        assert_eq!(split_first_chunk("Done."), ("Done.".into(), None));
    }
}
