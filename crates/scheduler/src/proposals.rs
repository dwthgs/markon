//! Gets better with use: improved prompts and skill descriptions are written beside the
//! originals as `<file>.proposed`, shown as a diff, and replace the original only when the user
//! approves.
use markon_provider::{ChatDelta, ChatRequest, Message, ModelProvider};
use serde::Serialize;
use std::path::{Path, PathBuf};

pub const SUFFIX: &str = "proposed";

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Proposal {
    pub path: PathBuf,
    pub diff: String,
}

pub fn proposed_path(original: &Path) -> PathBuf {
    let mut p = original.as_os_str().to_owned();
    p.push(format!(".{SUFFIX}"));
    PathBuf::from(p)
}

/// A line diff: unchanged lines with two spaces, removed with "- ", added with "+ ".
pub fn diff(old: &str, new: &str) -> String {
    let (a, b): (Vec<&str>, Vec<&str>) = (old.lines().collect(), new.lines().collect());
    let (n, m) = (a.len(), b.len());
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let (mut i, mut j, mut out) = (0, 0, Vec::new());
    while i < n || j < m {
        if i < n && j < m && a[i] == b[j] {
            out.push(format!("  {}", a[i]));
            i += 1;
            j += 1;
        } else if i < n && (j == m || lcs[i + 1][j] >= lcs[i][j + 1]) {
            out.push(format!("- {}", a[i]));
            i += 1;
        } else {
            out.push(format!("+ {}", b[j]));
            j += 1;
        }
    }
    out.join("\n")
}

/// Every pending proposal under the given folders (one level of skill folders included).
pub fn list(dirs: &[PathBuf]) -> Vec<Proposal> {
    let mut out = Vec::new();
    let mut look = |dir: &Path| {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == SUFFIX) {
                let original = p.with_extension("");
                let old = std::fs::read_to_string(&original).unwrap_or_default();
                let new = std::fs::read_to_string(&p).unwrap_or_default();
                out.push(Proposal {
                    path: original,
                    diff: diff(&old, &new),
                });
            }
        }
    };
    for d in dirs {
        look(d);
        for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
            if e.path().is_dir() {
                look(&e.path());
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// The user said yes: the proposal replaces the original, which is kept as `<file>.before`.
pub fn approve(original: &Path) -> Result<(), String> {
    let p = proposed_path(original);
    if !p.exists() {
        return Err("there is no proposal for that file".into());
    }
    let mut before = original.as_os_str().to_owned();
    before.push(".before");
    if original.exists() {
        std::fs::copy(original, PathBuf::from(before)).map_err(|e| e.to_string())?;
    }
    std::fs::rename(&p, original).map_err(|e| e.to_string())
}

pub fn reject(original: &Path) -> Result<(), String> {
    std::fs::remove_file(proposed_path(original)).map_err(|e| e.to_string())
}

const ASK: &str = "You improve the instructions of a voice assistant's agent or skill file. You get the file \
    and notes from recent runs that used it (slow turns, failed tools, user corrections). Return the whole \
    file again with small, specific improvements to its wording: clearer steps, a better description of \
    when to use it, fixes for the problems in the notes. Keep its format, its name and every field the \
    same. Do not add tools. Return only the file, nothing before or after it.";

/// Ask `model` for an improved version of `original` given `notes`, and write it beside it. Nothing
/// changes until the user approves. Returns the diff, or None when the model proposed no change.
pub async fn propose(
    provider: &dyn ModelProvider,
    model: &str,
    original: &Path,
    notes: &str,
) -> Result<Option<String>, String> {
    let old = std::fs::read_to_string(original).map_err(|e| e.to_string())?;
    let mut rx = provider.stream(ChatRequest {
        model: model.into(),
        system: ASK.into(),
        messages: vec![Message::user_text(&format!(
            "<file>\n{old}\n</file>\n<notes>\n{notes}\n</notes>"
        ))],
        tools: vec![],
        max_tokens: 4000,
    });
    let mut new = String::new();
    while let Some(d) = rx.recv().await {
        match d {
            Ok(ChatDelta::Text(t)) => new.push_str(&t),
            Ok(_) => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    let new = new
        .trim()
        .trim_start_matches("<file>")
        .trim_end_matches("</file>")
        .trim()
        .to_string()
        + "\n";
    if new.trim() == old.trim() || new.trim().is_empty() {
        return Ok(None);
    }
    // A broken agent file is never proposed.
    if original.extension().is_some_and(|x| x == "toml") {
        crate::files::parse(&new)
            .map_err(|e| format!("the model's version does not parse: {e}"))?;
    }
    std::fs::write(proposed_path(original), &new).map_err(|e| e.to_string())?;
    Ok(Some(diff(&old, &new)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_diff_shows_what_changed() {
        assert_eq!(diff("a\nb\nc", "a\nB\nc\nd"), "  a\n- b\n+ B\n  c\n+ d");
        assert_eq!(diff("same", "same"), "  same");
    }
}
