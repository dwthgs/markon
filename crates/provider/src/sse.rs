//! Minimal Server-Sent Events parser: feed bytes, get (event, data) pairs.

#[derive(Default)]
pub struct SseParser {
    buf: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SseEvent {
    pub event: String,
    pub data: String,
}

impl SseParser {
    pub fn push(&mut self, bytes: &[u8]) -> Vec<SseEvent> {
        self.buf.push_str(&String::from_utf8_lossy(bytes));
        if self.buf.contains('\r') {
            self.buf = self.buf.replace("\r\n", "\n");
        }
        let mut out = Vec::new();
        while let Some(end) = self.buf.find("\n\n") {
            let block: String = self.buf.drain(..end + 2).collect();
            let mut ev = SseEvent {
                event: "message".into(),
                data: String::new(),
            };
            for line in block.lines() {
                if let Some(v) = line.strip_prefix("event:") {
                    ev.event = v.trim().into();
                } else if let Some(v) = line.strip_prefix("data:") {
                    if !ev.data.is_empty() {
                        ev.data.push('\n');
                    }
                    ev.data.push_str(v.strip_prefix(' ').unwrap_or(v));
                }
            }
            if !ev.data.is_empty() {
                out.push(ev);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_events_across_chunks() {
        let mut p = SseParser::default();
        assert!(p.push(b"event: ping\nda").is_empty());
        let evs = p.push(b"ta: {}\n\nevent: x\ndata: 1\r\n\r\n");
        assert_eq!(
            evs,
            vec![
                SseEvent {
                    event: "ping".into(),
                    data: "{}".into()
                },
                SseEvent {
                    event: "x".into(),
                    data: "1".into()
                }
            ]
        );
    }
}
