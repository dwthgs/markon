//! Secret scrubbing for tool output. Runs on every tool
//! result before it reaches the model or the screen's event stream.
use regex::Regex;
use std::sync::LazyLock;

static SECRETS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"sk-ant-[A-Za-z0-9_-]{20,}",
        r"sk-or-v1-[A-Za-z0-9]{20,}",
        r"\bsk-[A-Za-z0-9_-]{20,}",
        r"\bAKIA[0-9A-Z]{16}\b",
        r"\b(?:ghp|gho|ghs|ghr|ghu|github_pat)_[A-Za-z0-9_]{30,}",
        r"\bxox[abposr]-[A-Za-z0-9-]{10,}",
        r"\b(?:sk|pk|rk)_(?:test|live)_[A-Za-z0-9]{20,}",
        r"\bAIza[0-9A-Za-z_-]{35}\b",
        r"\bglpat-[A-Za-z0-9_-]{20,}",
        r"\beyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}",
        r"(?i)\b(?:postgres|postgresql|mysql|mongodb(?:\+srv)?|redis|amqp)://[^\s:/]+:[^\s@]+@[^\s]+",
        r"(?is)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?(?:-----END [A-Z ]*PRIVATE KEY-----|\z)",
        r#"(?i)\b(?:password|passwd|pwd|api_key|apikey|secret_key|auth_token|access_token|client_secret)\s*[=:]\s*['"]?[^\s'"]{6,}"#,
    ]
    .iter()
    .map(|p| Regex::new(p).expect("secret pattern"))
    .collect()
});

/// Replace anything that looks like a key, token, password or private key with `[secret removed]`.
pub fn scrub(text: &str) -> String {
    let mut out = std::borrow::Cow::Borrowed(text);
    for re in SECRETS.iter() {
        if re.is_match(&out) {
            out = std::borrow::Cow::Owned(re.replace_all(&out, "[secret removed]").into_owned());
        }
    }
    out.into_owned()
}

#[cfg(test)]
mod tests {
    use super::scrub;

    #[test]
    fn keys_and_tokens_are_removed() {
        let s = scrub(
            "anthropic sk-ant-api03-abcdefghijklmnopqrstuv and ghp_abcdefghijklmnopqrstuvwxyz0123456789 \
             slack xoxb-1234567890-abcdef aws AKIAABCDEFGHIJKLMNOP db postgres://me:hunter22@db.example/x \
             PASSWORD=\"correct-horse\" -----BEGIN RSA PRIVATE KEY-----\nMIIE\n-----END RSA PRIVATE KEY----- end",
        );
        for leaked in [
            "sk-ant-api03",
            "ghp_",
            "xoxb-",
            "AKIA",
            "hunter22",
            "correct-horse",
            "MIIE",
        ] {
            assert!(!s.contains(leaked), "{leaked} leaked: {s}");
        }
        assert!(s.contains("anthropic") && s.ends_with("end"));
    }

    #[test]
    fn ordinary_text_is_untouched() {
        let t = "The meeting moved to 3pm. Ask Sara about the risk-assessment doc, sk is short for sketch.";
        assert_eq!(scrub(t), t);
    }
}
