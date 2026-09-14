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
    // t[impl storage.projection.rebuildable] — the roots register is read
    // back from disk on boot, and where it is gone the marker each root
    // carries in its own tree is the authority. Deleting every database
    // costs a re-read, not a project: the marker travels with the folder,
    // which is why `cp -r` of a root arrives intact
    pub fn open(data_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(data_dir)?;
        let path = data_dir.join("roots.json");
        let roots = if path.exists() {
            let bytes = std::fs::read(&path)?;
            let list: Vec<FileRootInfo> = facet_json::from_slice(&bytes)
                .map_err(|e| crate::error::Error::BadRequest(format!("roots.json: {e}")))?;
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
        let bytes = facet_json::to_string(&list)
            .map_err(|e| crate::error::Error::BadRequest(format!("roots.json: {e}")))?
            .into_bytes();
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

    /// Stop tracking a root, returning what was tracked.
    ///
    /// Removes the index entry and nothing else: the directory, its bytes
    /// and its `.fts-files/` history all stay exactly where they are.
    /// Releasing is not deleting — re-adopting the same path recovers the
    /// history, because the history lives in the tree rather than here.
    pub fn remove(&self, id: Uuid) -> Result<Option<FileRootInfo>> {
        let mut roots = self.roots.lock().expect("registry lock poisoned");
        let removed = roots.remove(&id);
        if removed.is_some() {
            self.persist(&roots)?;
        }
        Ok(removed)
    }

    pub fn get(&self, id: Uuid) -> Option<FileRootInfo> {
        self.roots
            .lock()
            .expect("registry lock poisoned")
            .get(&id)
            .cloned()
    }

    /// Move every root whose live tree sits under `from` to the same
    /// place under `to`, and say how many moved.
    ///
    /// Roots record absolute paths, so a directory rename above them —
    /// an org changing its slug, a data root moving disks — leaves every
    /// root pointing at a tree that is no longer there. This is the
    /// repair, done by prefix so one call covers an org's whole set. A
    /// root that does not start with `from`, or has no path at all, is
    /// left exactly as it was.
    ///
    /// Prefix means *path* prefix: `/data/orgs/ab` must not match
    /// `/data/orgs/abc/vault`, so the comparison is on components, not
    /// on the string.
    pub fn rebase_paths(&self, from: &Path, to: &Path) -> Result<usize> {
        let mut roots = self.roots.lock().expect("registry lock poisoned");
        let mut moved = 0;
        for root in roots.values_mut() {
            let Some(current) = root.path.as_deref().map(Path::new) else {
                continue;
            };
            let Ok(rest) = current.strip_prefix(from) else {
                continue;
            };
            root.path = Some(to.join(rest).to_string_lossy().into_owned());
            moved += 1;
        }
        if moved > 0 {
            self.persist(&roots)?;
        }
        Ok(moved)
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
            // An unplaced root matches no path: it has none here.
            .find(|r| r.local_tree() == Some(path))
            .cloned()
    }
}

/// What a root's on-disk marker (`.fts-root.json`) records. The folder
/// carries this with it, so it — not the registry's absolute path — is
/// the durable statement of "this directory is root X".
#[derive(Debug, Clone, facet::Facet)]
#[repr(C)]
pub struct RootMarker {
    pub id: Uuid,
    pub name: String,
}

/// Write a root's marker, through the type that reads it back.
pub fn write_root_marker(path: &Path, id: Uuid, name: &str) -> crate::error::Result<()> {
    let marker = RootMarker {
        id,
        name: name.to_string(),
    };
    let json = facet_json::to_string(&marker)
        .map_err(|e| crate::error::Error::BadRequest(format!("root marker: {e}")))?;
    std::fs::write(path, json)?;
    Ok(())
}

/// Read `dir`'s root marker, if it has a readable one.
///
/// Absent or unparsable both mean "not a root as far as we can tell":
/// a half-written marker must not make the folder un-registrable
/// forever, and the caller's next step is to write a fresh one.
pub fn read_root_marker(dir: &Path) -> Option<RootMarker> {
    let bytes = std::fs::read(dir.join(crate::consts::MARKER_FILE)).ok()?;
    facet_json::from_slice(&bytes).ok()
}

#[cfg(test)]
mod rebase_tests {
    use super::*;
    use files_proto::RootFlavor;

    fn root(name: &str, path: Option<&str>) -> FileRootInfo {
        FileRootInfo {
            id: Uuid::new_v4(),
            name: name.to_owned(),
            path: path.map(ToOwned::to_owned),
            flavor: RootFlavor::Media,
            created_at: chrono::Utc::now(),
            project_version: None,
        }
    }

    fn paths(reg: &Registry) -> Vec<Option<String>> {
        let mut v: Vec<_> = reg.list().into_iter().map(|r| r.path).collect();
        v.sort();
        v
    }

    /// The case this exists for: an org's directory moved, every root
    /// under it follows, and the result is on disk — a second open sees
    /// it.
    #[test]
    fn roots_under_the_old_prefix_move_and_persist() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Registry::open(dir.path()).unwrap();
        reg.insert(root("Vault", Some("/data/orgs/old/vault")))
            .unwrap();
        reg.insert(root("Wiki", Some("/data/orgs/old/wikis/docs")))
            .unwrap();

        let moved = reg
            .rebase_paths(Path::new("/data/orgs/old"), Path::new("/data/orgs/new"))
            .unwrap();
        assert_eq!(moved, 2);

        let reopened = Registry::open(dir.path()).unwrap();
        assert_eq!(
            paths(&reopened),
            vec![
                Some("/data/orgs/new/vault".to_owned()),
                Some("/data/orgs/new/wikis/docs".to_owned()),
            ],
            "the rebase must survive a reopen, or the next boot reads the old paths"
        );
    }

    /// `ab` is not a prefix of `abc`. A string comparison would move a
    /// neighbouring org's roots along with the one being renamed.
    #[test]
    fn a_prefix_is_a_path_prefix_not_a_string_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Registry::open(dir.path()).unwrap();
        reg.insert(root("Mine", Some("/data/orgs/ab/vault")))
            .unwrap();
        reg.insert(root("Theirs", Some("/data/orgs/abc/vault")))
            .unwrap();

        let moved = reg
            .rebase_paths(Path::new("/data/orgs/ab"), Path::new("/data/orgs/xy"))
            .unwrap();
        assert_eq!(moved, 1);
        assert!(
            paths(&reg).contains(&Some("/data/orgs/abc/vault".to_owned())),
            "the neighbour with the longer name must be untouched"
        );
    }

    /// A root with no live tree on this host has nothing to rebase, and
    /// must not be turned into one that points somewhere.
    #[test]
    fn a_root_with_no_path_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Registry::open(dir.path()).unwrap();
        reg.insert(root("Structure only", None)).unwrap();
        reg.insert(root("Elsewhere", Some("/somewhere/else")))
            .unwrap();

        let moved = reg
            .rebase_paths(Path::new("/data/orgs/old"), Path::new("/data/orgs/new"))
            .unwrap();
        assert_eq!(moved, 0, "nothing matched, nothing moved");
        assert_eq!(paths(&reg), vec![None, Some("/somewhere/else".to_owned())]);
    }
}
