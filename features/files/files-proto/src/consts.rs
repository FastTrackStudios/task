//! Names every part of the Files platform agrees on for a File Root's
//! own internals. They live in the proto (rather than in one backend
//! crate) because more than one implementation has to agree on them: the
//! `files` backend hides them from root browsing, and a Storage agent
//! hosting a root's live tree creates [`STORE_DIR`] when it initializes
//! the authoritative repo (issue #262).

/// Marker file at a root's top level recording its stable id (ADR 0001 /
/// glossary "File Root": "identified by a stable id in its entity plus a
/// marker file in the tree").
pub const MARKER_FILE: &str = ".fts-root.json";

/// Directory at a root's top level holding its version-store repo
/// (`task-files-version-store`'s jj repo + CAS chunk store; on a
/// software root, the jj metadata colocated with the real git repo —
/// see [`GIT_DIR`]).
pub const STORE_DIR: &str = ".fts-files";

/// A software File Root's real git repository (ADR 0001's `software`
/// flavor: "a perfectly normal `.git` for GitHub, CI, IDEs"). It is the
/// root's object store, so — exactly like [`STORE_DIR`] — it is skipped
/// at every walk depth and hidden from root browsing, never ingested as
/// ordinary content.
pub const GIT_DIR: &str = ".git";

/// Marker file that makes the directory holding it **node-private**:
/// it stays on this machine's disk and nowhere else. A private
/// directory is never versioned (so never in a root's history, never
/// replicated to a peer), never listed by a browse, a facet listing or
/// a vault manifest, and never served by name.
///
/// It exists for state a feature must keep beside the vault but that
/// is not vault content — the contacts feature's CardDAV account files
/// (`Records/contacts/.sync/`) are the first, and the reason. The
/// feature that owns the directory writes the marker before it writes
/// anything else into it; Files and the vault lanes only honour it, so
/// neither has to know which feature a private directory belongs to.
pub const PRIVATE_MARKER: &str = ".fts-private";

/// Does `dir` carry [`PRIVATE_MARKER`]? One `stat`.
#[cfg(not(target_arch = "wasm32"))]
#[must_use]
pub fn is_private_dir(dir: &std::path::Path) -> bool {
    dir.join(PRIVATE_MARKER).is_file()
}

/// Is `rel` (a `/`-separated path relative to `root`) inside a private
/// directory — at any depth below `root`, or naming the marker itself?
///
/// The question a lane asks before it serves a path by name: a listing
/// that hides a private directory is no guard if the same file can be
/// fetched by spelling its path.
#[cfg(not(target_arch = "wasm32"))]
#[must_use]
pub fn is_within_private(root: &std::path::Path, rel: &str) -> bool {
    let mut dir = root.to_path_buf();
    let parts: Vec<&str> = rel.split('/').filter(|p| !p.is_empty()).collect();
    if parts.last() == Some(&PRIVATE_MARKER) {
        return true;
    }
    // Every ancestor directory of `rel` (and `rel` itself, when it is a
    // directory) is checked; the root's own top level is a root, never
    // private.
    for part in parts {
        dir.push(part);
        if is_private_dir(&dir) {
            return true;
        }
    }
    false
}
