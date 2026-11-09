//! Permission tiers: read inside approved folders, write asks once per folder,
//! execute asks every time and shows the exact command. Denials are remembered.
use crate::BoxFuture;
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Read,
    Write,
    Execute,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Request {
    pub tier: Tier,
    pub tool: String,
    /// The exact path or command the user is asked about.
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Allow,
    Deny,
}

/// Shows the question to the user (a dialog on the HUD) and returns their answer.
pub trait Prompter: Send + Sync {
    fn ask(&self, req: Request) -> BoxFuture<'static, Decision>;
}

#[derive(Debug, Clone, Serialize)]
pub struct AuditEntry {
    pub at_ms: u64,
    pub tier: Tier,
    pub tool: String,
    pub detail: String,
    pub allowed: bool,
    pub reason: String,
}

pub struct Permissions {
    approved: RwLock<Vec<PathBuf>>,
    write_ok: Mutex<HashSet<PathBuf>>,
    denied: Mutex<HashSet<(Tier, String)>>,
    prompter: Arc<dyn Prompter>,
    audit: Mutex<Vec<AuditEntry>>,
}

/// Lexically resolve `.` and `..` without touching the disk, so a missing file still resolves.
fn normalise(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// Resolve symlinks for the deepest existing ancestor, then append the rest.
fn resolve(p: &Path) -> PathBuf {
    let p = normalise(p);
    let mut existing = p.clone();
    let mut rest = Vec::new();
    while !existing.exists() {
        match (
            existing.file_name().map(|s| s.to_owned()),
            existing.parent(),
        ) {
            (Some(name), Some(parent)) => {
                rest.push(name);
                existing = parent.to_path_buf();
            }
            _ => break,
        }
    }
    let mut out = existing.canonicalize().unwrap_or(existing);
    for name in rest.into_iter().rev() {
        out.push(name);
    }
    out
}

impl Permissions {
    pub fn new(approved: Vec<PathBuf>, prompter: Arc<dyn Prompter>) -> Self {
        Self {
            approved: RwLock::new(approved.iter().map(|p| resolve(p)).collect()),
            write_ok: Mutex::new(HashSet::new()),
            denied: Mutex::new(HashSet::new()),
            prompter,
            audit: Mutex::new(Vec::new()),
        }
    }

    pub fn set_approved(&self, folders: Vec<PathBuf>) {
        *self.approved.write().expect("approved") = folders.iter().map(|p| resolve(p)).collect();
    }

    pub fn audit(&self) -> Vec<AuditEntry> {
        self.audit.lock().expect("audit").clone()
    }

    fn log(&self, tier: Tier, tool: &str, detail: &str, allowed: bool, reason: &str) {
        self.audit.lock().expect("audit").push(AuditEntry {
            at_ms: crate::cards::now_ms(),
            tier,
            tool: tool.into(),
            detail: detail.into(),
            allowed,
            reason: reason.into(),
        });
    }

    /// The approved folder that contains `path`, if any.
    fn folder_of(&self, path: &Path) -> Option<PathBuf> {
        self.approved
            .read()
            .expect("approved")
            .iter()
            .find(|f| path.starts_with(f))
            .cloned()
    }

    pub async fn read(&self, tool: &str, path: &Path) -> Result<PathBuf, String> {
        let p = resolve(path);
        if self.folder_of(&p).is_none() {
            let reason = format!("{} is outside the folders Markon may use", p.display());
            self.log(Tier::Read, tool, &p.to_string_lossy(), false, &reason);
            return Err(reason);
        }
        self.log(
            Tier::Read,
            tool,
            &p.to_string_lossy(),
            true,
            "approved folder",
        );
        Ok(p)
    }

    pub async fn write(&self, tool: &str, path: &Path) -> Result<PathBuf, String> {
        let p = resolve(path);
        let Some(folder) = self.folder_of(&p) else {
            let reason = format!("{} is outside the folders Markon may use", p.display());
            self.log(Tier::Write, tool, &p.to_string_lossy(), false, &reason);
            return Err(reason);
        };
        if self.write_ok.lock().expect("write_ok").contains(&folder) {
            self.log(
                Tier::Write,
                tool,
                &p.to_string_lossy(),
                true,
                "folder already allowed",
            );
            return Ok(p);
        }
        let key = (Tier::Write, folder.to_string_lossy().to_string());
        if self.denied.lock().expect("denied").contains(&key) {
            self.log(
                Tier::Write,
                tool,
                &p.to_string_lossy(),
                false,
                "denied before",
            );
            return Err(format!("writing in {} was denied", folder.display()));
        }
        let d = self
            .prompter
            .ask(Request {
                tier: Tier::Write,
                tool: tool.into(),
                detail: folder.to_string_lossy().into(),
            })
            .await;
        let allowed = d == Decision::Allow;
        self.log(Tier::Write, tool, &p.to_string_lossy(), allowed, "asked");
        if allowed {
            self.write_ok.lock().expect("write_ok").insert(folder);
            Ok(p)
        } else {
            self.denied.lock().expect("denied").insert(key);
            Err(format!("writing in {} was denied", folder.display()))
        }
    }

    /// Ask every time, showing the exact command. A command denied once is refused again without asking.
    pub async fn execute(&self, tool: &str, command: &str) -> Result<(), String> {
        let key = (Tier::Execute, command.to_string());
        if self.denied.lock().expect("denied").contains(&key) {
            self.log(Tier::Execute, tool, command, false, "denied before");
            return Err("this command was denied".into());
        }
        let d = self
            .prompter
            .ask(Request {
                tier: Tier::Execute,
                tool: tool.into(),
                detail: command.into(),
            })
            .await;
        let allowed = d == Decision::Allow;
        self.log(Tier::Execute, tool, command, allowed, "asked");
        if allowed {
            Ok(())
        } else {
            self.denied.lock().expect("denied").insert(key);
            Err("the user did not allow this command".into())
        }
    }
}
