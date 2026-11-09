//! Speech to text. whisper.cpp does not stream: partial text comes from re-running on the growing clip.
use crate::AudioError;
use std::path::Path;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

pub trait SttEngine: Send {
    /// Transcribe 16 kHz mono audio.
    fn transcribe(&mut self, pcm16k: &[f32]) -> Result<String, AudioError>;
}

pub struct Whisper {
    ctx: WhisperContext,
}

/// Biases whisper toward the product names it otherwise hears as "Markham".
const PROMPT: &str = "Markon, Markan.";

impl Whisper {
    pub fn load(path: &Path) -> Result<Self, AudioError> {
        let ctx = WhisperContext::new_with_params(path, WhisperContextParameters::default())
            .map_err(|e| AudioError::Model(format!("whisper: {e}")))?;
        Ok(Self { ctx })
    }
}

impl SttEngine for Whisper {
    fn transcribe(&mut self, pcm16k: &[f32]) -> Result<String, AudioError> {
        // whisper needs at least one second; pad short clips with silence.
        let mut pcm = pcm16k.to_vec();
        if pcm.len() < 16_000 {
            pcm.resize(16_100, 0.0);
        }
        let mut state = self
            .ctx
            .create_state()
            .map_err(|e| AudioError::Model(e.to_string()))?;
        let mut p = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        p.set_n_threads(4); // faster than 8 on Apple Silicon,
        p.set_language(Some("en"));
        p.set_initial_prompt(PROMPT);
        p.set_print_progress(false);
        p.set_print_realtime(false);
        p.set_print_special(false);
        p.set_print_timestamps(false);
        p.set_no_context(true);
        p.set_single_segment(true);
        p.set_suppress_blank(true);
        state
            .full(p, &pcm)
            .map_err(|e| AudioError::Model(e.to_string()))?;
        let mut text = String::new();
        for seg in state.as_iter() {
            text.push_str(&seg.to_string());
        }
        Ok(clean(&text))
    }
}

/// Drop whisper's non-speech markers like "[BLANK_AUDIO]" or "(wind blowing)".
pub fn clean(text: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    for c in text.chars() {
        match c {
            '[' | '(' => depth += 1,
            ']' | ')' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    #[test]
    fn strips_non_speech_markers() {
        assert_eq!(super::clean(" [BLANK_AUDIO] "), "");
        assert_eq!(super::clean(" Hello (laughs) there."), "Hello there.");
    }
}
