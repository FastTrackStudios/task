//! Debounced filesystem watcher. Streams [`VaultEvent`]s
//! whenever a `.md` file changes under the vault root.
//!
//! `notify-debouncer-mini` coalesces rapid bursts (e.g. an
//! editor's swap-file dance) into a single event per path.

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use notify::EventKind;
use notify_debouncer_mini::{DebounceEventResult, DebouncedEventKind, new_debouncer};
use thiserror::Error;

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
    let (tx, rx) = mpsc::channel::<VaultEvent>();
    let mut debouncer = new_debouncer(delay, move |res: DebounceEventResult| match res {
        Ok(events) => {
            for event in events {
                let abs = event.path.clone();
                if !is_markdown(&abs) {
                    continue;
                }
                let evt = match event.kind {
                    DebouncedEventKind::Any => VaultEvent::Changed { abs_path: abs },
                    DebouncedEventKind::AnyContinuous => VaultEvent::Changed { abs_path: abs },
                    _ => continue,
                };
                let _ = tx.send(evt);
            }
        }
        Err(e) => tracing::warn!(?e, "watcher error"),
    })
    .map_err(|e| WatchError::Notify(e.to_string()))?;
    debouncer
        .watcher()
        .watch(&root, notify::RecursiveMode::Recursive)
        .map_err(|e| WatchError::Notify(e.to_string()))?;
    Ok((
        rx,
        WatcherGuard {
            _debouncer: debouncer,
        },
    ))
}

/// Holds the watcher alive; drop it to stop receiving events.
pub struct WatcherGuard {
    _debouncer: notify_debouncer_mini::Debouncer<notify::RecommendedWatcher>,
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
    let mut debouncer = new_debouncer(delay, move |res: DebounceEventResult| match res {
        Ok(events) => {
            for event in events {
                let abs = event.path.clone();
                let evt = match event.kind {
                    DebouncedEventKind::Any => FsEvent::Changed { abs_path: abs },
                    DebouncedEventKind::AnyContinuous => FsEvent::Changed { abs_path: abs },
                    _ => continue,
                };
                let _ = tx.send(evt);
            }
        }
        Err(e) => tracing::warn!(?e, "watcher error"),
    })
    .map_err(|e| WatchError::Notify(e.to_string()))?;
    debouncer
        .watcher()
        .watch(&root, notify::RecursiveMode::Recursive)
        .map_err(|e| WatchError::Notify(e.to_string()))?;
    Ok((
        rx,
        WatcherGuard {
            _debouncer: debouncer,
        },
    ))
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

// Quiet the unused-import warning until the watcher needs
// granular kinds.
#[allow(dead_code)]
fn _force_link_to_notify_eventkind(_k: EventKind) {}
