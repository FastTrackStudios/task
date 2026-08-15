//! The server-side filesystem watcher: one recursive watch per File
//! Root, feeding activity **hints** to the cadence engine.
//!
//! Hints only — see [`crate::cadence`]'s module doc for why this is a
//! timing signal and never a content signal. Consequently this module
//! is deliberately dumb: it maps event paths to root-relative strings,
//! drops the root's own internals, and hands them to an
//! [`ActivitySink`]. Every decision about whether those paths matter
//! (the Ignore set, save points, the debounce) belongs to
//! [`crate::cadence::CadenceEngine`], and every decision about their
//! *content* belongs to the certifying scan.
//!
//! The crates.io watcher is imported as `notify_fs`: `notify` as a
//! workspace-dependency name is already the in-tree notifications
//! feature (`features/task/notify`), and one name can only mean one
//! crate — hence the rename in the root `Cargo.toml`.

use std::path::{Path, PathBuf};

use notify_fs::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use uuid::Uuid;

use crate::consts::{MARKER_FILE, STORE_DIR};
use crate::error::{Error, Result};

/// Where a watcher's hints go. [`crate::FilesBackend`] implements this;
/// the trait exists so the watcher depends on the direction of data
/// flow rather than on the whole backend.
pub trait ActivitySink: Send + Sync + 'static {
    /// `paths` are root-relative, `/`-separated.
    fn note_activity(&self, root_id: Uuid, paths: Vec<String>);
}

/// A live recursive watch on one root's live tree. Dropping it stops
/// the watch.
pub struct RootWatcher {
    _watcher: RecommendedWatcher,
    root_id: Uuid,
}

impl std::fmt::Debug for RootWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RootWatcher")
            .field("root_id", &self.root_id)
            .finish_non_exhaustive()
    }
}

impl RootWatcher {
    /// Start watching `root_path` recursively, reporting to `sink`.
    pub fn spawn<S: ActivitySink>(
        root_id: Uuid,
        root_path: &Path,
        sink: std::sync::Arc<S>,
    ) -> Result<Self> {
        let root_path = root_path.to_path_buf();
        let watched = root_path.clone();
        let mut watcher = notify_fs::recommended_watcher(move |res: notify_fs::Result<Event>| {
            let Ok(event) = res else {
                // A dropped/failed event is survivable by construction:
                // the next hint (or an explicit checkpoint) still finds
                // everything, because the scan is the authority.
                return;
            };
            if !is_write_like(&event) {
                return;
            }
            let paths = relative_paths(&root_path, &event.paths);
            if !paths.is_empty() {
                sink.note_activity(root_id, paths);
            }
        })
        .map_err(|e| Error::Repo(format!("watcher: {e}")))?;
        watcher
            .watch(&watched, RecursiveMode::Recursive)
            .map_err(|e| Error::Repo(format!("watch {}: {e}", watched.display())))?;
        Ok(Self {
            _watcher: watcher,
            root_id,
        })
    }

    #[must_use]
    pub fn root_id(&self) -> Uuid {
        self.root_id
    }
}

/// Does this event mean someone touched content? Access events (and
/// pure metadata reads) are not activity — opening a session because a
/// backup tool walked the tree would checkpoint nothing and reset
/// quiescence for no reason.
fn is_write_like(event: &Event) -> bool {
    use notify_fs::EventKind;
    matches!(
        event.kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) | EventKind::Any
    )
}

/// Root-relative, `/`-separated paths, with the root's own internals
/// (marker file, version store) dropped — writes into `.fts-files` are
/// *our own* writes, and treating them as activity would make every
/// capture start the next session.
fn relative_paths(root_path: &Path, paths: &[PathBuf]) -> Vec<String> {
    let mut out = Vec::new();
    for path in paths {
        let Ok(rel) = path.strip_prefix(root_path) else {
            continue;
        };
        let Some(rel) = rel.to_str() else {
            continue;
        };
        if rel.is_empty() {
            continue;
        }
        let rel = rel.replace(std::path::MAIN_SEPARATOR, "/");
        if rel == MARKER_FILE || rel == STORE_DIR || rel.starts_with(&format!("{STORE_DIR}/")) {
            continue;
        }
        out.push(rel);
    }
    out.sort();
    out.dedup();
    out
}
