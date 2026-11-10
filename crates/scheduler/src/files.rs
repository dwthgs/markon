//! Agents as files: one TOML file per agent in
//! `~/.markon/agents`, read on every scheduler tick, so a new file works with no rebuild.
use crate::AgentSpec;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One agent file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentFile {
    pub name: String,
    pub prompt: String,
    /// The only tools this agent may call. Empty means none.
    #[serde(default)]
    pub tools: Vec<String>,
    /// Tool rounds before it must answer.
    #[serde(default)]
    pub max_rounds: Option<usize>,
    /// A cron line ("30 7 * * *"). Without one the agent runs only when something starts it.
    #[serde(default)]
    pub schedule: Option<String>,
    /// Off by default for a file the user has not switched on in settings.
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

impl AgentFile {
    pub fn spec(&self) -> AgentSpec {
        AgentSpec {
            prompt: self.prompt.clone(),
            tools: self.tools.clone(),
            max_rounds: self.max_rounds,
        }
    }
}

pub fn parse(text: &str) -> Result<AgentFile, String> {
    let f: AgentFile = toml::from_str(text).map_err(|e| e.to_string())?;
    if f.name.trim().is_empty() || f.prompt.trim().is_empty() {
        return Err("an agent needs a name and a prompt".into());
    }
    if let Some(c) = &f.schedule {
        crate::next_after(c, chrono::Local::now())?;
    }
    if f.max_rounds.is_some_and(|n| n == 0 || n > 30) {
        return Err("max_rounds must be between 1 and 30".into());
    }
    Ok(f)
}

/// Every `*.toml` in `dir`, sorted by file name, with the files that did not parse.
pub fn load_dir(dir: &Path) -> (Vec<(PathBuf, AgentFile)>, Vec<String>) {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "toml")).collect())
        .unwrap_or_default();
    paths.sort();
    let (mut ok, mut bad) = (Vec::new(), Vec::new());
    for p in paths {
        match std::fs::read_to_string(&p).map_err(|e| e.to_string()).and_then(|t| parse(&t)) {
            Ok(f) => ok.push((p, f)),
            Err(e) => bad.push(format!("{}: {e}", p.display())),
        }
    }
    (ok, bad)
}

/// The bundled agents, written to `dir` once (a file the user deleted on purpose is not brought back:
/// the marker remembers what was written).
pub fn install_bundled(dir: &Path, bundled: &[(&str, &str)]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let marker = dir.join(".bundled");
    let done = std::fs::read_to_string(&marker).unwrap_or_default();
    let mut written: Vec<String> = done.lines().map(String::from).collect();
    for (file, text) in bundled {
        if written.iter().any(|w| w == file) {
            continue;
        }
        let p = dir.join(file);
        if !p.exists() {
            std::fs::write(&p, text)?;
        }
        written.push((*file).to_string());
    }
    std::fs::write(marker, written.join("\n"))
}
