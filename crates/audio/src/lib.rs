//! Audio pipeline: capture, end-of-speech, whisper, Kokoro, playback and the features
//! that drive the voice core.
pub mod echo;
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

/// The one ONNX Runtime environment a process may have, shared by Kokoro and the embedding model:
/// a second environment would fail to build.
pub fn onnx_env() -> Result<&'static ort::environment::Environment, String> {
    static ENV: std::sync::OnceLock<Result<ort::environment::Environment, String>> =
        std::sync::OnceLock::new();
    ENV.get_or_init(|| {
        ort::init()
            .with_telemetry(false)
            .build()
            .map_err(|e| e.to_string())
    })
    .as_ref()
    .map_err(|e| e.clone())
}
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
