//! Local filesystem [`ContentBackend`] impl.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use mount_proto::{ContentBackend, Entry, MountError};

/// Filesystem-backed mount. Read/write rooted at `root`; all
/// trait paths are joined onto it.
///
/// Safety: rejects paths containing `..` segments so a
/// caller-supplied relative path can't escape the mount root.
#[derive(Debug, Clone)]
pub struct Filesystem {
    root: PathBuf,
}

impl Filesystem {
    /// Wrap a root path. Caller is responsible for ensuring
    /// the path exists (or that the first write creates it).
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Root directory this mount serves.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn resolve(&self, rel: &str) -> Result<PathBuf, MountError> {
        if rel.split(['/', '\\']).any(|seg| seg == "..") {
            return Err(MountError::InvalidMount(format!(
                "path `{rel}` contains `..` — refused"
            )));
        }
        Ok(self.root.join(rel))
    }
}

impl ContentBackend for Filesystem {
    fn list(&self, prefix: &str) -> Result<Vec<Entry>, MountError> {
        let base = self.resolve(prefix)?;
        if !base.exists() {
            return Err(MountError::NotFound {
                path: prefix.to_owned(),
            });
        }
        let mut out = Vec::new();
        for entry in walkdir::WalkDir::new(&base)
            .min_depth(1)
            .max_depth(1)
            .into_iter()
            .filter_map(Result::ok)
        {
            let rel = entry
                .path()
                .strip_prefix(&self.root)
                .unwrap_or(entry.path())
                .to_string_lossy()
                .replace('\\', "/");
            let meta = entry
                .metadata()
                .map_err(|e| MountError::Io(e.to_string()))?;
            let modified = meta.modified().ok().map_or_else(
                || DateTime::<Utc>::from(std::time::UNIX_EPOCH),
                DateTime::<Utc>::from,
            );
            out.push(Entry {
                path: rel,
                is_dir: meta.is_dir(),
                size: if meta.is_dir() { 0 } else { meta.len() },
                modified,
            });
        }
        Ok(out)
    }

    fn read(&self, path: &str) -> Result<Vec<u8>, MountError> {
        let p = self.resolve(path)?;
        std::fs::read(&p).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                MountError::NotFound {
                    path: path.to_owned(),
                }
            } else {
                MountError::Io(format!("read {}: {e}", p.display()))
            }
        })
    }

    fn write(&self, path: &str, bytes: &[u8]) -> Result<(), MountError> {
        let p = self.resolve(path)?;
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| MountError::Io(format!("mkdir {}: {e}", parent.display())))?;
        }
        std::fs::write(&p, bytes).map_err(|e| MountError::Io(format!("write {}: {e}", p.display())))
    }

    fn delete(&self, path: &str) -> Result<(), MountError> {
        let p = self.resolve(path)?;
        match std::fs::remove_file(&p) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(MountError::Io(format!("delete {}: {e}", p.display()))),
        }
    }

    fn exists(&self, path: &str) -> Result<bool, MountError> {
        Ok(self.resolve(path)?.exists())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_write_read_delete() {
        let tmp = tempfile::tempdir().unwrap();
        let fs = Filesystem::new(tmp.path());
        fs.write("a/b.md", b"hello").unwrap();
        assert!(fs.exists("a/b.md").unwrap());
        assert_eq!(fs.read("a/b.md").unwrap(), b"hello");
        fs.delete("a/b.md").unwrap();
        assert!(!fs.exists("a/b.md").unwrap());
        // delete is idempotent
        fs.delete("a/b.md").unwrap();
    }

    #[test]
    fn rejects_dotdot_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        let fs = Filesystem::new(tmp.path());
        let err = fs.read("../etc/passwd").unwrap_err();
        assert!(matches!(err, MountError::InvalidMount(_)));
    }

    #[test]
    fn list_skips_parent() {
        let tmp = tempfile::tempdir().unwrap();
        let fs = Filesystem::new(tmp.path());
        fs.write("a.md", b"x").unwrap();
        fs.write("b.md", b"x").unwrap();
        let entries = fs.list("").unwrap();
        assert_eq!(entries.len(), 2);
    }
}
