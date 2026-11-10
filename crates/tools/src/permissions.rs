//! Permission tiers: read inside approved folders, write asks once per folder,
//! execute asks every time and shows the exact command. Denials are remembered.
//! Send asks every time with the exact text; "always yes" per recipient is remembered
//! only when the user picks it, and never applies once the conversation has read outside content.
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
    /// Reaches another person or changes an account: send, reply, archive, add an event.
    Send,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Request {
    pub tier: Tier,
    pub tool: String,
    /// The exact path, command or message the user is asked about.
    pub detail: String,
    /// For a send: who it goes to, offered as "always yes for <pattern>" when `always_ok`.
    pub pattern: Option<String>,
    pub always_ok: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Allow,
    Deny,
    /// Allow, and stop asking for this tool and recipient (sends only).
    Always,
}

/// Remembered "always yes" answers (the memory database in the app).
pub trait ApprovalStore: Send + Sync {
    fn allowed(&self, tool: &str, pattern: &str) -> bool;
    fn allow_always(&self, tool: &str, pattern: &str);
}

#[derive(Default)]
pub struct MemoryApprovals(pub Mutex<HashSet<(String, String)>>);
impl ApprovalStore for MemoryApprovals {
    fn allowed(&self, tool: &str, pattern: &str) -> bool {
        self.0
            .lock()
            .expect("approvals")
            .contains(&(tool.to_string(), pattern.to_lowercase()))
    }
    fn allow_always(&self, tool: &str, pattern: &str) {
        self.0
            .lock()
            .expect("approvals")
            .insert((tool.to_string(), pattern.to_lowercase()));
    }
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
    pub approvals: RwLock<Arc<dyn ApprovalStore>>,
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
            approvals: RwLock::new(Arc::new(MemoryApprovals::default())),
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
                pattern: None,
                always_ok: false,
            })
            .await;
        let allowed = d != Decision::Deny;
        self.log(Tier::Write, tool, &p.to_string_lossy(), allowed, "asked");
        if allowed {
            self.write_ok.lock().expect("write_ok").insert(folder);
            Ok(p)
        } else {
            self.denied.lock().expect("denied").insert(key);
            Err(format!("writing in {} was denied", folder.display()))
        }
    }

    /// A send: ask every time with the exact text, unless the user said "always yes" for this tool and
    /// recipient and the conversation has read nothing from outside. `always` is false for actions that
    /// may never be remembered (archive, calendar changes).
    pub async fn send(
        &self,
        tool: &str,
        to: &str,
        detail: &str,
        tainted: bool,
        always: bool,
    ) -> Result<(), String> {
        let store = self.approvals.read().expect("approvals").clone();
        if always && !tainted && store.allowed(tool, to) {
            self.log(
                Tier::Send,
                tool,
                detail,
                true,
                "always yes for this recipient",
            );
            return Ok(());
        }
        let d = self
            .prompter
            .ask(Request {
                tier: Tier::Send,
                tool: tool.into(),
                detail: detail.into(),
                pattern: Some(to.to_string()),
                always_ok: always && !tainted,
            })
            .await;
        let reason = if tainted {
            "asked (outside content in this conversation)"
        } else {
            "asked"
        };
        self.log(Tier::Send, tool, detail, d != Decision::Deny, reason);
        match d {
            Decision::Deny => Err("the user said no, so nothing was sent".into()),
            Decision::Always if always && !tainted => {
                store.allow_always(tool, to);
                Ok(())
            }
            _ => Ok(()),
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
                pattern: None,
                always_ok: false,
            })
            .await;
        let allowed = d != Decision::Deny;
        self.log(Tier::Execute, tool, command, allowed, "asked");
        if allowed {
            Ok(())
        } else {
            self.denied.lock().expect("denied").insert(key);
            Err("the user did not allow this command".into())
        }
    }
}
