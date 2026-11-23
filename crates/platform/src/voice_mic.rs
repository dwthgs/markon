//! The microphone through Apple's Voice Processing I/O unit, the echo canceller FaceTime uses.
//! It removes whatever the Mac is playing from the microphone signal, including the
//! built-in speakers' distortion at high volume, which WebRTC's AEC3 cannot.
use coreaudio::audio_unit::audio_format::LinearPcmFlags;
use coreaudio::audio_unit::render_callback::{Args, data::Interleaved};
use coreaudio::audio_unit::{AudioUnit, Element, IOType, SampleFormat, Scope, StreamFormat};

/// `kAUVoiceIOProperty_OtherAudioDuckingConfiguration` (macOS 14) and its value.
const DUCKING: u32 = 2108;
#[repr(C)]
struct Ducking {
    advanced: u8,
    level: u32,
}
/// `kAUVoiceIOOtherAudioDuckingLevelMin`
const DUCK_LEAST: u32 = 10;

/// Open until dropped.
pub struct VoiceMic {
    _unit: AudioUnit,
}

/// Start the default microphone, echo-cancelled. `on_audio` gets mono samples at `rate` on the
/// audio thread. Not `Send`: keep it on the thread that opened it.
pub fn open(rate: u32, mut on_audio: impl FnMut(&[f32]) + 'static) -> Result<VoiceMic, String> {
    let e = |e: coreaudio::Error| e.to_string();
    let mut unit = AudioUnit::new_uninitialized(IOType::VoiceProcessingIO).map_err(e)?;
    // By default the unit turns everything else the Mac plays far down, Markon's voice included.
    // Older systems lack the setting: the default ducking stays.
    let _ = unit.set_property(
        DUCKING,
        Scope::Global,
        Element::Output,
        Some(&Ducking {
            advanced: 0,
            level: DUCK_LEAST,
        }),
    );
    let format = StreamFormat {
        sample_rate: f64::from(rate),
        sample_format: SampleFormat::F32,
        flags: LinearPcmFlags::IS_FLOAT | LinearPcmFlags::IS_PACKED,
        channels: 1,
    };
    unit.set_stream_format(format, Scope::Output, Element::Input)
        .map_err(e)?;
    // The unit's own (unused) speaker side must run at the same rate, or it will not initialise.
    unit.set_stream_format(format, Scope::Input, Element::Output)
        .map_err(e)?;
    unit.initialize().map_err(e)?;
    unit.set_input_callback(move |args: Args<Interleaved<f32>>| {
        on_audio(args.data.buffer);
        Ok(())
    })
    .map_err(e)?;
    unit.start().map_err(e)?;
    Ok(VoiceMic { _unit: unit })
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    /// Opens the real microphone: `cargo test -p markon-platform voice_mic -- --ignored --nocapture`.
    #[test]
    #[ignore = "needs a microphone and permission to use it"]
    fn delivers_microphone_audio() {
        let heard = Arc::new(Mutex::new(Vec::new()));
        let h = heard.clone();
        let mic = super::open(48_000, move |pcm| h.lock().unwrap().extend_from_slice(pcm)).unwrap();
        std::thread::sleep(std::time::Duration::from_secs(1));
        drop(mic);
        let pcm = heard.lock().unwrap();
        let rms = (pcm.iter().map(|x| x * x).sum::<f32>() / pcm.len().max(1) as f32).sqrt();
        println!("{} samples in 1 s, rms {rms:.5}", pcm.len());
        assert!((40_000..=56_000).contains(&pcm.len()));
    }
}
