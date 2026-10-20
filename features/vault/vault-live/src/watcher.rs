//! Debounced filesystem watcher. Streams [`VaultEvent`]s
//! whenever a `.md` file changes under the vault root.
//!
//! `notify-debouncer-mini` coalesces rapid bursts (e.g. an
//! editor's swap-file dance) into a single event per path.
//!
//! # Native or polling
//!
//! The OS's watcher by default (FSEvents on macOS). With
//! [`POLL_ENV`] set, a polling watcher instead: no OS registration
//! at all, a stat walk of the roots every [`POLL_EVERY`]. That is
//! for the test suites. Every native watch is a round trip to
//! `fseventsd`, served one at a time per process, and seconds apiece
//! when the test binary lives on a busy external volume — a suite of
//! servers that each wait for their watchers measured `fseventsd`, not
//! the code. A test's trees are a handful of files, so polling them is
//! free.

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use notify_debouncer_mini::{
    Config, DebounceEventHandler, DebounceEventResult, DebouncedEventKind, new_debouncer_opt,
};
use thiserror::Error;

/// Set (to anything) to watch by polling rather than with the OS's
/// watcher — see the module docs.
pub const POLL_ENV: &str = "TASK_WATCH_POLL";

/// How often a polling watcher walks its roots.
pub const POLL_EVERY: Duration = Duration::from_millis(100);

/// Whether watchers poll ([`POLL_ENV`]).
#[must_use]
pub fn polling() -> bool {
    std::env::var_os(POLL_ENV).is_some()
}

#[derive(Debug, Error)]
pub enum WatchError {
    #[error("notify: {0}")]
    Notify(String),
}

/// A vault change. The watcher emits one of these per debounced
/// burst; callers reload the affected page (or scan the vault
/// for renames).
#[derive(Clone, Debug)]
pub enum VaultEvent {
    /// File was created or modified.
    Changed { abs_path: PathBuf },
    /// File was removed or renamed.
    Removed { abs_path: PathBuf },
}

/// Start watching `root`. Events arrive on the returned receiver
/// at a debounce of `delay`. The watcher runs in a background
/// thread held by the returned guard — drop the guard to stop.
pub fn watch(
    root: PathBuf,
    delay: Duration,
) -> Result<(mpsc::Receiver<VaultEvent>, WatcherGuard), WatchError> {
    let (mut receivers, guard) = watch_many(vec![root], delay)?;
    let (_, rx) = receivers
        .pop()
        .ok_or_else(|| WatchError::Notify("vault root not watched".into()))?;
    Ok((rx, guard))
}

/// [`watch`] over several roots with **one** OS watch: every root is
/// added to a single stream and committed at once, so on macOS the
/// whole set costs one `fseventsd` registration rather than one each — a
/// registration that takes seconds when the process lives on a slow
/// volume, and that an org with a vault, wikis and asset groups used to
/// pay four or five times over. Each root gets its own receiver, fed
/// every markdown event under it (as given or as the filesystem names
/// it), exactly as a watcher of its own would be. A root that cannot be
/// watched is left out of the stream and reported, the rest still are.
///
/// # Errors
///
/// The watcher cannot be created, or no root could be added.
pub fn watch_many(
    roots: Vec<PathBuf>,
    delay: Duration,
) -> Result<(Vec<(PathBuf, mpsc::Receiver<VaultEvent>)>, WatcherGuard), WatchError> {
    let mut routes: Vec<(Vec<PathBuf>, mpsc::Sender<VaultEvent>)> = Vec::new();
    let mut receivers = Vec::new();
    for root in &roots {
        let (tx, rx) = mpsc::channel::<VaultEvent>();
        let canonical = std::fs::canonicalize(root).unwrap_or_else(|_| root.clone());
        routes.push((vec![root.clone(), canonical], tx));
        receivers.push((root.clone(), rx));
    }
    let handler = move |res: DebounceEventResult| match res {
        Ok(events) => {
            for event in events {
                if !is_markdown(&event.path) {
                    continue;
                }
                if !matches!(
                    event.kind,
                    DebouncedEventKind::Any | DebouncedEventKind::AnyContinuous
                ) {
                    continue;
                }
                for (prefixes, tx) in &routes {
                    if prefixes.iter().any(|p| event.path.starts_with(p)) {
                        let _ = tx.send(VaultEvent::Changed {
                            abs_path: event.path.clone(),
                        });
                    }
                }
            }
        }
        Err(e) => tracing::warn!(?e, "watcher error"),
    };
    let (added, guard) = debounced(delay, handler, &roots)?;
    if added == 0 {
        return Err(WatchError::Notify("no vault root could be watched".into()));
    }
    Ok((receivers, guard))
}

/// Holds the watcher alive; drop it to stop receiving events.
pub struct WatcherGuard {
    _debouncer: Box<dyn Send>,
}

