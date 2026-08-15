//! `WikiLive` — the file-backed wiki handle.
//!
//! Single struct anchored at the **wiki root** (typically
//! `<data_root>/orgs/<slug>/wiki/Knowledge/`). All operations
//! route through helper modules so the handle stays small.
//! Concurrency: each mutating method acquires a per-vault
//! file lock by convention (atomic temp+rename writes), so
//! multiple handles can coexist safely. No in-memory mutex.

use std::path::{Path, PathBuf};

#[allow(unused_imports)]
use wiki_proto::paths;

use crate::error::WikiLiveError;

/// File-backed wiki handle. Cheap to clone (just wraps
/// a `PathBuf`).
#[derive(Debug, Clone)]
pub struct WikiLive {
    root: PathBuf,
}

impl WikiLive {
    /// Open an existing wiki rooted at `root`. Does not
    /// validate structure — call [`Self::bootstrap`] or
    /// [`Self::is_bootstrapped`] if you need that.
    pub fn open(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Wiki root path. Identical to what was passed to
    /// [`Self::open`].
    #[must_use]
    pub fn wiki_root(&self) -> PathBuf {
        self.root.clone()
    }

    /// Backwards-compat alias for [`Self::wiki_root`]. Kept
    /// because a handful of older call sites still ask for
    /// the "vault root" of a wiki; the two are the same now
    /// that the wiki is its own tree, not a vault subfolder.
    #[must_use]
    pub fn vault_root(&self) -> &Path {
        &self.root
    }

    /// Resolve a path relative to the wiki root, rejecting
    /// any escape via `..`.
    pub(crate) fn wiki_path(&self, rel: &str) -> Result<PathBuf, WikiLiveError> {
        let root = self.wiki_root();
        let full = root.join(rel);
        // Canonicalize the parent (it must exist for
        // canonicalize to succeed); the leaf can be new.
        let parent = full.parent().ok_or_else(|| WikiLiveError::PathEscape {
            path: rel.to_string(),
        })?;
        std::fs::create_dir_all(parent)?;
        let canon_parent = parent.canonicalize()?;
        let canon_root = root.canonicalize()?;
        if !canon_parent.starts_with(&canon_root) {
            return Err(WikiLiveError::PathEscape {
                path: rel.to_string(),
            });
        }
        Ok(canon_parent.join(full.file_name().unwrap_or_default()))
    }

    /// Has [`Self::bootstrap`] run for this vault?
    #[must_use]
    pub fn is_bootstrapped(&self) -> bool {
        let r = self.wiki_root();
        r.is_dir() && r.join(paths::SCHEMA_MD).is_file() && r.join(paths::PURPOSE_MD).is_file()
    }

    /// Scaffold the wiki on disk. Idempotent: existing
    /// files are kept; missing ones get defaults. Returns
    /// `Ok(true)` if anything was created, `Ok(false)`
    /// otherwise.
    pub fn bootstrap(&self) -> Result<bool, WikiLiveError> {
        crate::raw::bootstrap_dirs(self)?;
        let created_schema = crate::context::ensure_schema(self)?;
        let created_purpose = crate::context::ensure_purpose(self)?;
        let created_index = crate::index::ensure_index(self)?;
        let created_log = crate::log_md::ensure_log(self)?;
        Ok(created_schema || created_purpose || created_index || created_log)
    }
}
