//! Vault mutations — create / save / append / delete pages.
//!
//! Each operation writes through to disk atomically (write to a
//! temp file in the same directory, then rename) so a crash
//! mid-write leaves the previous file intact.
//!
//! # The write path is a port
//!
//! Every page in the system reaches disk through this module —
//! [`save_page`], [`create_page`], [`append_to_page`] and
//! [`delete_page`] are the whole surface, and every entity store is a
//! caller of them. That is what makes `project.vault.write-path` ("the
//! vault becomes a File Root and all writes to it go through the Files
//! API") a binding rather than a rewrite: a [`PageSink`] is bound per
//! vault root, and when one is, the bytes go to it. When none is, they
//! go to the filesystem exactly as before. This crate cannot depend on
//! the Files backend — Files depends on the vault — so the sink is a
//! trait and the binding is a registry the Files side writes into once
//! it has adopted the vault as a root.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use thiserror::Error;

use crate::vault::{Vault, VaultPage};

/// Where a vault's pages go when they are written.
///
/// The port `project.vault.write-path` is implemented through. `rel` is
/// the page's path relative to the vault root, forward-slashed, the way
/// `VaultPage::rel_path` holds it. The implementation owns atomicity:
/// a reader must never see a partial page, which the default (the
/// filesystem, via [`write_atomic`]) guarantees by temp-and-rename and
/// a Files-backed sink guarantees the same way inside the root's tree.
pub trait PageSink: Send + Sync {
    /// Write the whole page.
    fn write(&self, root: &Path, rel: &str, bytes: &[u8]) -> Result<(), String>;
    /// Remove the page. Absent is not an error.
    fn remove(&self, root: &Path, rel: &str) -> Result<(), String>;
    /// Move the page. The destination must not exist; the caller has
    /// already decided what an occupied destination means.
    fn rename(&self, root: &Path, from: &str, to: &str) -> Result<(), String>;
}

/// The bound sinks, keyed by vault root. A handful at most — one per
/// org this process hosts — so a scan is the right data structure.
fn sinks() -> &'static RwLock<Vec<(PathBuf, Arc<dyn PageSink>)>> {
    static SINKS: std::sync::OnceLock<RwLock<Vec<(PathBuf, Arc<dyn PageSink>)>>> =
        std::sync::OnceLock::new();
    SINKS.get_or_init(|| RwLock::new(Vec::new()))
}

/// The form roots are compared in: canonical where the root exists,
/// as given where it does not yet. A vault that is about to be created
/// can still be bound.
fn key_of(root: &Path) -> PathBuf {
    root.canonicalize().unwrap_or_else(|_| root.to_path_buf())
}

// t[impl project.vault.write-path] — the binding. After this, every
// page under `root` written by any entity store in the process goes
// through `sink` rather than `std::fs`
/// Route every write under `root` through `sink`. Rebinding replaces.
pub fn bind_sink(root: &Path, sink: Arc<dyn PageSink>) {
    let key = key_of(root);
    let mut bound = sinks().write().expect("page sink registry poisoned");
    bound.retain(|(r, _)| *r != key);
    bound.push((key, sink));
}

/// Send writes under `root` back to the filesystem.
pub fn unbind_sink(root: &Path) {
    let key = key_of(root);
    sinks()
        .write()
        .expect("page sink registry poisoned")
        .retain(|(r, _)| *r != key);
}

/// The sink bound for `root`, if any.
pub fn sink_for(root: &Path) -> Option<Arc<dyn PageSink>> {
    let key = key_of(root);
    sinks()
        .read()
        .expect("page sink registry poisoned")
        .iter()
        .find(|(r, _)| *r == key)
        .map(|(_, s)| Arc::clone(s))
}

/// Write one page's bytes: through the bound sink when there is one,
/// to the filesystem when there is not.
fn write_page(vault: &Vault, rel: &str, bytes: &[u8]) -> Result<(), String> {
    match sink_for(&vault.root) {
        Some(sink) => sink.write(&vault.root, rel, bytes),
        None => {
            let abs = vault.root.join(rel);
            if let Some(parent) = abs.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
            }
            write_atomic(&abs, bytes)
        }
    }
}

/// Remove one page: through the bound sink when there is one.
fn remove_page(vault: &Vault, rel: &str) -> Result<(), String> {
    match sink_for(&vault.root) {
        Some(sink) => sink.remove(&vault.root, rel),
        None => {
            let abs = vault.root.join(rel);
            if abs.exists() {
                std::fs::remove_file(&abs).map_err(|e| format!("rm {}: {e}", abs.display()))?;
            }
            Ok(())
        }
    }
}

/// Write a page under `root` without loading the vault.
///
/// The entity backends — project, task, goal, milestone, workstream —
/// write one page at a time and must not pay O(vault) to do it, so they
/// used to `std::fs::write` directly. This is the same one-page write,
/// through the port: bound sink if there is one, atomic filesystem
/// write if not. Parent directories are created.
pub fn save_page_at(root: &Path, rel: &str, body: &str) -> Result<(), MutateError> {
    let vault = one_page(root, rel, body);
    write_page(&vault, rel, body.as_bytes()).map_err(MutateError::Io)
}

