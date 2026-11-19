//! The running audio service. cpal streams are not `Send` on macOS, so each lives on its own thread.
use crate::cartesia::Cartesia;
use crate::features::{FeatureExtractor, Features};
use crate::manifest::Manifest;
use crate::resample::{downmix, resample};
use crate::stt::{SttEngine, Whisper};
use crate::tts::{Kokoro, TtsEngine, Voice, split_first_chunk};
use crate::vad::{EndOfSpeech, Vad, ends_clause};
use crate::{AudioError, g2p::G2p};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use serde::Serialize;
use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub const WHISPER_RATE: u32 = 16_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Who {
    User,
    Markon,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AudioEvent {
    PartialTranscript {
        text: String,
    },
    FinalTranscript {
        text: String,
        ms: u64,
    },
    SpeechStarted,
    SpeechEnded,
    Features {
        who: Who,
        rms: f32,
        pitch: f32,
        bands: Vec<f32>,
    },
    PlaybackStarted,
    PlaybackEnded,
    /// The user started talking while Markon was speaking.
    BargeIn,
    NoMicrophone,
    MicDenied,
    ModelsLoading,
    ModelsReady,
    ModelMismatch {
        id: String,
    },
    Error {
        message: String,
    },
    /// "Hey Markon" was heard; `rest` is anything said in the same breath.
    Wake {
        rest: String,
    },
}

pub type EventSink = Arc<dyn Fn(AudioEvent) + Send + Sync>;

type Stt = Arc<Mutex<Option<Box<dyn SttEngine>>>>;
type Tts = Arc<Mutex<Option<Box<dyn TtsEngine>>>>;
/// When set, Cartesia speaks instead of Kokoro. Kokoro stays loaded to switch back.
type Online = Arc<Mutex<Option<Cartesia>>>;

struct Capture {
    stop: Arc<AtomicBool>,
    buf: Arc<Mutex<Vec<f32>>>,
    rate: Arc<Mutex<u32>>,
    thread: JoinHandle<()>,
}

enum Job {
    Speak { generation: u64, text: String },
}

pub(crate) struct Playback {
    queue: Mutex<VecDeque<f32>>,
    generation: AtomicU64,
    /// A reply is in progress: set with its first sound, cleared once nothing more is queued. It stays
    /// set through the silence while the next sentence is synthesised.
    pub(crate) active: AtomicBool,
    /// Markon's voice is leaving the speaker now (the last output callback played queued samples).
    pub(crate) sounding: AtomicBool,
    /// Samples taken from the queue since the app started.
    popped: AtomicU64,
    spoken: Mutex<Spoken>,
    /// What the speakers just played, for the echo canceller (mono, at `render_rate`).
    pub(crate) render: Mutex<Vec<f32>>,
    pub(crate) render_rate: AtomicU32,
    /// The user may be starting to talk over Markon: play at a quarter of the volume.
    pub(crate) duck: AtomicBool,
    /// The last short phrase ("Yes?") as played: (text, output rate, samples). Synthesising it on every
    /// wake cost 0.4-1 s. Cleared whenever the voice or engine changes.
    ack: Mutex<Option<(String, u32, Vec<f32>)>>,
}

/// Phrases up to this long are cached: the wake answer, not reply sentences.
const ACK_MAX_CHARS: usize = 12;

/// The sentences of the current reply and where each ends in the stream of queued samples, so a cut
/// knows which ones finished playing.
#[derive(Default)]
struct Spoken {
    pushed: u64,
    sentences: Vec<(u64, String)>,
    /// The sentence that was playing when the reply was cut.
    cut_off: Option<String>,
}

impl Spoken {
    fn add(&mut self, samples: usize, text: String) {
        self.pushed += samples as u64;
        self.sentences.push((self.pushed, text));
    }

    /// The queue was emptied after `popped` samples had played.
    fn cut(&mut self, popped: u64) {
        self.pushed = popped;
        let done = self.sentences.partition_point(|(end, _)| *end <= popped);
        if let Some((_, s)) = self.sentences.get(done) {
            self.cut_off = Some(s.clone());
        }
        self.sentences.truncate(done);
    }

    /// The sentences that finished playing, and those plus the one cut off mid-way.
    fn said(&self) -> (String, String) {
        let done: Vec<&str> = self.sentences.iter().map(|(_, s)| s.as_str()).collect();
        let heard = done
            .iter()
            .copied()
            .chain(self.cut_off.as_deref())
            .collect::<Vec<_>>();
        (done.join(" "), heard.join(" "))
    }
}

impl Playback {
    /// Silence now: drop queued sentences and the unplayed tail. `active` stays set, so the app's
    /// own `cancel_speech` still reports the end of playback.
    pub(crate) fn cut(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        let mut q = self.queue.lock().expect("queue");
        q.clear();
        self.spoken
            .lock()
            .expect("spoken")
            .cut(self.popped.load(Ordering::SeqCst));
        drop(q);
        self.duck.store(false, Ordering::SeqCst);
    }
}

pub struct Audio {
    sink: EventSink,
    stt: Stt,
    tts: Tts,
    online: Online,
    capture: Mutex<Option<Capture>>,
    play: Arc<Playback>,
    jobs: Sender<Job>,
    first_of_reply: AtomicBool,
    /// True while a listening session holds the microphone (the wake listener steps aside).
    listening: Arc<AtomicBool>,
    wake: Mutex<Option<(Arc<AtomicBool>, JoinHandle<()>)>>,
    handoff: crate::wake::Handoff,
    /// A turn is running: words said while Markon thinks reach it without the wake phrase.
    attentive: Arc<AtomicBool>,
    /// The voice the loaded speech engine speaks with (the persona's name follows it).
    voice: Mutex<Option<Voice>>,
    /// The verified Silero model: without it talking over Markon is off.
    vad: Mutex<Option<std::path::PathBuf>>,
}

impl Audio {
    pub fn new(sink: EventSink) -> Self {
        let stt: Stt = Arc::new(Mutex::new(None));
        let tts: Tts = Arc::new(Mutex::new(None));
        let online: Online = Arc::new(Mutex::new(None));
        let play = Arc::new(Playback {
            queue: Mutex::new(VecDeque::new()),
            generation: AtomicU64::new(0),
            active: AtomicBool::new(false),
            render: Mutex::new(Vec::new()),
            render_rate: AtomicU32::new(0),
            duck: AtomicBool::new(false),
            sounding: AtomicBool::new(false),
            popped: AtomicU64::new(0),
            spoken: Mutex::new(Spoken::default()),
            ack: Mutex::new(None),
        });
        let (jobs, rx) = channel();
        {
            let (sink, tts, online, play) =
                (sink.clone(), tts.clone(), online.clone(), play.clone());
            std::thread::Builder::new()
                .name("markon-speaker".into())
                .spawn(move || speaker_thread(rx, sink, tts, online, play))
                .expect("speaker thread");
        }
        Self {
            sink,
            stt,
            tts,
            online,
            capture: Mutex::new(None),
            play,
            jobs,
            first_of_reply: AtomicBool::new(true),
            listening: Arc::new(AtomicBool::new(false)),
            wake: Mutex::new(None),
            handoff: Default::default(),
            attentive: Default::default(),
            voice: Mutex::new(None),
            vad: Mutex::new(None),
        }
    }

    /// Plug in engines directly (tests and alternative engines).
    pub fn set_engines(&self, stt: Option<Box<dyn SttEngine>>, tts: Option<Box<dyn TtsEngine>>) {
        *self.stt.lock().expect("stt") = stt;
        *self.tts.lock().expect("tts") = tts;
        self.play.ack.lock().expect("ack").take();
    }

    /// Listen for "Hey Markon" in the background until `stop_wake`. Needs the models loaded.
    pub fn start_wake(&self) {
        let mut w = self.wake.lock().expect("wake");
        if w.is_some() {
            return;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let vad = self.vad.lock().expect("vad").clone();
        let (st, listening, attentive, play, handoff, sink, stt) = (
            stop.clone(),
            self.listening.clone(),
            self.attentive.clone(),
            self.play.clone(),
            self.handoff.clone(),
            self.sink.clone(),
            self.stt.clone(),
        );
        let transcribe = move |pcm16: &[f32]| {
            let mut guard = stt.lock().ok()?;
            guard.as_mut()?.transcribe(pcm16).ok()
        };
        if let Ok(t) = std::thread::Builder::new()
            .name("markon-wake".into())
            .spawn(move || {
                crate::wake::wake_thread(
                    st, listening, attentive, play, handoff, sink, vad, transcribe,
                )
            })
        {
            *w = Some((stop, t));
        }
    }

    /// While set, anything said (not only the wake phrase) is reported as `Wake`.
    pub fn set_attentive(&self, on: bool) {
        self.attentive.store(on, Ordering::SeqCst);
    }

    pub fn stop_wake(&self) {
        if let Some((stop, t)) = self.wake.lock().expect("wake").take() {
            stop.store(true, Ordering::SeqCst);
            let _ = t.join();
        }
    }

    pub fn is_ready(&self) -> bool {
        self.tts.lock().map(|t| t.is_some()).unwrap_or(false)
            && self.stt.lock().map(|s| s.is_some()).unwrap_or(false)
    }

    /// Verify every pinned file, then load whisper and Kokoro with the chosen voice.
    /// A file that does not match its checksum is never loaded: Markon refuses to speak.
    /// `hint` is vocabulary whisper should expect, passed to [`Whisper::load`].
    pub fn load_models(&self, dir: &Path, voice: Voice, hint: &str) -> Result<(), AudioError> {
        (self.sink)(AudioEvent::ModelsLoading);
        let m = Manifest::bundled();
        let check = |id: &str| {
            m.entry(id)?.verify(dir).map_err(|e| {
                if let crate::manifest::ManifestError::Mismatch { id, .. } = &e {
                    (self.sink)(AudioEvent::ModelMismatch { id: id.clone() });
                }
                AudioError::from(e)
            })
        };
        let whisper = check("whisper-base.en")?;
        let kokoro = check("kokoro-v1")?;
        let voice_file = check(voice.manifest_id())?;
        let gold = check("g2p-gb-gold")?;
        let silver = check("g2p-gb-silver")?;
        // Talking over Markon only: a bad file turns that off and says so, Markon still speaks.
        *self.vad.lock().expect("vad") = m
            .entry("silero-vad")
            .and_then(|e| e.verify(dir))
            .inspect_err(|e| {
                eprintln!("[barge] Silero VAD not verified ({e}): talking over Markon is off")
            })
            .ok();
        let stt = Whisper::load(&whisper, hint)?;
        let tts = Kokoro::load(&kokoro, &voice_file, G2p::load(&gold, &silver)?)?;
        self.set_engines(Some(Box::new(stt)), Some(Box::new(tts)));
        *self.voice.lock().expect("voice") = Some(voice);
        (self.sink)(AudioEvent::ModelsReady);
        Ok(())
    }

    /// Switch the speaking voice. The file is verified against its pinned checksum first.
    pub fn set_voice(&self, dir: &Path, voice: Voice) -> Result<(), AudioError> {
        let path = Manifest::bundled()
            .entry(voice.manifest_id())?
            .verify(dir)?;
        match self.tts.lock().expect("tts").as_mut() {
            Some(t) => t.set_voice(&path)?,
            None => return Err(AudioError::NotReady),
        }
        if let Some(c) = self.online.lock().expect("online").as_mut() {
            c.select(voice);
        }
        *self.voice.lock().expect("voice") = Some(voice);
        self.play.ack.lock().expect("ack").take();
        Ok(())
    }

    /// Speak through Cartesia (Some) or Kokoro (None) from the next sentence on.
    pub fn set_online(&self, engine: Option<Cartesia>) {
        *self.online.lock().expect("online") = engine;
        self.play.ack.lock().expect("ack").take();
    }

    pub fn is_online(&self) -> bool {
        self.online.lock().map(|o| o.is_some()).unwrap_or(false)
    }

    /// The voice the loaded engine speaks with, if any.
    pub fn voice(&self) -> Option<Voice> {
        *self.voice.lock().expect("voice")
    }

    pub fn is_listening(&self) -> bool {
        self.capture.lock().map(|c| c.is_some()).unwrap_or(false)
    }

    /// Open the current default microphone (so a newly plugged headset is picked up) and record.
    pub fn start_listening(&self) -> Result<(), AudioError> {
        let mut cap = self.capture.lock().expect("capture");
        if cap.is_some() {
            return Ok(());
        }
        let stop = Arc::new(AtomicBool::new(false));
        let buf = Arc::new(Mutex::new(Vec::new()));
        let rate = Arc::new(Mutex::new(0u32));
        let (ready_tx, ready_rx) = channel::<Result<(), AudioError>>();
        let thread = {
            let (stop, buf, rate, sink, stt) = (
                stop.clone(),
                buf.clone(),
                rate.clone(),
                self.sink.clone(),
                self.stt.clone(),
            );
            std::thread::Builder::new()
                .name("markon-mic".into())
                .spawn(move || capture_thread(stop, buf, rate, sink, stt, ready_tx))
                .map_err(|e| AudioError::Device(e.to_string()))?
        };
        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => {
                self.listening.store(true, Ordering::SeqCst);
                // Speech said over Markon (barge-in) starts this clip, so its first words are kept.
                let (r, lead) = std::mem::take(&mut *self.handoff.lock().expect("handoff"));
                if r == *rate.lock().expect("rate") && !lead.is_empty() {
                    let mut b = buf.lock().expect("buf");
                    b.splice(0..0, lead);
                }
                *cap = Some(Capture {
                    stop,
                    buf,
                    rate,
                    thread,
                });
                Ok(())
            }
            Ok(Err(e)) => {
                let _ = thread.join();
                (self.sink)(match e {
                    AudioError::NoMicrophone => AudioEvent::NoMicrophone,
                    AudioError::MicDenied => AudioEvent::MicDenied,
                    ref other => AudioEvent::Error {
                        message: other.to_string(),
                    },
                });
                Err(e)
            }
            Err(_) => {
                stop.store(true, Ordering::SeqCst);
                Err(AudioError::Device("microphone did not start".into()))
            }
        }
    }

    /// Stop recording and return the final transcript (blocking: run off the async runtime).
    pub fn stop_listening(&self) -> Result<String, AudioError> {
        let Some(cap) = self.capture.lock().expect("capture").take() else {
            return Ok(String::new());
        };
        let stopped = Instant::now();
        cap.stop.store(true, Ordering::SeqCst);
        let _ = cap.thread.join();
        self.listening.store(false, Ordering::SeqCst);
        let rate = *cap.rate.lock().expect("rate");
        let pcm = std::mem::take(&mut *cap.buf.lock().expect("buf"));
        let pcm16 = resample(&pcm, rate, WHISPER_RATE);
        let text = match self.stt.lock().expect("stt").as_mut() {
            Some(stt) => stt.transcribe(&pcm16)?,
            None => return Err(AudioError::NotReady),
        };
        (self.sink)(AudioEvent::FinalTranscript {
            text: text.clone(),
            ms: stopped.elapsed().as_millis() as u64,
        });
        Ok(text)
    }

    /// Start a new reply: the next sentence is split so its first words play sooner.
    pub fn begin_reply(&self) {
        self.first_of_reply.store(true, Ordering::SeqCst);
        *self.play.spoken.lock().expect("spoken") = Spoken {
            pushed: self.play.popped.load(Ordering::SeqCst),
            ..Spoken::default()
        };
    }

    /// What Markon said aloud in the current or just cut reply: the sentences that finished playing,
    /// and those plus the one it was in the middle of (for telling its own echo from the user).
    pub fn spoken(&self) -> (String, String) {
        self.play.spoken.lock().expect("spoken").said()
    }

    /// Queue one sentence for speech. Refuses if the pinned voice is not loaded.
    pub fn speak(&self, sentence: &str) -> Result<(), AudioError> {
        if self.tts.lock().map(|t| t.is_none()).unwrap_or(true) {
            return Err(AudioError::NotReady);
        }
        let generation = self.play.generation.load(Ordering::SeqCst);
        let texts = if self.first_of_reply.swap(false, Ordering::SeqCst) {
            let (a, b) = split_first_chunk(sentence);
            [Some(a), b].into_iter().flatten().collect()
        } else {
            vec![sentence.to_string()]
        };
        for text in texts {
            self.jobs
                .send(Job::Speak { generation, text })
                .map_err(|_| AudioError::Device("speaker stopped".into()))?;
        }
        Ok(())
    }

    /// Stop speaking now: drop queued sentences and the unplayed tail.
    pub fn cancel_speech(&self) {
        self.play.cut();
        if self.play.active.swap(false, Ordering::SeqCst) {
            (self.sink)(AudioEvent::PlaybackEnded);
        }
    }

    pub fn is_speaking(&self) -> bool {
        self.play.active.load(Ordering::SeqCst)
    }
}

fn input_error(e: impl std::fmt::Display) -> AudioError {
    let s = e.to_string();
    if s.to_lowercase().contains("permission") || s.to_lowercase().contains("not permitted") {
        AudioError::MicDenied
    } else {
        AudioError::Device(s)
    }
}

fn capture_thread(
    stop: Arc<AtomicBool>,
    buf: Arc<Mutex<Vec<f32>>>,
    rate_out: Arc<Mutex<u32>>,
    sink: EventSink,
    stt: Stt,
    ready: Sender<Result<(), AudioError>>,
) {
    let host = cpal::default_host();
    let Some(dev) = host.default_input_device() else {
        let _ = ready.send(Err(AudioError::NoMicrophone));
        return;
    };
    let cfg = match dev.default_input_config() {
        Ok(c) => c,
        Err(e) => {
            let _ = ready.send(Err(input_error(e)));
            return;
        }
    };
    let channels = cfg.channels() as usize;
    let rate = cfg.sample_rate();
    *rate_out.lock().expect("rate") = rate;
    let config: cpal::StreamConfig = cfg.into();
    let mut fx = FeatureExtractor::new(rate);
    let mut vad = Vad::new(rate);
    let partial = Arc::new(Mutex::new(String::new()));
    let (b, s, p) = (buf.clone(), sink.clone(), partial.clone());
    let mut mono = Vec::with_capacity(4096);
    let stream = dev.build_input_stream(
        config,
        move |data: &[f32], _: &cpal::InputCallbackInfo| {
            mono.clear();
            mono.extend(downmix(data, channels));
            if let Ok(mut b) = b.try_lock() {
                b.extend_from_slice(&mono);
            }
            if let Some(f) = fx.push(&mono) {
                s(AudioEvent::Features {
                    who: Who::User,
                    rms: f.rms,
                    pitch: f.pitch,
                    bands: f.bands,
                });
            }
            let mid = p
                .try_lock()
                .map(|t| !t.is_empty() && !ends_clause(&t))
                .unwrap_or(false);
            match vad.push(&mono, mid) {
                Some(EndOfSpeech::Started) => s(AudioEvent::SpeechStarted),
                Some(EndOfSpeech::Ended) => s(AudioEvent::SpeechEnded),
                None => {}
            }
        },
        |e| eprintln!("[audio] input stream error: {e}"),
        None,
    );
    let stream = match stream {
        Ok(s) => s,
        Err(e) => {
            let _ = ready.send(Err(input_error(e)));
            return;
        }
    };
    if let Err(e) = stream.play() {
        let _ = ready.send(Err(input_error(e)));
        return;
    }
    let _ = ready.send(Ok(()));
    // Partial transcripts: whisper does not stream, so re-run on the growing clip about once a second.
    let mut last_len = 0;
    let mut next = Instant::now() + Duration::from_millis(900);
    while !stop.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(20));
        if Instant::now() < next {
            continue;
        }
        next = Instant::now() + Duration::from_millis(900);
        let pcm = buf.lock().expect("buf").clone();
        if pcm.len() <= last_len + rate as usize / 4 {
            continue;
        }
        last_len = pcm.len();
        let Ok(mut guard) = stt.try_lock() else {
            continue;
        };
        let Some(engine) = guard.as_mut() else {
            continue;
        };
        if let Ok(text) = engine.transcribe(&resample(&pcm, rate, WHISPER_RATE)) {
            drop(guard);
            if !text.is_empty() && !stop.load(Ordering::SeqCst) {
                *partial.lock().expect("partial") = text.clone();
                sink(AudioEvent::PartialTranscript { text });
            }
        }
    }
    drop(stream);
}

