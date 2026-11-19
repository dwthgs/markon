//! Speech from Cartesia's hosted Sonic model, an alternative to Kokoro for the same two
//! voices. The key comes from the keychain, the text leaves this Mac, and a failed request speaks
//! nothing: there is no fallback voice.
use crate::tts::TtsEngine;
use crate::{AudioError, Voice};
use std::sync::OnceLock;
use std::time::Duration;

pub const RATE: u32 = 24_000;
const API: &str = "https://api.cartesia.ai";
const VERSION: &str = "2026-08-14";
const MODEL: &str = "sonic-3.6";

/// Cartesia's British voices closest to Kokoro's: Archie for Markon, Gemma for Markan.
pub fn voice_id(voice: Voice) -> &'static str {
    match voice {
        Voice::Markon => "ef191366-f52f-447a-a398-ed8c0f2943a1",
        Voice::Markan => "62ae83ad-4f6a-430b-af41-a9bede9286ca",
    }
}

/// One runtime for every request, never dropped: the speaker thread blocks on it, and keeping it
/// alive keeps the connection pool warm between sentences.
fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("cartesia runtime")
    })
}

#[derive(Clone)]
pub struct Cartesia {
    key: String,
    voice: Voice,
    base: String,
    client: reqwest::Client,
}

impl Cartesia {
    pub fn new(key: String, voice: Voice) -> Self {
        Self::with_base(key, voice, API)
    }

    /// For tests: a local fake server instead of Cartesia.
    pub fn with_base(key: String, voice: Voice, base: &str) -> Self {
        Self {
            key,
            voice,
            base: base.trim_end_matches('/').to_string(),
            client: reqwest::Client::new(),
        }
    }

    pub fn voice(&self) -> Voice {
        self.voice
    }

    pub fn select(&mut self, voice: Voice) {
        self.voice = voice;
    }

    fn body(&self, text: &str) -> serde_json::Value {
        serde_json::json!({
            "model_id": MODEL,
            "transcript": text,
            "voice": { "id": voice_id(self.voice) },
            "language": "en",
            "output_format": { "container": "raw", "encoding": "pcm_f32le", "sample_rate": RATE },
        })
    }
}

fn decode(bytes: &[u8]) -> Vec<f32> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect()
}

impl TtsEngine for Cartesia {
    fn synth(&mut self, text: &str) -> Result<Vec<f32>, AudioError> {
        let req = self
            .client
            .post(format!("{}/tts/bytes", self.base))
            .bearer_auth(&self.key)
            .header("Cartesia-Version", VERSION)
            .timeout(Duration::from_secs(15))
            .json(&self.body(text));
        let fail = |e: String| AudioError::Model(format!("cartesia: {e}"));
        runtime().block_on(async {
            let r = req
                .send()
                .await
                .map_err(|e| fail(e.without_url().to_string()))?;
            let status = r.status();
            let bytes = r.bytes().await.map_err(|e| fail(e.to_string()))?;
            if !status.is_success() {
                let msg = String::from_utf8_lossy(&bytes[..bytes.len().min(200)]).to_string();
                return Err(fail(format!("{status}: {msg}")));
            }
            Ok(decode(&bytes))
        })
    }

    fn sample_rate(&self) -> u32 {
        RATE
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// Answers one request with `status` and `body`, and hands back the raw request it received.
    fn fake(status: &str, body: Vec<u8>) -> (String, std::thread::JoinHandle<String>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        let status = status.to_string();
        let h = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut req = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let n = s.read(&mut buf).unwrap();
                req.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&req).to_string();
                if let Some(end) = text.find("\r\n\r\n") {
                    let len = text
                        .lines()
                        .find_map(|l| {
                            l.to_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if req.len() >= end + 4 + len {
                        break;
                    }
                }
            }
            let head = format!(
                "HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            s.write_all(head.as_bytes()).unwrap();
            s.write_all(&body).unwrap();
            String::from_utf8_lossy(&req).to_string()
        });
        (base, h)
    }

    #[test]
    fn speaks_the_chosen_voice_as_raw_float_audio() {
        let samples = [0.25f32, -0.5, 1.0];
        let (base, h) = fake(
            "200 OK",
            samples.iter().flat_map(|s| s.to_le_bytes()).collect(),
        );
        let mut c = Cartesia::with_base("test-key".into(), Voice::Markan, &base);
        assert_eq!(c.synth("Yes, sir.").unwrap(), samples);
        let req = h.join().unwrap();
        assert!(req.starts_with("POST /tts/bytes "), "{req}");
        let lower = req.to_lowercase();
        assert!(lower.contains("authorization: bearer test-key"), "{req}");
        assert!(lower.contains("cartesia-version: 2026-08-14"), "{req}");
        let body: serde_json::Value =
            serde_json::from_str(&req[req.find("\r\n\r\n").unwrap() + 4..]).unwrap();
        assert_eq!(body["transcript"], "Yes, sir.");
        assert_eq!(body["model_id"], MODEL);
        assert_eq!(body["voice"]["id"], voice_id(Voice::Markan));
        assert_eq!(body["output_format"]["encoding"], "pcm_f32le");
        assert_eq!(body["output_format"]["sample_rate"], RATE);
    }

    #[test]
    fn switching_voice_changes_the_requested_voice() {
        let mut c = Cartesia::new("k".into(), Voice::Markon);
        c.select(Voice::Markan);
        assert_eq!(c.body("hi")["voice"]["id"], voice_id(Voice::Markan));
        assert_ne!(voice_id(Voice::Markon), voice_id(Voice::Markan));
    }

    #[test]
    fn a_refused_request_is_an_error_without_the_key() {
        let (base, h) = fake(
            "401 Unauthorized",
            b"{\"error\":\"invalid api key\"}".to_vec(),
        );
        let mut c = Cartesia::with_base("secret-key-123".into(), Voice::Markon, &base);
        let e = c.synth("Hello.").unwrap_err().to_string();
        h.join().unwrap();
        assert!(e.contains("401") && e.contains("invalid api key"), "{e}");
        assert!(!e.contains("secret-key-123"), "{e}");
    }

    #[test]
    fn no_server_is_an_error_not_silence_passed_off_as_speech() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        drop(l);
        let mut c = Cartesia::with_base("k".into(), Voice::Markon, &base);
        assert!(c.synth("Hello.").is_err());
    }
}
