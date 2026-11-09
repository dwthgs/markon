//! Splits a streamed reply into sentences so each can be spoken as soon as it is complete.

pub struct SentenceQueue {
    buf: String,
    cap: usize,
    emitted: usize,
    dropped: usize,
}

impl SentenceQueue {
    /// `cap` is the most sentences one reply may queue for speech; later ones are dropped, oldest kept.
    pub fn new(cap: usize) -> Self {
        Self {
            buf: String::new(),
            cap,
            emitted: 0,
            dropped: 0,
        }
    }

    pub fn dropped(&self) -> usize {
        self.dropped
    }

    pub fn push(&mut self, delta: &str) -> Vec<String> {
        self.buf.push_str(delta);
        let mut out = Vec::new();
        loop {
            let b = self.buf.as_bytes();
            let Some(end) = (0..b.len().saturating_sub(1))
                .find(|&i| matches!(b[i], b'.' | b'!' | b'?') && b[i + 1].is_ascii_whitespace())
            else {
                break;
            };
            let sentence: String = self.buf[..=end].trim().to_string();
            self.buf.drain(..=end);
            self.emit(sentence, &mut out);
        }
        out
    }

    /// The unfinished tail once the reply has ended.
    pub fn finish(&mut self) -> Option<String> {
        let tail = std::mem::take(&mut self.buf).trim().to_string();
        let mut out = Vec::new();
        self.emit(tail, &mut out);
        out.pop()
    }

    fn emit(&mut self, s: String, out: &mut Vec<String>) {
        if s.is_empty() {
            return;
        }
        if self.emitted < self.cap {
            self.emitted += 1;
            out.push(s);
        } else {
            self.dropped += 1;
        }
    }
}
