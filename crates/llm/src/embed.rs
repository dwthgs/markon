//! Meaning search: all-MiniLM-L6-v2 (int8 ONNX, 23 MB, pinned in the manifest) run with
//! `ort`, the runtime Kokoro already uses.
//! A BERT WordPiece tokenizer, mean pooling over the attention mask, unit length.
use ort::session::Session;
use ort::value::Tensor;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

pub const MODEL_ID: &str = "minilm-l6-v2";
pub const VOCAB_ID: &str = "minilm-l6-v2-vocab";
const MAX_TOKENS: usize = 256;

/// BERT's uncased WordPiece: lowercase, accents off, punctuation split out, longest match first.
pub struct WordPiece {
    vocab: HashMap<String, i64>,
    unk: i64,
    cls: i64,
    sep: i64,
}

fn strip_accents(c: char) -> Option<char> {
    // The common Latin accents; anything else passes through unchanged.
    const FROM: &str = "àáâãäåçèéêëìíîïñòóôõöùúûüýÿ";
    const TO: &str = "aaaaaaceeeeiiiinooooouuuuyy";
    FROM.chars()
        .position(|f| f == c)
        .and_then(|i| TO.chars().nth(i))
        .or(Some(c))
}

impl WordPiece {
    pub fn from_vocab(text: &str) -> Self {
        let vocab: HashMap<String, i64> = text
            .lines()
            .enumerate()
            .map(|(i, t)| (t.to_string(), i as i64))
            .collect();
        let id = |t: &str| vocab.get(t).copied().unwrap_or(0);
        Self {
            unk: id("[UNK]"),
            cls: id("[CLS]"),
            sep: id("[SEP]"),
            vocab,
        }
    }

    fn basic(text: &str) -> Vec<String> {
        let mut words = Vec::new();
        let mut cur = String::new();
        for c in text.to_lowercase().chars().filter_map(strip_accents) {
            if c.is_whitespace() || c.is_control() {
                if !cur.is_empty() {
                    words.push(std::mem::take(&mut cur));
                }
            } else if c.is_ascii_punctuation() || (!c.is_alphanumeric() && !c.is_whitespace()) {
                if !cur.is_empty() {
                    words.push(std::mem::take(&mut cur));
                }
                words.push(c.to_string());
            } else {
                cur.push(c);
            }
        }
        if !cur.is_empty() {
            words.push(cur);
        }
        words
    }

    /// Token ids with [CLS] and [SEP], at most `MAX_TOKENS`.
    pub fn encode(&self, text: &str) -> Vec<i64> {
        let mut ids = vec![self.cls];
        'words: for w in Self::basic(text) {
            let chars: Vec<char> = w.chars().collect();
            if chars.len() > 100 {
                ids.push(self.unk);
                continue;
            }
            let mut start = 0;
            let mut pieces = Vec::new();
            while start < chars.len() {
                let mut end = chars.len();
                let mut found = None;
                while start < end {
                    let s: String = chars[start..end].iter().collect();
                    let s = if start > 0 { format!("##{s}") } else { s };
                    if let Some(id) = self.vocab.get(&s) {
                        found = Some(*id);
                        break;
                    }
                    end -= 1;
                }
                match found {
                    Some(id) => {
                        pieces.push(id);
                        start = end;
                    }
                    None => {
                        ids.push(self.unk);
                        continue 'words;
                    }
                }
            }
            ids.extend(pieces);
            if ids.len() >= MAX_TOKENS - 1 {
                break;
            }
        }
        ids.truncate(MAX_TOKENS - 1);
        ids.push(self.sep);
        ids
    }
}

pub struct Embedder {
    session: Mutex<Session>,
    tok: WordPiece,
}

impl Embedder {
    /// Both paths must already be verified against the manifest.
    pub fn load(model: &Path, vocab: &Path) -> Result<Self, String> {
        let env = markon_audio::onnx_env()?;
        let session = Session::builder(env)
            .map_err(|e| e.to_string())?
            .with_intra_threads(2)
            .map_err(|e| e.to_string())?
            .commit_from_file(model)
            .map_err(|e| e.to_string())?;
        let tok =
            WordPiece::from_vocab(&std::fs::read_to_string(vocab).map_err(|e| e.to_string())?);
        Ok(Self {
            session: Mutex::new(session),
            tok,
        })
    }

    /// A unit-length 384-number vector for `text`.
    pub fn embed(&self, text: &str) -> Result<Vec<f32>, String> {
        let ids = self.tok.encode(text);
        let n = ids.len();
        let err = |e: ort::Error| e.to_string();
        let mut s = self.session.lock().unwrap_or_else(|e| e.into_inner());
        let out = s
            .run(ort::inputs![
                "input_ids" => Tensor::from_array(([1usize, n], ids)).map_err(err)?,
                "attention_mask" => Tensor::from_array(([1usize, n], vec![1i64; n])).map_err(err)?,
                "token_type_ids" => Tensor::from_array(([1usize, n], vec![0i64; n])).map_err(err)?,
            ])
            .map_err(err)?;
        let (shape, hidden) = out["last_hidden_state"]
            .try_extract_tensor::<f32>()
            .map_err(err)?;
        let dim = *shape.last().ok_or("no output")? as usize;
        let mut v = vec![0f32; dim];
        for t in 0..n {
            for (d, x) in v.iter_mut().enumerate() {
                *x += hidden[t * dim + d];
            }
        }
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
        // Mean pooling then unit length: dividing the sum by its own norm does both.
        Ok(v.into_iter().map(|x| x / norm).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wordpiece_splits_like_bert() {
        let vocab = "[PAD]\n[UNK]\n[CLS]\n[SEP]\nhello\n,\nmark\n##on\nworld\n!\ncafe";
        let t = WordPiece::from_vocab(vocab);
        assert_eq!(t.encode("Hello, Markon world!"), [2, 4, 5, 6, 7, 8, 9, 3]);
        assert_eq!(t.encode("Café zzz"), [2, 10, 1, 3]);
    }

    /// With the real files: `MARKON_EMBED_DIR=<dir with minilm-int8.onnx and minilm-vocab.txt>
    /// cargo test -p markon-llm -- --ignored`.
    #[test]
    #[ignore]
    fn related_sentences_are_closer_than_unrelated_ones() {
        let d =
            std::path::PathBuf::from(std::env::var("MARKON_EMBED_DIR").expect("MARKON_EMBED_DIR"));
        let e = Embedder::load(&d.join("minilm-int8.onnx"), &d.join("minilm-vocab.txt")).unwrap();
        let dot = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();
        let q = e.embed("When is the dentist appointment?").unwrap();
        let near = e
            .embed("Reminder: dental check-up on Thursday at 9")
            .unwrap();
        let far = e
            .embed("The quarterly budget spreadsheet has three tabs")
            .unwrap();
        assert_eq!(q.len(), 384);
        assert!((dot(&q, &q) - 1.0).abs() < 1e-3);
        assert!(
            dot(&q, &near) > dot(&q, &far) + 0.1,
            "{} vs {}",
            dot(&q, &near),
            dot(&q, &far)
        );
    }
}
