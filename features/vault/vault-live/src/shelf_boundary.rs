//! Where one registered root stops and a nested one begins.
//!
//! # The rule, and the bug it exists to prevent
//!
//! A *shelf* is a directory registered on [`crate::sync::Backend`],
//! `GraphBackend` and `VaultCollab` (see `org_proto::shelf`). Two
//! shelves may sit one inside the other on disk — an album and one of
//! its songs, a project and its sub-project — and when they do, every
//! file under the inner one must belong to **exactly one** of them.
//!
//! Without that, a file under the child lands in two roots at once:
//! two link graphs claiming it, two disk watchers announcing each
//! write, and — the expensive one — two CRDT document ids over one
//! file. Two people editing that file then converge on two different
//! documents and clobber each other, silently, with no error anywhere.
//! `org_proto::shelf`'s module docs call that the worst class of bug
//! this codebase can produce, and this module is the guard.
//!
//! # The Files layer already had this, and it is the template
//!
//! `files::registry::Registry::conflicting_root` deliberately *allows*
//! a File Root inside a File Root — "a song inside an album, a show
//! inside a venue" is its own example — and it can afford to because
//! `files::scan::walk_live_tree` prunes the inner root's directory out
//! of the outer root's walk. Its doc puts the pairing plainly:
//!
//! > **Do not loosen this further without checking that the walk still
//! > prunes**; the failure mode is silent and expensive.
//!
//! That was true on one side of the system and not the other. The
//! collaboration layer allowed nothing and pruned nothing, so nesting
//! was simply unavailable here — which is why `OrgRoot::project_shelves`
//! could only ever enumerate top-level projects, and why a sub-project
//! could not be a shelf of its own. This is the missing half.
//!
//! # Why a marker file and not a registry lookup
//!
//! The walk is synchronous, runs on every scan, and has no handle on
//! the backend's root map — and even if it did, consulting the map
//! would make the answer depend on registration *order*: a child
//! registered after its parent would be invisible to a walk the parent
//! had already done. A marker on disk is the same answer whoever asks
//! and whenever, which is the property `files_proto::consts::MARKER_FILE`
//! is already relied on for one layer down.
//!
//! # The one marker, today
//!
//! [`org_proto::PROJECT_PAGE`] — a directory holding a `project.md` is
//! a project, and `project.identity.declaration` says a project is
//! exactly a directory that declares itself one. Wikis and asset groups
//! do not nest (their tiers are flat by construction), so the list has
//! one entry. It is a list rather than a constant because the next tier
//! that nests should add itself here rather than growing a second prune
//! somewhere else — a second prune is how the two halves drift apart,
//! which is the failure `walk_live_tree`'s doc warns about.

use std::path::Path;

/// The files that declare a directory to be a registered root of its
/// own, and therefore not part of its parent's.
pub const SHELF_MARKERS: &[&str] = &[org_proto::PROJECT_PAGE];

/// Whether `dir` is a shelf nested inside the root being walked.
///
/// `root` itself is never a nested shelf — it is the shelf being walked
/// — and forgetting that would make every project's own scan return
/// nothing, which is the first thing to check if one ever does.
#[must_use]
pub fn is_nested_shelf(root: &Path, dir: &Path) -> bool {
    if dir == root {
        return false;
    }
    SHELF_MARKERS.iter().any(|m| dir.join(m).is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shelf(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join(org_proto::PROJECT_PAGE),
            "---\ntype: project\n---\n",
        )
        .unwrap();
    }

    #[test]
    fn a_shelf_is_not_nested_inside_itself() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("album");
        shelf(&root);
        assert!(
            !is_nested_shelf(&root, &root),
            "a shelf that pruned itself would scan to nothing"
        );
    }

    #[test]
    fn a_child_that_declares_itself_is_a_boundary() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("album");
        shelf(&root);
        let song = root.join("track-two");
        shelf(&song);
        let plain = root.join("Deliverables");
        std::fs::create_dir_all(&plain).unwrap();

        assert!(is_nested_shelf(&root, &song));
        assert!(
            !is_nested_shelf(&root, &plain),
            "an ordinary folder is the parent's material, not a boundary"
        );
    }
}