/// Remove a page under `root` without loading the vault. Absent is not
/// an error.
pub fn delete_page_at(root: &Path, rel: &str) -> Result<(), MutateError> {
    let vault = one_page(root, rel, "");
    remove_page(&vault, rel).map_err(MutateError::Io)
}

/// Move a page under `root` without loading the vault.
///
/// Refuses an occupied destination with [`MutateError::AlreadyExists`]
/// — the entity backends all check first and this keeps the check true
/// under a race. Parent directories of `to` are created.
pub fn move_page_at(root: &Path, from: &str, to: &str) -> Result<(), MutateError> {
    if root.join(to).exists() {
        return Err(MutateError::AlreadyExists(to.to_string()));
    }
    match sink_for(root) {
        Some(sink) => sink.rename(root, from, to).map_err(MutateError::Io),
        None => {
            let (src, dst) = (root.join(from), root.join(to));
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| MutateError::Io(format!("mkdir {}: {e}", parent.display())))?;
            }
            std::fs::rename(&src, &dst).map_err(|e| MutateError::Io(format!("rename: {e}")))
        }
    }
}

/// A vault holding one page, for the `*_at` helpers: `save_page` needs
/// the page in memory to write it, and opening the whole org vault to
/// save one file would make every write O(vault).
fn one_page(root: &Path, rel: &str, body: &str) -> Vault {
    let path = Path::new(rel);
    Vault {
        root: root.to_path_buf(),
        pages: vec![VaultPage {
            rel_path: rel.to_string(),
            basename: path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string(),
            folder: path
                .parent()
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default(),
            raw: body.to_string(),
            mtime: std::time::SystemTime::now(),
        }],
        bases: Vec::new(),
        property_types: Default::default(),
    }
}

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
    let body = vault.pages[idx].raw.clone();
    write_page(vault, page_rel_path, body.as_bytes()).map_err(MutateError::Io)?;
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
    remove_page(vault, rel_path).map_err(MutateError::Io)?;
    vault.pages.remove(idx);
    Ok(())
}

/// Write `bytes` to `path` atomically — write to a sibling temp
/// file, then rename over the target. Survives partial writes.
// t[impl vault.write.atomic] — temp file in the same directory, then
// rename. The same-directory part is load-bearing: a rename across
// filesystems is a copy, and a copy is exactly the torn write this
// avoids
// Not `project.vault.write-path`: this is the filesystem half of the
// port, what a vault gets when no [`PageSink`] is bound to it. The rule
// is met by the binding in [`bind_sink`] and the Files-backed sink that
// calls it, not by this function. Public because that sink writes the
// same way inside the root's tree and should not have a second copy of
// the temp-and-rename dance
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
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

    /// A sink that records rather than writes.
    struct Recording(std::sync::Mutex<Vec<String>>);

    impl PageSink for Recording {
        fn write(&self, _root: &Path, rel: &str, bytes: &[u8]) -> Result<(), String> {
            self.0
                .lock()
                .unwrap()
                .push(format!("write {rel} {}", bytes.len()));
            Ok(())
        }
        fn remove(&self, _root: &Path, rel: &str) -> Result<(), String> {
            self.0.lock().unwrap().push(format!("remove {rel}"));
            Ok(())
        }
        fn rename(&self, _root: &Path, from: &str, to: &str) -> Result<(), String> {
            self.0.lock().unwrap().push(format!("rename {from} {to}"));
            Ok(())
        }
    }

    // t[verify project.vault.write-path] — the port: a bound sink sees
    // every mutation and the filesystem sees none; unbound, the
    // filesystem write is back
    #[test]
    fn a_bound_sink_takes_every_write_and_unbinding_gives_them_back() {
        let (tmp, mut v) = fresh_vault();
        let sink = Arc::new(Recording(std::sync::Mutex::new(Vec::new())));
        bind_sink(tmp.path(), sink.clone());

        create_page(&mut v, "a.md", "one").unwrap();
        save_page_at(tmp.path(), "b.md", "two").unwrap();
        move_page_at(tmp.path(), "b.md", "c.md").unwrap();
        delete_page(&mut v, "a.md").unwrap();
        delete_page_at(tmp.path(), "c.md").unwrap();

        assert_eq!(
            *sink.0.lock().unwrap(),
            vec![
                "write a.md 3",
                "write b.md 3",
                "rename b.md c.md",
                "remove a.md",
                "remove c.md",
            ]
        );
        assert!(
            std::fs::read_dir(tmp.path()).unwrap().next().is_none(),
            "the filesystem saw a write while a sink was bound"
        );

        unbind_sink(tmp.path());
        save_page_at(tmp.path(), "d.md", "four").unwrap();
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("d.md")).unwrap(),
            "four"
        );
        assert_eq!(
            sink.0.lock().unwrap().len(),
            5,
            "an unbound sink still heard a write"
        );
    }
}
