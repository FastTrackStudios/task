//! Debounced FS watcher. Emits a `VaultEvent` on stdout-style
//! channel; consumers decide what to do (most likely re-open the
//! `Vault` snapshot or patch individual entries).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::guard::SelfWriteGuard;

#[derive(Clone, Debug)]
pub enum VaultEvent {
    /// One or more files changed under the vault root. Paths are
    /// absolute. Self-writes have been filtered out.
    Changed { paths: Vec<PathBuf> },
}

#[derive(Debug, thiserror::Error)]
pub enum WatchError {
    #[error("notify: {0}")]
    Notify(String),
}

/// Spawn a watcher on `root` and stream events into `sink`. Caller
/// keeps the returned `WatchHandle` alive — dropping it stops the
/// watcher. Self-writes (those marked on `guard`) are suppressed.
pub fn watch(
    root: PathBuf,
    guard: Arc<SelfWriteGuard>,
    sink: tokio::sync::mpsc::UnboundedSender<VaultEvent>,
) -> Result<WatchHandle, WatchError> {
    use notify::RecursiveMode;
    use notify_debouncer_mini::new_debouncer;

    let (raw_tx, mut raw_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<PathBuf>>();
    let mut debouncer = new_debouncer(
        Duration::from_millis(500),
        move |res: notify_debouncer_mini::DebounceEventResult| {
            if let Ok(events) = res {
                let paths: Vec<PathBuf> = events.into_iter().map(|e| e.path).collect();
                let _ = raw_tx.send(paths);
            }
        },
    )
    .map_err(|e| WatchError::Notify(e.to_string()))?;
    debouncer
        .watcher()
        .watch(&root, RecursiveMode::Recursive)
        .map_err(|e| WatchError::Notify(e.to_string()))?;

    let pump = tokio::spawn(async move {
        while let Some(paths) = raw_rx.recv().await {
            let filtered: Vec<PathBuf> = paths
                .into_iter()
                .filter(|p| !guard.is_recent(p) && !is_hidden(p))
                .collect();
            if filtered.is_empty() {
                continue;
            }
            if sink.send(VaultEvent::Changed { paths: filtered }).is_err() {
                break;
            }
        }
    });

    Ok(WatchHandle {
        _debouncer: debouncer,
        pump,
    })
}

fn is_hidden(p: &Path) -> bool {
    p.components().any(|c| {
        c.as_os_str().to_string_lossy().starts_with('.')
            && c.as_os_str() != std::ffi::OsStr::new(".")
            && c.as_os_str() != std::ffi::OsStr::new("..")
    })
}

pub struct WatchHandle {
    _debouncer: notify_debouncer_mini::Debouncer<notify::RecommendedWatcher>,
    pump: tokio::task::JoinHandle<()>,
}

impl Drop for WatchHandle {
    fn drop(&mut self) {
        self.pump.abort();
    }
}
