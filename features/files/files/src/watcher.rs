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

use notify_fs::{Event, RecursiveMode, Watcher};
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

/// Set (to anything) to watch roots by polling instead of with the OS's
/// watcher: no `fseventsd` registration, a stat walk every
/// [`POLL_EVERY`]. For the test suites, whose trees are a handful of
/// files and whose binaries — on a busy external build volume — waited
/// seconds per native registration. The same switch as `vault-live`'s.
pub const POLL_ENV: &str = "TASK_WATCH_POLL";

/// How often a polling watcher walks its roots.
pub const POLL_EVERY: std::time::Duration = std::time::Duration::from_millis(100);

/// A live recursive watch on one root's live tree. Dropping it stops
/// the root's hints.
///
/// Several roots can share one OS watch ([`Self::spawn_many`]): on macOS
/// every watch is a registration with `fseventsd`, seconds apiece on a
/// busy machine or from a slow volume, so the roots an org boots with are
/// watched by one stream rather than one each. A dropped handle takes its
/// root out of the stream's routing — no OS call, so tearing a backend
/// down costs nothing — and the stream itself stops with the last one.
pub struct RootWatcher {
    shared: std::sync::Arc<Shared>,
    root_id: Uuid,
}

/// One OS watch and the roots it reports for.
struct Shared {
    _watcher: Box<dyn Watcher + Send + Sync>,
    routes: std::sync::Arc<std::sync::Mutex<Vec<(Uuid, PathBuf)>>>,
}

impl std::fmt::Debug for RootWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RootWatcher")
            .field("root_id", &self.root_id)
            .finish_non_exhaustive()
    }
}

impl Drop for RootWatcher {
    fn drop(&mut self) {
        if let Ok(mut routes) = self.shared.routes.lock() {
            routes.retain(|(id, _)| *id != self.root_id);
        }
    }
}

impl RootWatcher {
    /// Start watching `root_path` recursively, reporting to `sink`.
    pub fn spawn<S: ActivitySink>(
        root_id: Uuid,
        root_path: &Path,
        sink: std::sync::Arc<S>,
    ) -> Result<Self> {
        Self::spawn_many(vec![(root_id, root_path.to_path_buf())], sink)?
            .pop()
            .ok_or_else(|| Error::Repo(format!("watch {}: not watched", root_path.display())))
    }

    /// Watch every root in `roots` with **one** OS watch — all added,
    /// then committed once. A root that cannot be watched is left out
    /// (and logged); the rest are returned, one handle each.
    pub fn spawn_many<S: ActivitySink>(
        roots: Vec<(Uuid, PathBuf)>,
        sink: std::sync::Arc<S>,
    ) -> Result<Vec<Self>> {
        let routes = std::sync::Arc::new(std::sync::Mutex::new(roots.clone()));
        let routing = routes.clone();
        let handler = move |res: notify_fs::Result<Event>| {
            let Ok(event) = res else {
                // A dropped/failed event is survivable by construction:
                // the next hint (or an explicit checkpoint) still finds
                // everything, because the scan is the authority.
                return;
            };
            if !is_write_like(&event) {
                return;
            }
            let Ok(routes) = routing.lock() else { return };
            for (root_id, root_path) in routes.iter() {
                let paths = relative_paths(root_path, &event.paths);
                if !paths.is_empty() {
                    sink.note_activity(*root_id, paths);
                }
            }
        };
        // Native, or polling for the test suites (`POLL_ENV`): see there
        // and `poll_config` for why the poller hashes contents.
        let mut watcher: Box<dyn Watcher + Send + Sync> = if std::env::var_os(POLL_ENV).is_some() {
            Box::new(
                notify_fs::PollWatcher::new(handler, poll_config())
                    .map_err(|e| Error::Repo(format!("watcher: {e}")))?,
            )
        } else {
            Box::new(
                notify_fs::recommended_watcher(handler)
                    .map_err(|e| Error::Repo(format!("watcher: {e}")))?,
            )
        };
        let mut watched = Vec::with_capacity(roots.len());
        {
            let mut paths = watcher.paths_mut();
            for (root_id, root_path) in &roots {
                match paths.add(root_path, RecursiveMode::Recursive) {
                    Ok(()) => watched.push(*root_id),
                    Err(e) => {
                        tracing::warn!(%root_id, path = %root_path.display(), "files: root not watched: {e}");
                    }
                }
            }
            paths
                .commit()
                .map_err(|e| Error::Repo(format!("watch roots: {e}")))?;
        }
        if let Ok(mut r) = routes.lock() {
            r.retain(|(id, _)| watched.contains(id));
        }
        let shared = std::sync::Arc::new(Shared {
            _watcher: watcher,
            routes,
        });
        Ok(watched
            .into_iter()
            .map(|root_id| Self {
                shared: shared.clone(),
                root_id,
            })
            .collect())
    }

    #[must_use]
    pub fn root_id(&self) -> Uuid {
        self.root_id
    }
}

/// How the polling watcher (`POLL_ENV`) is configured.
///
/// It compares contents, not only modification times. `notify`'s
/// `PollWatcher` keeps a file's mtime truncated to whole seconds and
/// reports a change only when that number grows, so a file rewritten
/// within the same second it was last polled — a save right after a
/// create, the commonest edit there is — produced no hint, no session
/// opened, and nothing was captured. Hashing every file each poll is
/// what this mode can afford: it only runs over test trees of a handful
/// of small files.
fn poll_config() -> notify_fs::Config {
    notify_fs::Config::default()
        .with_poll_interval(POLL_EVERY)
        .with_compare_contents(true)
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
        // A write into a node-private directory is not activity on the
        // root: nothing under it will ever be versioned.
        if crate::consts::is_within_private(root_path, &rel) {
            continue;
        }
        out.push(rel);
    }
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A rewrite the poller's whole-second mtime cannot see — here made
    /// certain by putting the old mtime back — is still a change, because
    /// the poller hashes contents. Without that the commonest edit (a
    /// save within a second of the last poll) opened no session.
    #[test]
    fn the_poller_sees_a_rewrite_that_leaves_the_mtime_alone() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("note.txt");
        std::fs::write(&file, b"hello").unwrap();
        let mtime = std::fs::metadata(&file).unwrap().modified().unwrap();

        let (tx, rx) = std::sync::mpsc::channel();
        let mut watcher = notify_fs::PollWatcher::new(
            move |res: notify_fs::Result<Event>| {
                if let Ok(event) = res
                    && is_write_like(&event)
                {
                    let _ = tx.send(event);
                }
            },
            poll_config(),
        )
        .expect("poll watcher");
        watcher
            .watch(dir.path(), RecursiveMode::Recursive)
            .expect("watch");

        std::fs::write(&file, b"hello, again").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(mtime)
            .unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let seen = loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match rx.recv_timeout(left) {
                Ok(event) if event.paths.iter().any(|p| p.ends_with("note.txt")) => break true,
                Ok(_) => {}
                Err(_) => break false,
            }
        };
        assert!(seen, "a rewrite within the same second produced no event");
    }
}
