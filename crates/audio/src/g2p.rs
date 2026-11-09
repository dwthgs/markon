//! Text to Kokoro phonemes without espeak-ng: misaki's Apache-2.0 British dictionaries,
//! simple suffix rules, number words, and letter-by-letter spelling for anything still unknown.
use std::collections::HashMap;
use std::path::Path;

pub struct G2p {
    dict: HashMap<String, String>,
}

/// Kokoro's phoneme vocabulary (from the model's tokenizer.json, Apache-2.0).
const VOCAB: &str = include_str!("../data/kokoro-vocab.json");

/// Map phonemes to Kokoro token ids, padded with 0 at both ends. Unknown symbols are skipped.
pub fn tokenize(phonemes: &str) -> Vec<i64> {
    thread_local! {
        static V: HashMap<char, i64> = serde_json::from_str::<HashMap<String, i64>>(VOCAB)
            .expect("vocab")
            .into_iter()
            .filter_map(|(k, v)| k.chars().next().map(|c| (c, v)))
            .collect();
    }
    V.with(|v| {
        let mut ids = vec![0];
        ids.extend(phonemes.chars().filter_map(|c| v.get(&c).copied()));
        ids.push(0);
        ids
    })
}

fn parse(json: &str) -> Result<HashMap<String, String>, serde_json::Error> {
    let raw: HashMap<String, serde_json::Value> = serde_json::from_str(json)?;
    Ok(raw
        .into_iter()
        .filter_map(|(k, v)| match v {
            serde_json::Value::String(s) => Some((k, s)),
            serde_json::Value::Object(o) => o
                .get("DEFAULT")
                .and_then(|d| d.as_str())
                .map(|s| (k, s.to_string())),
            _ => None,
        })
        .collect())
}

const ONES: [&str; 20] = [
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
];
const TENS: [&str; 10] = [
    "", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
];

/// English words for a whole number, British style ("one hundred and five").
pub fn number_words(n: u64) -> String {
    fn under_1000(n: u64, out: &mut Vec<String>) {
        let (h, r) = (n / 100, n % 100);
        if h > 0 {
            out.push(ONES[h as usize].into());
            out.push("hundred".into());
            if r > 0 {
                out.push("and".into());
            }
        }
        if r >= 20 {
            out.push(TENS[(r / 10) as usize].into());
            if r % 10 > 0 {
                out.push(ONES[(r % 10) as usize].into());
            }
        } else if r > 0 || h == 0 {
            out.push(ONES[r as usize].into());
        }
    }
    if n == 0 {
        return "zero".into();
    }
    let mut out = Vec::new();
    for (scale, name) in [
        (1_000_000_000_000, "trillion"),
        (1_000_000_000, "billion"),
        (1_000_000, "million"),
        (1000, "thousand"),
    ] {
        if n % (scale * 1000) / scale > 0 || (scale == 1_000_000_000_000 && n >= scale) {
            under_1000(n / scale % 1000, &mut out);
            out.push(name.into());
        }
    }
    if !n.is_multiple_of(1000) {
        if n >= 1000 && n % 1000 < 100 {
            out.push("and".into());
        }
        under_1000(n % 1000, &mut out);
    }
    out.join(" ")
}

/// The product names are not in misaki's dictionaries; without these they would be spelled out.
const NAMES: &[(&str, &str)] = &[("Markon", "mˈɑːkɒn"), ("Markan", "mˈɑːkən")];

