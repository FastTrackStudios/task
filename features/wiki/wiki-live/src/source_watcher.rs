//! Source folder watcher — bridges `vault-live::watch_any`
//! to the wiki's ingest queue.
//!
//! On any FS event under `Wiki/raw/sources/`, runs
//! [`WikiLive::rescan_sources`] to refresh the snapshot,
//! then (optionally) enqueues ingest tasks for the
//! created / modified entries.

use std::sync::mpsc::Receiver;
use std::time::Duration;

use vault_live::{FsEvent, WatchError, watch_any};
use wiki_proto::paths;

use crate::error::WikiLiveError;
use crate::queue::SourceChange;
use crate::snapshot::RescanDiff;
use crate::vault::WikiLive;

/// Configuration for the source watcher.
#[derive(Debug, Clone)]
pub struct WatchSourcesOpts {
    /// Debounce window — FS bursts within this interval get
    /// coalesced into a single rescan.
    pub debounce: Duration,
    /// If `true`, the watcher pushes each
    /// `created`/`modified` diff entry into the ingest
    /// queue.
    pub auto_enqueue: bool,
}

impl Default for WatchSourcesOpts {
    fn default() -> Self {
        Self {
            debounce: Duration::from_secs(2),
            auto_enqueue: true,
        }
    }
}

/// Live-event stream item emitted by the source watcher.
#[derive(Debug, Clone)]
pub struct SourceEvent {
    /// Diff produced by the rescan triggered by this FS
    /// event.
    pub diff: RescanDiff,
    /// New ingest tasks the watcher enqueued (empty when
    /// `auto_enqueue == false`).
    pub enqueued_task_ids: Vec<String>,
}

/// Holds the underlying vault-live watcher alive. Drop to
/// stop receiving events.
pub struct SourceWatcherGuard {
    _inner: vault_live::watcher::WatcherGuard,
}

impl WikiLive {
    /// Spawn a watcher on `Wiki/raw/sources/`. Each FS
    /// event triggers a rescan; the receiver yields one
    /// [`SourceEvent`] per debounced burst. Drop the
    /// returned guard to stop.
    pub fn watch_sources(
        &self,
        opts: WatchSourcesOpts,
    ) -> Result<(Receiver<SourceEvent>, SourceWatcherGuard), WikiLiveError> {
        if !self.is_bootstrapped() {
            return Err(WikiLiveError::NotBootstrapped);
        }
        let sources_dir = self.wiki_root().join(paths::SOURCES_DIR);
        std::fs::create_dir_all(&sources_dir)?;

        let (fs_rx, guard) = watch_any(sources_dir, opts.debounce)
            .map_err(|e: WatchError| WikiLiveError::Io(std::io::Error::other(e.to_string())))?;

        let (out_tx, out_rx) = std::sync::mpsc::channel::<SourceEvent>();
        let wiki = self.clone();
        std::thread::spawn(move || {
            // Drain bursts: any number of FS events ⇒ one
            // rescan. Block on the first event, then
            // non-blocking-drain anything that arrived
            // during the rescan.
            while let Ok(_first) = fs_rx.recv() {
                while fs_rx.try_recv().is_ok() {}
                let Ok(diff) = wiki.rescan_sources() else {
                    continue;
                };
                let mut enqueued = Vec::new();
                if opts.auto_enqueue {
                    for rel in diff.created.iter().chain(diff.modified.iter()) {
                        let kind = if diff.created.contains(rel) {
                            SourceChange::Created
                        } else {
                            SourceChange::Modified
                        };
                        let abs = wiki.wiki_root().join(rel);
                        let Ok(bytes) = std::fs::read(&abs) else {
                            continue;
                        };
                        if let Ok(task) = wiki.enqueue_ingest(rel, kind, &bytes) {
                            enqueued.push(task.id);
                        }
                    }
                }
                if out_tx
                    .send(SourceEvent {
                        diff,
                        enqueued_task_ids: enqueued,
                    })
                    .is_err()
                {
                    return;
                }
            }
        });

        // Drop `_first` would happen automatically; the
        // FS watcher keeps streaming until `guard` drops.
        let _ = FsEvent::Changed {
            abs_path: std::path::PathBuf::new(),
        };
        Ok((out_rx, SourceWatcherGuard { _inner: guard }))
    }
}
