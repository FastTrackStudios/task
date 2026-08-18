//! Vault mutations — create / save / append / delete pages.
//!
//! Each operation writes through to disk atomically (write to a
//! temp file in the same directory, then rename) so a crash
//! mid-write leaves the previous file intact.

use std::path::Path;

use thiserror::Error;

use crate::vault::{Vault, VaultPage};

#[derive(Debug, Error)]
pub enum MutateError {
    #[error("io: {0}")]
    Io(String),
    #[error("page already exists: {0}")]
    AlreadyExists(String),
    #[error("page not found: {0}")]
    NotFound(String),
}

/// Write `page.raw` to disk under `vault.root`. Updates the
/// in-memory `mtime` to match the new file metadata.
pub fn save_page(vault: &mut Vault, page_rel_path: &str) -> Result<(), MutateError> {
    let idx = vault
        .pages
        .iter()
        .position(|p| p.rel_path == page_rel_path)
        .ok_or_else(|| MutateError::NotFound(page_rel_path.to_string()))?;
    let abs = vault.root.join(page_rel_path);
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| MutateError::Io(format!("mkdir {}: {e}", parent.display())))?;
    }
    let body = vault.pages[idx].raw.clone();
    write_atomic(&abs, body.as_bytes()).map_err(MutateError::Io)?;
    let mtime = std::fs::metadata(&abs)
        .and_then(|m| m.modified())
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
    vault.pages[idx].mtime = mtime;
    Ok(())
}

/// Create a new page at `rel_path` with `body` as the initial
/// content. Returns the index in `vault.pages`. Errors when a
/// page at that path is already loaded.
pub fn create_page(
    vault: &mut Vault,
    rel_path: &str,
    body: impl Into<String>,
) -> Result<usize, MutateError> {
    if vault.pages.iter().any(|p| p.rel_path == rel_path) {
        return Err(MutateError::AlreadyExists(rel_path.to_string()));
    }
    let path = std::path::PathBuf::from(rel_path);
    let basename = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(rel_path)
        .to_string();
    let folder = path
        .parent()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();
    vault.pages.push(VaultPage {
        rel_path: rel_path.to_string(),
        basename,
        folder,
        raw: body.into(),
        mtime: std::time::SystemTime::now(),
    });
    save_page(vault, rel_path)?;
    Ok(vault.pages.len() - 1)
}

/// Append `text` to the end of an existing page. Adds a leading
/// newline if the existing content doesn't end with one.
pub fn append_to_page(vault: &mut Vault, rel_path: &str, text: &str) -> Result<(), MutateError> {
    let idx = vault
        .pages
        .iter()
        .position(|p| p.rel_path == rel_path)
        .ok_or_else(|| MutateError::NotFound(rel_path.to_string()))?;
    let page = &mut vault.pages[idx];
    if !page.raw.is_empty() && !page.raw.ends_with('\n') {
        page.raw.push('\n');
    }
    page.raw.push_str(text);
    save_page(vault, rel_path)
}

/// Remove a page from the vault + disk.
pub fn delete_page(vault: &mut Vault, rel_path: &str) -> Result<(), MutateError> {
    let idx = vault
        .pages
        .iter()
        .position(|p| p.rel_path == rel_path)
        .ok_or_else(|| MutateError::NotFound(rel_path.to_string()))?;
    let abs = vault.root.join(rel_path);
    if abs.exists() {
        std::fs::remove_file(&abs)
            .map_err(|e| MutateError::Io(format!("rm {}: {e}", abs.display())))?;
    }
    vault.pages.remove(idx);
    Ok(())
}

/// Write `bytes` to `path` atomically — write to a sibling temp
/// file, then rename over the target. Survives partial writes.
// t[impl vault.write.atomic] — temp file in the same directory, then
// rename. The same-directory part is load-bearing: a rename across
// filesystems is a copy, and a copy is exactly the torn write this
// avoids
// t[impl project.vault.write-path] — the single choke point every vault
// page passes through. The rule wants this to be the Files API rather
// than `std::fs`, which it is not yet; what makes that a tractable
// migration is that this is one function and not eighty call sites
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "path has no parent".to_string())?;
    let mut tmp = parent.join(
        path.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("tmp")
            .to_string()
            + ".tmp",
    );
    // If a stale tmp file lingers from a crash, drop it.
    if tmp.exists() {
        let _ = std::fs::remove_file(&tmp);
    }
    // PID + nanos suffix so concurrent writes don't collide.
    let suffix = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    tmp.set_extension(format!("tmp.{suffix}"));
    std::fs::write(&tmp, bytes).map_err(|e| format!("write tmp: {e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("rename: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fresh_vault() -> (tempfile::TempDir, Vault) {
        let tmp = tempfile::tempdir().unwrap();
        let v = Vault {
            bases: Vec::new(),
            property_types: Default::default(),
            root: tmp.path().to_path_buf(),
            pages: Vec::new(),
        };
        (tmp, v)
    }

    #[test]
    fn create_writes_to_disk() {
        let (tmp, mut v) = fresh_vault();
        create_page(&mut v, "notes/hello.md", "hi\n").unwrap();
        let read = std::fs::read_to_string(tmp.path().join("notes/hello.md")).unwrap();
        assert_eq!(read, "hi\n");
        assert_eq!(v.pages.len(), 1);
    }

    #[test]
    fn create_refuses_duplicate() {
        let (_tmp, mut v) = fresh_vault();
        create_page(&mut v, "p.md", "1").unwrap();
        assert!(matches!(
            create_page(&mut v, "p.md", "2"),
            Err(MutateError::AlreadyExists(_))
        ));
    }

    #[test]
    fn append_adds_newline_when_needed() {
        let (_tmp, mut v) = fresh_vault();
        create_page(&mut v, "p.md", "first").unwrap();
        append_to_page(&mut v, "p.md", "second").unwrap();
        assert_eq!(v.pages[0].raw, "first\nsecond");
    }

    #[test]
    fn save_round_trips() {
        let (tmp, mut v) = fresh_vault();
        create_page(&mut v, "x.md", "original").unwrap();
        v.pages[0].raw = "updated".into();
        save_page(&mut v, "x.md").unwrap();
        let read = std::fs::read_to_string(tmp.path().join("x.md")).unwrap();
        assert_eq!(read, "updated");
    }

    #[test]
    fn delete_removes_file_and_entry() {
        let (tmp, mut v) = fresh_vault();
        create_page(&mut v, "p.md", "data").unwrap();
        let path: PathBuf = tmp.path().join("p.md");
        assert!(path.exists());
        delete_page(&mut v, "p.md").unwrap();
        assert!(!path.exists());
        assert!(v.pages.is_empty());
    }
}