impl G2p {
    pub fn load(gold: &Path, silver: &Path) -> std::io::Result<Self> {
        let mut dict = parse(&std::fs::read_to_string(silver)?).map_err(std::io::Error::other)?;
        dict.extend(parse(&std::fs::read_to_string(gold)?).map_err(std::io::Error::other)?);
        dict.extend(NAMES.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        Ok(Self { dict })
    }

    pub fn from_entries(entries: &[(&str, &str)]) -> Self {
        Self {
            dict: NAMES
                .iter()
                .chain(entries)
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    /// Expand numbers and symbols into words.
    pub fn normalise(text: &str) -> String {
        let mut out = String::new();
        let chars: Vec<char> = text.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            if c.is_ascii_digit() {
                let start = i;
                while i < chars.len()
                    && (chars[i].is_ascii_digit()
                        || (chars[i] == ','
                            && i + 1 < chars.len()
                            && chars[i + 1].is_ascii_digit()))
                {
                    i += 1;
                }
                let whole: String = chars[start..i].iter().filter(|c| **c != ',').collect();
                out.push_str(&whole.parse::<u64>().map(number_words).unwrap_or(whole));
                if i + 1 < chars.len() && chars[i] == '.' && chars[i + 1].is_ascii_digit() {
                    out.push_str(" point");
                    i += 1;
                    while i < chars.len() && chars[i].is_ascii_digit() {
                        out.push(' ');
                        out.push_str(ONES[chars[i].to_digit(10).unwrap_or(0) as usize]);
                        i += 1;
                    }
                }
                if i < chars.len() && chars[i] == '%' {
                    out.push_str(" percent");
                    i += 1;
                }
                continue;
            }
            match c {
                '&' => out.push_str(" and "),
                '+' => out.push_str(" plus "),
                '@' => out.push_str(" at "),
                '–' | '-' if i > 0 && chars[i - 1] == ' ' => out.push('—'),
                _ => out.push(c),
            }
            i += 1;
        }
        out
    }

    fn lookup(&self, w: &str) -> Option<String> {
        let lower = w.to_lowercase();
        let mut cap = lower.clone();
        if let Some(f) = cap.get_mut(0..1) {
            f.make_ascii_uppercase();
        }
        for k in [w, lower.as_str(), cap.as_str()] {
            if let Some(p) = self.dict.get(k) {
                return Some(p.clone());
            }
        }
        None
    }

    fn word(&self, w: &str) -> String {
        if let Some(p) = self.lookup(w) {
            return p;
        }
        let lower = w.to_lowercase();
        // Plural and third-person -s, past -ed, and -ing, built from the stem as misaki does.
        if let Some(stem) = lower.strip_suffix("'s").or_else(|| lower.strip_suffix('s'))
            && let Some(p) = self.lookup(stem)
        {
            {
                let last = p.chars().last().unwrap_or(' ');
                let tail = if "szʃʒʧʤ".contains(last) {
                    "ɪz"
                } else if "ptkfθ".contains(last) {
                    "s"
                } else {
                    "z"
                };
                return p + tail;
            }
        }
        if let Some(stem) = lower.strip_suffix("ed") {
            for s in [stem.to_string(), format!("{stem}e")] {
                if let Some(p) = self.lookup(&s) {
                    let last = p.chars().last().unwrap_or(' ');
                    let tail = if "td".contains(last) {
                        "ɪd"
                    } else if "pkfθsʃʧ".contains(last) {
                        "t"
                    } else {
                        "d"
                    };
                    return p + tail;
                }
            }
        }
        if let Some(stem) = lower.strip_suffix("ing") {
            for s in [stem.to_string(), format!("{stem}e")] {
                if let Some(p) = self.lookup(&s) {
                    return p + "ɪŋ";
                }
            }
        }
        // Still unknown: spell it out.
        lower
            .chars()
            .filter(|c| c.is_alphanumeric())
            .filter_map(|c| self.lookup(&c.to_uppercase().to_string()))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Phonemes for a sentence, ready for `tokenize`.
    pub fn phonemize(&self, text: &str) -> String {
        let text = Self::normalise(text);
        let mut out = String::new();
        let mut word = String::new();
        let flush = |word: &mut String, out: &mut String| {
            if !word.is_empty() {
                if !out.is_empty() && !out.ends_with(' ') {
                    out.push(' ');
                }
                out.push_str(&self.word(word));
                word.clear();
            }
        };
        for c in text.chars() {
            if c.is_alphanumeric() || c == '\'' || c == '’' {
                word.push(if c == '’' { '\'' } else { c });
            } else {
                flush(&mut word, &mut out);
                match c {
                    '.' | ',' | '!' | '?' | ';' | ':' | '—' | '…' | '(' | ')' | '"' | '“' | '”' => {
                        out.push(c)
                    }
                    '-' => {}
                    _ if c.is_whitespace() => {}
                    _ => {}
                }
            }
        }
        flush(&mut word, &mut out);
        out.trim().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g() -> G2p {
        G2p::from_entries(&[
            ("hello", "həlˈQ"),
            ("world", "wˈɜːld"),
            ("meeting", "mˈiːtɪŋ"),
            ("walk", "wˈɔːk"),
            ("move", "mˈuːv"),
            ("two", "tˈuː"),
            ("point", "pˈɔɪnt"),
            ("five", "fˈIv"),
            ("X", "ˈɛks"),
            ("Q", "kjˈuː"),
        ])
    }

    #[test]
    fn looks_up_words_and_keeps_punctuation() {
        assert_eq!(g().phonemize("Hello, world!"), "həlˈQ, wˈɜːld!");
    }

    #[test]
    fn builds_suffixes_from_the_stem() {
        let g = g();
        assert_eq!(g.phonemize("meetings"), "mˈiːtɪŋz");
        assert_eq!(g.phonemize("walked"), "wˈɔːkt");
        assert_eq!(g.phonemize("moving"), "mˈuːvɪŋ");
    }

    #[test]
    fn spells_out_unknown_words_instead_of_guessing() {
        assert_eq!(g().phonemize("QX"), "kjˈuː ˈɛks");
    }

    #[test]
    fn says_its_own_names() {
        assert_eq!(g().phonemize("Markon"), "mˈɑːkɒn");
        assert_eq!(g().phonemize("markan's"), "mˈɑːkənz");
    }

    #[test]
    fn says_numbers_as_words() {
        assert_eq!(number_words(105), "one hundred and five");
        assert_eq!(number_words(2026), "two thousand and twenty six");
        assert_eq!(number_words(1_000_000), "one million");
        assert_eq!(G2p::normalise("2.5"), "two point five");
        assert_eq!(g().phonemize("2.5"), "tˈuː pˈɔɪnt fˈIv");
    }

    #[test]
    fn tokenizes_with_padding() {
        let ids = tokenize("ab");
        assert_eq!(ids.first(), Some(&0));
        assert_eq!(ids.last(), Some(&0));
        assert_eq!(ids.len(), 4);
    }
}
