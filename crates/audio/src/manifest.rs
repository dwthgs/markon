//! models/manifest.json: every model and voice file pinned by size and SHA-256.
//! A file that does not match is never used. There is no fallback voice.
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub files: Vec<Entry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Entry {
    pub id: String,
    pub path: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
    pub license: String,
    /// Downloaded only when the user asks for it (the built-in language model), so it is
    /// not part of the start-up check.
    #[serde(default)]
    pub on_demand: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("{id} is missing ({path})")]
    Missing { id: String, path: PathBuf },
    #[error("{id} does not match its pinned checksum; refusing to use it")]
    Mismatch { id: String, path: PathBuf },
    #[error("unknown file id {0}")]
    Unknown(String),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("download failed: {0}")]
    Download(String),
    #[error("bad manifest: {0}")]
    Parse(#[from] serde_json::Error),
}

/// The manifest shipped with the app (compiled in, so it cannot be swapped next to the models).
pub const BUNDLED: &str = include_str!("../../../models/manifest.json");

impl Manifest {
    pub fn bundled() -> Self {
        serde_json::from_str(BUNDLED).expect("bundled manifest is valid")
    }

    pub fn entry(&self, id: &str) -> Result<&Entry, ManifestError> {
        self.files
            .iter()
            .find(|e| e.id == id)
            .ok_or_else(|| ManifestError::Unknown(id.into()))
    }

    /// Check every file needed at start. Returns the first problem found.
    pub fn verify(&self, dir: &Path) -> Result<(), ManifestError> {
        self.files
            .iter()
            .filter(|e| !e.on_demand)
            .try_for_each(|e| e.verify(dir).map(|_| ()))
    }

    /// Entries needed at start that are missing or wrong and need downloading.
    pub fn missing(&self, dir: &Path) -> Vec<&Entry> {
        self.files
            .iter()
            .filter(|e| !e.on_demand && e.verify(dir).is_err())
            .collect()
    }
}

pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

impl Entry {
    /// Verified path of this file, or why it cannot be used.
    pub fn verify(&self, dir: &Path) -> Result<PathBuf, ManifestError> {
        let path = dir.join(&self.path);
        let meta = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(_) => {
                return Err(ManifestError::Missing {
                    id: self.id.clone(),
                    path,
                });
            }
        };
        if meta.len() != self.size || sha256_file(&path)? != self.sha256 {
            return Err(ManifestError::Mismatch {
                id: self.id.clone(),
                path,
            });
        }
        Ok(path)
    }

    /// Download to `dir`, checking size and hash before the file is moved into place.
    pub async fn download(
        &self,
        dir: &Path,
        mut progress: impl FnMut(u64, u64),
    ) -> Result<PathBuf, ManifestError> {
        use tokio::io::AsyncWriteExt;
        let path = dir.join(&self.path);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let part = path.with_extension("part");
        let mut resp = reqwest::get(&self.url)
            .await
            .map_err(|e| ManifestError::Download(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(ManifestError::Download(format!(
                "{} returned {}",
                self.id,
                resp.status()
            )));
        }
        let mut file = tokio::fs::File::create(&part).await?;
        let mut h = Sha256::new();
        let mut got = 0u64;
        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|e| ManifestError::Download(e.to_string()))?
        {
            got += chunk.len() as u64;
            if got > self.size {
                break;
            }
            h.update(&chunk);
            file.write_all(&chunk).await?;
            progress(got, self.size);
        }
        file.flush().await?;
        drop(file);
        if got != self.size || hex(&h.finalize()) != self.sha256 {
            let _ = tokio::fs::remove_file(&part).await;
            return Err(ManifestError::Mismatch {
                id: self.id.clone(),
                path,
            });
        }
        tokio::fs::rename(&part, &path).await?;
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn on_demand_files_are_not_needed_at_start() {
        let m = Manifest::bundled();
        let dir = std::path::Path::new("/nonexistent-markon-models");
        let llm = m.entry("qwen2.5-1.5b-instruct").unwrap();
        assert!(llm.on_demand);
        assert!(m.missing(dir).iter().all(|e| !e.on_demand));
        assert!(!m.missing(dir).is_empty(), "the voice files still are");
    }
}
