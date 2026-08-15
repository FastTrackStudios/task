//! The server's own index of known File Roots, persisted as JSON beside
//! its version-store repos (`<data_dir>/roots.json`). Together with the
//! marker file each root carries in its own live tree
//! ([`crate::backend::MARKER_FILE`]), this is the "entity" half of
//! ADR 0001 / the glossary's "File Root — a first-class vault entity":
//! a full Vault-entity integration (frontmatter note, sync) is future
//! work past this ticket's RPC-surface scope, but identity already
//! survives a restart through this file plus the marker.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use files_proto::FileRootInfo;
use uuid::Uuid;

use crate::error::Result;

#[derive(Debug)]
pub struct Registry {
    path: PathBuf,
    roots: Mutex<HashMap<Uuid, FileRootInfo>>,
}

impl Registry {
    pub fn open(data_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(data_dir)?;
        let path = data_dir.join("roots.json");
        let roots = if path.exists() {
            let bytes = std::fs::read(&path)?;
            let list: Vec<FileRootInfo> = serde_json::from_slice(&bytes)?;
            list.into_iter().map(|r| (r.id, r)).collect()
        } else {
            HashMap::new()
        };
        Ok(Self {
            path,
            roots: Mutex::new(roots),
        })
    }

    /// Write the index atomically: a bare `fs::write` that is interrupted
    /// (a crash, a full disk) leaves a truncated `roots.json` and every
    /// root's identity with it, which is a bad trade for one rename
    /// (PR #284 review — `files-storage`'s registry already did this).
    fn persist(&self, roots: &HashMap<Uuid, FileRootInfo>) -> Result<()> {
        let mut list: Vec<&FileRootInfo> = roots.values().collect();
        list.sort_by(|a, b| a.id.cmp(&b.id));
        let bytes = serde_json::to_vec_pretty(&list)?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    pub fn insert(&self, root: FileRootInfo) -> Result<()> {
        let mut roots = self.roots.lock().expect("registry lock poisoned");
        roots.insert(root.id, root);
        self.persist(&roots)
    }

    pub fn get(&self, id: Uuid) -> Option<FileRootInfo> {
        self.roots
            .lock()
            .expect("registry lock poisoned")
            .get(&id)
            .cloned()
    }

    pub fn list(&self) -> Vec<FileRootInfo> {
        let mut v: Vec<_> = self
            .roots
            .lock()
            .expect("registry lock poisoned")
            .values()
            .cloned()
            .collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    /// A registered root that would conflict with a new root at `path`
    /// — an EXACT match, and only that.
    ///
    /// Nesting is allowed: a root inside another root is a **submodule**
    /// (a song inside an album, a show inside a venue), which is the
    /// shape this work is actually organised in. The parent keeps its
    /// own files and its own history; the child keeps its own store,
    /// and [`crate::scan::walk_live_tree`] prunes the child's directory
    /// out of the parent's walk so the parent never ingests it as
    /// content.
    ///
    /// This used to refuse both containment directions, on the sound
    /// reasoning that an outer root would otherwise swallow the inner
    /// root's version store as ordinary files. That is still true — the
    /// prune is what makes relaxing this safe, so the two must stay
    /// together. **Do not loosen this further without checking that the
    /// walk still prunes**; the failure mode is silent and expensive
    /// (the child's whole history duplicated into the parent's store,
    /// again on every checkpoint).
    ///
    /// The marker file on disk still covers exact-match for a root this
    /// registry has not seen.
    pub fn conflicting_root(&self, path: &Path) -> Option<FileRootInfo> {
        self.roots
            .lock()
            .expect("registry lock poisoned")
            .values()
            .find(|r| path == Path::new(&r.path))
            .cloned()
    }
}

/// What a root's on-disk marker (`.fts-root.json`) records. The folder
/// carries this with it, so it — not the registry's absolute path — is
/// the durable statement of "this directory is root X".
#[derive(Debug, Clone, serde::Deserialize)]
pub struct RootMarker {
    pub id: Uuid,
    pub name: String,
}

/// Read `dir`'s root marker, if it has a readable one.
///
/// Absent or unparsable both mean "not a root as far as we can tell":
/// a half-written marker must not make the folder un-registrable
/// forever, and the caller's next step is to write a fresh one.
pub fn read_root_marker(dir: &Path) -> Option<RootMarker> {
    let bytes = std::fs::read(dir.join(crate::consts::MARKER_FILE)).ok()?;
    serde_json::from_slice(&bytes).ok()
}
