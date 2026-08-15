//! Filesystem helpers for the Project Version restart flow (issue
//! #268): clearing the live tree for a new lineage and seeding it from
//! a template. The commit-side of the flip lives in `backend` — these
//! are the disk halves, kept pure so they are testable and so the
//! backend reads as the sequence the spec names: checkpoint, reshape,
//! flip.

use std::path::{Path, PathBuf};

use crate::consts::{GIT_DIR, MARKER_FILE, STORE_DIR};
use crate::error::{Error, Result};

/// Copy `source`'s contents into `root_path` (the template seed of
/// [`files_proto::RestartMode::Template`]). A root's internals — the
/// marker, the store, a `.git` — are never copied in from a template,
/// whatever the template folder contains: a template carrying a stale
/// `.fts-files` would otherwise smuggle a second version store into
/// the tree (the nested-root hazard of PR #280).
pub fn seed_template(root_path: &Path, source: &Path) -> Result<()> {
    copy_dir(source, root_path)
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == MARKER_FILE || name == STORE_DIR || name == GIT_DIR {
            continue;
        }
        let target = to.join(&name);
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            std::fs::create_dir_all(&target)?;
            copy_dir(&entry.path(), &target)?;
        } else if file_type.is_file() {
            // Never overwrite a file that survived the clear: whatever
            // it is — an ignored note, a mid-flip save the clear kept —
            // it is by definition data no checkpoint holds, and a
            // template seed must not be the thing that destroys it
            // (PR #290 review). The template's copy simply loses to
            // what's already there.
            if target.exists() {
                tracing::warn!(
                    target_path = %target.display(),
                    "template seed skipping a file that survived the clear",
                );
                continue;
            }
            std::fs::copy(entry.path(), &target)?;
        }
        // Symlinks in a template are skipped, matching the scan walker.
    }
    Ok(())
}

/// Remove the now-empty ancestor directories of `removed` files, up to
/// (never including) `root_path`. Best effort: a directory that still
/// holds anything — an ignored file, a mid-flip save, the root's own
/// internals — simply stays, which is exactly right.
pub fn prune_empty_dirs(root_path: &Path, removed: &[PathBuf]) {
    for path in removed {
        let mut dir = path.parent();
        while let Some(d) = dir {
            if d == root_path || !d.starts_with(root_path) {
                break;
            }
            if std::fs::remove_dir(d).is_err() {
                break; // not empty (or gone) — done climbing this line
            }
            dir = d.parent();
        }
    }
}

/// Validate a template source directory before anything is mutated:
/// it must exist and be a directory.
pub fn validate_template(source: &Path) -> Result<()> {
    if !source.is_dir() {
        return Err(Error::BadRequest(format!(
            "{}: template source is not a directory",
            source.display()
        )));
    }
    Ok(())
}