/// Generic file event — not filtered by extension. Used by
/// the wiki feature to watch its raw-sources folder for any
/// file type (PDFs, .txt, .docx, ...).
#[derive(Clone, Debug)]
pub enum FsEvent {
    /// File was created or modified.
    Changed { abs_path: PathBuf },
    /// File was removed or renamed (notify-debouncer-mini
    /// surfaces renames as a remove + create pair).
    Removed { abs_path: PathBuf },
}

/// Start watching `root` for **any file** event (no extension
/// filter). Events arrive on the returned receiver at the
/// given debounce delay.
pub fn watch_any(
    root: PathBuf,
    delay: Duration,
) -> Result<(mpsc::Receiver<FsEvent>, WatcherGuard), WatchError> {
    let (tx, rx) = mpsc::channel::<FsEvent>();
    let handler = move |res: DebounceEventResult| match res {
        Ok(events) => {
            for event in events {
                let abs = event.path.clone();
                let evt = match event.kind {
                    DebouncedEventKind::Any | DebouncedEventKind::AnyContinuous => {
                        FsEvent::Changed { abs_path: abs }
                    }
                    _ => continue,
                };
                let _ = tx.send(evt);
            }
        }
        Err(e) => tracing::warn!(?e, "watcher error"),
    };
    let (added, guard) = debounced(delay, handler, std::slice::from_ref(&root))?;
    if added == 0 {
        return Err(WatchError::Notify(format!(
            "{} not watched",
            root.display()
        )));
    }
    Ok((rx, guard))
}

/// A debouncer over `roots` — the OS's watcher, or a polling one
/// ([`polling`]) — with every root added and committed at once.
/// Returns how many roots it watches.
fn debounced<F: DebounceEventHandler>(
    delay: Duration,
    handler: F,
    roots: &[PathBuf],
) -> Result<(usize, WatcherGuard), WatchError> {
    if polling() {
        attach::<notify::PollWatcher, F>(delay, handler, roots)
    } else {
        attach::<notify::RecommendedWatcher, F>(delay, handler, roots)
    }
}

fn attach<T: notify::Watcher + Send + 'static, F: DebounceEventHandler>(
    delay: Duration,
    handler: F,
    roots: &[PathBuf],
) -> Result<(usize, WatcherGuard), WatchError> {
    let config = Config::default()
        .with_timeout(delay)
        .with_notify_config(notify_config());
    let mut debouncer = new_debouncer_opt::<F, T>(config, handler)
        .map_err(|e| WatchError::Notify(e.to_string()))?;
    let mut added = 0usize;
    {
        let mut paths = debouncer.watcher().paths_mut();
        for root in roots {
            match paths.add(root, notify::RecursiveMode::Recursive) {
                Ok(()) => added += 1,
                Err(e) => tracing::warn!(root = %root.display(), "vault root not watched: {e}"),
            }
        }
        paths
            .commit()
            .map_err(|e| WatchError::Notify(e.to_string()))?;
    }
    Ok((
        added,
        WatcherGuard {
            _debouncer: Box::new(debouncer),
        },
    ))
}

/// How the underlying `notify` watcher is configured.
///
/// The polling watcher ([`POLL_ENV`]) compares contents, not only
/// modification times. `notify`'s `PollWatcher` keeps a file's mtime
/// truncated to whole seconds and reports a change only when that
/// number grows, so a file rewritten within the same second it was last
/// polled — a save right after a create, the commonest edit there is —
/// produced no event. Hashing every file each poll is what this mode can
/// afford: it only runs over test trees of a handful of small files. The
/// native watcher ignores both settings.
fn notify_config() -> notify::Config {
    notify::Config::default()
        .with_poll_interval(POLL_EVERY)
        .with_compare_contents(true)
}

fn is_markdown(path: &std::path::Path) -> bool {
    matches!(
        path.extension()
            .and_then(|s| s.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("md" | "markdown")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A rewrite the poller's whole-second mtime cannot see — here made
    /// certain by putting the old mtime back — is still a change, because
    /// the poller hashes contents. Without that a save within a second of
    /// the last poll (a create then an edit, the commonest pair there is)
    /// produced no event at all under [`POLL_ENV`].
    #[test]
    fn the_poller_sees_a_rewrite_that_leaves_the_mtime_alone() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("note.md");
        std::fs::write(&file, b"hello").unwrap();
        let mtime = std::fs::metadata(&file).unwrap().modified().unwrap();

        let (tx, rx) = mpsc::channel::<DebounceEventResult>();
        let (added, _guard) = attach::<notify::PollWatcher, _>(
            Duration::from_millis(50),
            tx,
            &[dir.path().to_path_buf()],
        )
        .expect("poll watcher");
        assert_eq!(added, 1);

        std::fs::write(&file, b"hello, again").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(mtime)
            .unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let seen = loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match rx.recv_timeout(left) {
                Ok(Ok(events)) if events.iter().any(|e| e.path.ends_with("note.md")) => {
                    break true;
                }
                Ok(_) => {}
                Err(_) => break false,
            }
        };
        assert!(seen, "a rewrite within the same second produced no event");
    }
}