struct Output {
    _stream: cpal::Stream,
    rate: u32,
}

fn open_output(play: Arc<Playback>, sink: EventSink) -> Result<Output, AudioError> {
    let host = cpal::default_host();
    let dev = host.default_output_device().ok_or(AudioError::NoOutput)?;
    let cfg = dev
        .default_output_config()
        .map_err(|e| AudioError::Device(e.to_string()))?;
    let channels = cfg.channels() as usize;
    let rate = cfg.sample_rate();
    play.render_rate.store(rate, Ordering::SeqCst);
    let mut fx = FeatureExtractor::new(rate);
    let mut mono = Vec::with_capacity(4096);
    let stream = dev
        .build_output_stream(
            cfg.into(),
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                mono.clear();
                {
                    let mut q = play.queue.lock().expect("queue");
                    let gain = if play.duck.load(Ordering::Relaxed) {
                        0.25
                    } else {
                        1.0
                    };
                    let mut popped = 0;
                    for frame in data.chunks_mut(channels) {
                        let s = q.pop_front().map_or(0.0, |s| {
                            popped += 1;
                            s * gain
                        });
                        frame.fill(s);
                        mono.push(s);
                    }
                    play.popped.fetch_add(popped, Ordering::SeqCst);
                    play.sounding.store(popped > 0, Ordering::Relaxed);
                }
                if let Ok(mut r) = play.render.try_lock() {
                    r.extend_from_slice(&mono);
                    // Nobody reading (no microphone): keep at most a second.
                    let cap = rate as usize;
                    if r.len() > cap {
                        let extra = r.len() - cap;
                        r.drain(..extra);
                    }
                }
                if play.active.load(Ordering::Relaxed)
                    && let Some(f) = fx.push(&mono)
                {
                    sink(AudioEvent::Features {
                        who: Who::Markon,
                        rms: f.rms,
                        pitch: f.pitch,
                        bands: f.bands,
                    });
                }
            },
            |e| eprintln!("[audio] output stream error: {e}"),
            None,
        )
        .map_err(|e| AudioError::Device(e.to_string()))?;
    stream
        .play()
        .map_err(|e| AudioError::Device(e.to_string()))?;
    Ok(Output {
        _stream: stream,
        rate,
    })
}

