//! Mounting a root as a filesystem — the cloud-folder half of the agent.
//!
//! `files-fuse` knows how to *be* a filesystem; it does not know how to
//! fetch anything, on purpose, so that it depends on no sync engine.
//! This is the join: a [`Fetch`] that turns "the kernel wants this
//! file's bytes" into the agent's own `hydrate`, and the bookkeeping
//! that keeps a mount alive and brings it back after a restart.
//!
//! # Why the agent mounts, rather than a separate program
//!
//! One process owns the store. A second one holding the same jj repo
//! would be a second writer to a thing whose locking assumes a single
//! one — and hydration *writes*: it materializes content into the live
//! tree. Mounting from inside the agent means the fetch a mount triggers
//! is the same call the CLI and the app make, through the same backend,
//! with no coordination to get wrong.
//!
//! # Linux only, for now
//!
//! macOS reaches the same behaviour through a File Provider extension,
//! which is not a crate: it is an app-bundle target the system loads,
//! asking *it* for material rather than being asked. The seam that makes
//! that possible is the same one used here — the agent's control socket
//! — so the Swift side is a client of `DaemonControlService` and none of
//! this file changes.

use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::daemon::SyncDaemon;
use crate::error::{DaemonError, Result};

/// The bridge from a kernel request to the agent's hydration.
///
/// Blocking by construction: FUSE calls it on a worker thread and the
/// caller is inside `open(2)`, which cannot be told to wait. The agent's
/// own `hydrate` is async, so this drives it on a runtime handle rather
/// than borrowing the caller's thread for an executor.
#[cfg(target_os = "linux")]
struct Fetch {
    daemon: SyncDaemon,
    root_id: Uuid,
    runtime: tokio::runtime::Handle,
}

#[cfg(target_os = "linux")]
impl files_fuse::Hydrator for Fetch {
    fn hydrate(&self, rel: &Path) -> std::result::Result<(), String> {
        let (daemon, root_id) = (self.daemon.clone(), self.root_id);
        let path = rel.to_string_lossy().into_owned();
        // `block_in_place` and not `block_on`: this thread belongs to
        // FUSE, not to tokio, so what is needed is a way to *wait* for
        // work the runtime does — which `Handle::block_on` provides —
        // without the runtime believing one of its own workers is
        // parked.
        self.runtime
            .block_on(async move { daemon.hydrate(root_id, path).await })
            .map_err(|e| e.to_string())
    }
}

/// Mount `root_id`'s live tree at `mountpoint`.
///
/// The mount lives until [`SyncDaemon::unmount`] or the process exits;
/// the session handle is kept by the daemon, and dropping it unmounts,
/// so an agent that stops does not leave a dead mount behind.
#[cfg(target_os = "linux")]
pub(crate) fn mount(
    daemon: &SyncDaemon,
    root_id: Uuid,
    tree: &Path,
    mountpoint: &Path,
) -> Result<fuser_session::Session> {
    clear_stale(mountpoint);
    std::fs::create_dir_all(mountpoint)
        .map_err(|e| DaemonError::Io(format!("creating {}: {e}", mountpoint.display())))?;

    let fetch = Fetch {
        daemon: daemon.clone(),
        root_id,
        runtime: tokio::runtime::Handle::current(),
    };
    let fs = files_fuse::LiveTree::new(tree, std::sync::Arc::new(fetch));
    let session = fs
        .mount(mountpoint)
        .map_err(|e| DaemonError::Io(format!("mounting at {}: {e}", mountpoint.display())))?;
    tracing::info!(
        %root_id,
        mountpoint = %mountpoint.display(),
        "mounted a root — dehydrated files fetch when something opens them"
    );
    Ok(session)
}

/// Take down a mount this agent no longer owns, if one is in the way.
///
/// An agent that is killed rather than stopped — `systemctl restart`
/// while a read is in flight, a crash, a power cut — leaves the kernel
/// holding a mount whose server is gone. Every call against it then
/// fails with `ENOTCONN`, which `fusermount3` reports as a permission
/// error, and the person who restarts the service to fix things gets
/// told they may not mount their own directory.
///
/// Nothing is destroyed by unmounting one of these: the tree it was a
/// window onto is untouched, and a live mount of ours never reaches
/// here — the daemon rejects a second mount of the same root before
/// this runs.
#[cfg(target_os = "linux")]
fn clear_stale(mountpoint: &Path) {
    // `stat` is the probe: on a live mount it succeeds, on an abandoned
    // one the kernel answers for the missing server.
    match std::fs::metadata(mountpoint) {
        Ok(_) => return,
        // Not there at all, which `create_dir_all` handles.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            tracing::info!(
                at = %mountpoint.display(),
                error = %e,
                "the mountpoint answers for a mount whose agent is gone — taking it down"
            );
        }
    }
    let out = std::process::Command::new("fusermount3")
        .arg("-uz")
        .arg(mountpoint)
        .output();
    match out {
        Ok(o) if o.status.success() => {
            tracing::info!(at = %mountpoint.display(), "cleared the stale mount");
        }
        Ok(o) => tracing::warn!(
            at = %mountpoint.display(),
            stderr = %String::from_utf8_lossy(&o.stderr).trim(),
            "could not clear the stale mount"
        ),
        Err(e) => tracing::warn!(error = %e, "could not run fusermount3 to clear a stale mount"),
    }
}

/// The same call on a platform with no FUSE.
#[cfg(not(target_os = "linux"))]
pub(crate) fn mount(
    _daemon: &SyncDaemon,
    _root_id: Uuid,
    _tree: &Path,
    _mountpoint: &Path,
) -> Result<fuser_session::Session> {
    Err(DaemonError::BadRequest(format!(
        "mounting a root as a filesystem is Linux-only here; on {} it is the \
         File Provider extension's job, which the app installs rather than the agent",
        std::env::consts::OS
    )))
}

/// The live session type, or a placeholder where there is none.
#[cfg(target_os = "linux")]
pub(crate) mod fuser_session {
    pub type Session = files_fuse::BackgroundSession;
}

#[cfg(not(target_os = "linux"))]
pub(crate) mod fuser_session {
    /// Uninhabited-in-practice stand-in: `mount` never returns one off
    /// Linux, so this exists only to give the signature a type.
    pub struct Session;
}

/// What the agent remembers about the roots it has mounted.
///
/// Persisted for the same reason the sync choices are: a mount is a
/// decision somebody made, and a background service that forgets it on
/// reboot leaves a person staring at an empty directory where their
/// project was.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct Mounts {
    pub(crate) at: Vec<Mounted>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct Mounted {
    pub(crate) root_id: Uuid,
    pub(crate) mountpoint: PathBuf,
}
