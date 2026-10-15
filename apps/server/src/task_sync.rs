//! Task files changed from outside the task service, announced as task
//! events.
//!
//! The task stream (`TaskService::events`) used to fire only for writes
//! the service made itself. A task is a vault file, though, and with
//! documents syncing between nodes (`offline.peers`) and the desktop
//! folder open to any editor (`offline.doc.local-folder`), most changes a
//! node sees arrive as a file the collab layer or someone else wrote.
//! This listener reads each changed `.md` file and hands it to
//! [`task::TaskBackend::notice_file`], which publishes it when it is a
//! task — so every list, the activity feed, the notifier and the
//! suggestion react to a task synced from a peer exactly as to one made
//! here. The sibling of [`crate::link_sync`], over the same broadcast.
//!
//! Each change handled is one span, `task_sync/notice`, carrying
//! `task_sync.change` (`put` | `delete`) and `task_sync.outcome`
//! (`upserted` | `deleted` | `not_a_task` | `unreadable`). Never the
//! path (docs/observability.md § 4).

use std::path::{Path, PathBuf};

use architect_telemetry::wide;
use tokio::sync::broadcast;
use tracing::Instrument;
use vault_proto::VaultEvent;

fn is_md(path: &str) -> bool {
    path.ends_with(".md")
}

// t[impl offline.peers] — a task file from any writer reaches the task stream
/// Subscribe to the vault's change broadcast and announce task files
/// written (`Put`) or removed (`Delete`) by anyone.
pub fn spawn(
    tasks: task::TaskBackend,
    vault_root: PathBuf,
    mut rx: broadcast::Receiver<VaultEvent>,
) {
    tokio::spawn(async move {
        loop {
            let (change, path) = match rx.recv().await {
                Ok(VaultEvent::Put { path, .. }) if is_md(&path) => ("put", path),
                Ok(VaultEvent::Delete { path }) if is_md(&path) => ("delete", path),
                Ok(_) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
                Err(broadcast::error::RecvError::Lagged(missed)) => {
                    // Changes dropped: a task edited in them is stale in
                    // every list until its next change or a refetch.
                    tracing::warn!(
                        missed,
                        "task_sync: lagged behind the vault, changes dropped"
                    );
                    continue;
                }
            };
            let span = tracing::info_span!("task_sync.notice", otel.name = "task_sync/notice");
            notice(&tasks, &vault_root, change, &path)
                .instrument(span)
                .await;
        }
    });
}

async fn notice(tasks: &task::TaskBackend, vault_root: &Path, change: &'static str, path: &str) {
    wide::set("task_sync.change", change);
    let outcome = if change == "put" {
        match tokio::fs::read_to_string(vault_root.join(path)).await {
            Ok(raw) => tasks.notice_file(path, &raw).as_str(),
            // Written and removed again before we read it.
            Err(_) => {
                tasks.notice_removed(path);
                "unreadable"
            }
        }
    } else {
        tasks.notice_removed(path).as_str()
    };
    wide::set("task_sync.outcome", outcome);
}