fn speaker_thread(
    rx: Receiver<Job>,
    sink: EventSink,
    tts: Tts,
    online: Online,
    play: Arc<Playback>,
) {
    let mut out: Option<Output> = None;
    loop {
        match rx.recv_timeout(Duration::from_millis(40)) {
            Ok(Job::Speak { generation, text }) => {
                if generation != play.generation.load(Ordering::SeqCst) {
                    continue;
                }
                if out.is_none() {
                    match open_output(play.clone(), sink.clone()) {
                        Ok(o) => out = Some(o),
                        Err(e) => {
                            sink(AudioEvent::Error {
                                message: e.to_string(),
                            });
                            continue;
                        }
                    }
                }
                let Some(o) = out.as_ref() else { continue };
                // A clone, so a slow request never holds the lock settings need to switch engines.
                let short = text.chars().count() <= ACK_MAX_CHARS;
                let cached = play
                    .ack
                    .lock()
                    .expect("ack")
                    .as_ref()
                    .filter(|(t, r, _)| short && *t == text && *r == o.rate)
                    .map(|(.., s)| s.clone());
                let cloud = online.lock().expect("online").clone();
                let samples = if let Some(s) = cached {
                    s
                } else if let Some(mut c) = cloud {
                    match c.synth(&text) {
                        Ok(s) => resample(&s, c.sample_rate(), o.rate),
                        Err(e) => {
                            sink(AudioEvent::Error {
                                message: e.to_string(),
                            });
                            continue;
                        }
                    }
                } else {
                    let mut guard = tts.lock().expect("tts");
                    let Some(engine) = guard.as_mut() else {
                        sink(AudioEvent::Error {
                            message: "voice not verified: refusing to speak".into(),
                        });
                        continue;
                    };
                    match engine.synth(&text) {
                        Ok(s) => resample(&s, engine.sample_rate(), o.rate),
                        Err(e) => {
                            sink(AudioEvent::Error {
                                message: e.to_string(),
                            });
                            continue;
                        }
                    }
                };
                if short {
                    *play.ack.lock().expect("ack") = Some((text.clone(), o.rate, samples.clone()));
                }
                if generation != play.generation.load(Ordering::SeqCst) {
                    continue; // cancelled while synthesising
                }
                // Recorded first: its end must never look played before its samples are queued.
                play.spoken.lock().expect("spoken").add(samples.len(), text);
                play.queue.lock().expect("queue").extend(samples);
                if !play.active.swap(true, Ordering::SeqCst) {
                    sink(AudioEvent::PlaybackStarted);
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                // Finished: close the device so the next reply follows output changes (headphones).
                if out.is_some() && play.queue.lock().map(|q| q.is_empty()).unwrap_or(true) {
                    if play.active.swap(false, Ordering::SeqCst) {
                        sink(AudioEvent::PlaybackEnded);
                    }
                    out = None;
                    play.sounding.store(false, Ordering::SeqCst);
                }
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// Features of a finished frame, re-exported for the app.
pub type FeatureFrame = Features;

#[cfg(test)]
mod tests {
    use super::Spoken;

    #[test]
    fn a_cut_keeps_only_the_sentences_that_finished_playing() {
        let mut s = Spoken::default();
        s.add(100, "One.".into());
        s.add(100, "Two.".into());
        s.add(100, "Three.".into());
        s.cut(150);
        assert_eq!(s.said(), ("One.".into(), "One. Two.".into()));
        // Cancelling again after the cut changes nothing.
        s.cut(150);
        assert_eq!(s.said(), ("One.".into(), "One. Two.".into()));
        // Positions carry on from the cut for whatever is queued next.
        s.add(50, "Four.".into());
        s.cut(200);
        assert_eq!(s.said().0, "One. Four.");
    }
}
