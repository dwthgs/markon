//! Audio pipeline: capture, end-of-speech, whisper, Kokoro, playback and the features
//! that drive the voice core.
pub mod features;
pub mod g2p;
pub mod manifest;
pub mod resample;
pub mod sentences;
pub mod service;
pub mod stt;
pub mod tts;
pub mod vad;
pub mod wake;

pub use service::{Audio, AudioEvent, Who};
pub use tts::Voice;

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("no microphone found")]
    NoMicrophone,
    #[error("microphone access is denied")]
    MicDenied,
    #[error("no audio output device")]
    NoOutput,
    #[error("audio device: {0}")]
    Device(String),
    #[error("model: {0}")]
    Model(String),
    #[error(transparent)]
    Manifest(#[from] manifest::ManifestError),
    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),
    #[error("speech models are not loaded yet")]
    NotReady,
}
