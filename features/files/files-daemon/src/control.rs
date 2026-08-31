//! [`DaemonControl`] — the [`SyncDaemon`] wrapped as a
//! [`DaemonControlService`] server. This is what binds to the local
//! socket; the desktop app and CLI establish a client against it.

use uuid::Uuid;

use crate::daemon::SyncDaemon;
use crate::model::DaemonStatus;
use crate::service::{DaemonControlService, DaemonError, DaemonEvent};

/// The control server over a daemon. Cheap to clone (the daemon is an
/// `Arc` inside).
#[derive(Clone, architect::HasDispatcher)]
pub struct DaemonControl {
    daemon: SyncDaemon,
}

impl std::fmt::Debug for DaemonControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DaemonControl").finish_non_exhaustive()
    }
}

impl DaemonControl {
    #[must_use]
    pub fn new(daemon: SyncDaemon) -> Self {
        Self { daemon }
    }

    /// The daemon this control surface drives — for the caller that
    /// also runs the tick loop.
    #[must_use]
    pub fn daemon(&self) -> &SyncDaemon {
        &self.daemon
    }
}

impl DaemonControlService for DaemonControl {
    async fn status(&self) -> Result<DaemonStatus, DaemonError> {
        Ok(self.daemon.status())
    }

    async fn set_sync_choice(
        &self,
        root_id: Uuid,
        slice: Vec<String>,
    ) -> Result<DaemonStatus, DaemonError> {
        self.daemon.choose_root(root_id, slice).await
    }

    async fn remove_sync_choice(&self, root_id: Uuid) -> Result<DaemonStatus, DaemonError> {
        self.daemon.remove_sync_choice(root_id);
        Ok(self.daemon.status())
    }

    async fn shares(&self) -> Result<Vec<(Uuid, String, String)>, DaemonError> {
        Ok(self
            .daemon
            .shares()
            .await?
            .into_iter()
            // A root with no path on *this* host is one whose structure
            // arrived but whose tree lives elsewhere — real, and worth
            // listing as such rather than hiding.
            .map(|r| (r.id, r.name, r.path.unwrap_or_default()))
            .collect())
    }

    async fn placed_roots(&self) -> Result<Vec<crate::service::PlacedRoot>, DaemonError> {
        Ok(self
            .daemon
            .shares()
            .await?
            .into_iter()
            .map(|r| {
                let place = self.daemon.place_of(r.id, &r.name);
                crate::service::PlacedRoot {
                    made_by: self.daemon.maker_of(r.id),
                    id: r.id,
                    name: r.name,
                    path: r.path.unwrap_or_default(),
                    place,
                }
            })
            .collect())
    }

    async fn share_deferred(
        &self,
        path: String,
        name: Option<String>,
    ) -> Result<files_proto::model::FileRootInfo, DaemonError> {
        self.daemon
            .share_capturing(std::path::Path::new(&path), name, false)
            .await
    }

    async fn start_capture(&self) -> Result<u32, DaemonError> {
        self.daemon.start_capture_backlog()
    }

    async fn share(
        &self,
        path: String,
        name: Option<String>,
    ) -> Result<(Uuid, String), DaemonError> {
        let root = self.daemon.share(std::path::Path::new(&path), name).await?;
        Ok((root.id, root.name))
    }

    async fn unshare(&self, root_id: Uuid) -> Result<(), DaemonError> {
        self.daemon.unshare(root_id)
    }

    async fn pull_all(
        &self,
        endpoint_id: String,
        under: String,
    ) -> Result<Vec<crate::service::Pulled>, DaemonError> {
        // An empty `under` means "wherever this agent keeps its roots",
        // which is what a caller wants unless it has a reason not to —
        // and saves it from reconstructing a path the agent already
        // knows.
        let under = if under.trim().is_empty() {
            self.daemon.roots_dir()
        } else {
            std::path::PathBuf::from(under)
        };
        self.daemon.pull_all(&endpoint_id, &under).await
    }

    async fn set_coordinator(&self, endpoint_id: String) -> Result<DaemonStatus, DaemonError> {
        self.daemon.set_coordinator_peer(&endpoint_id).await?;
        // Admitted in return: syncing *with* an org means it pulls this
        // machine too, and a coordinator this machine will not answer
        // can never collect what it did offline.
        self.daemon.admit_peer(&endpoint_id);
        Ok(self.daemon.status())
    }

    async fn remember_peer(&self, endpoint_id: String) -> Result<(), DaemonError> {
        self.daemon.remember_peer(&endpoint_id);
        Ok(())
    }

    async fn admit_peer(&self, endpoint_id: String) -> Result<DaemonStatus, DaemonError> {
        self.daemon.admit_peer(&endpoint_id);
        Ok(self.daemon.status())
    }

    async fn dismiss_peer(&self, endpoint_id: String) -> Result<DaemonStatus, DaemonError> {
        self.daemon.dismiss_peer(&endpoint_id);
        Ok(self.daemon.status())
    }

    async fn peer_roots(&self, endpoint_id: String) -> Result<Vec<(Uuid, String)>, DaemonError> {
        Ok(self
            .daemon
            .peer_roots(&endpoint_id)
            .await?
            .into_iter()
            .map(|r| (r.id, r.name))
            .collect())
    }

    async fn sync_from_peer(
        &self,
        endpoint_id: String,
        root_id: Uuid,
        slice: Vec<String>,
        under: String,
    ) -> Result<DaemonStatus, DaemonError> {
        self.daemon
            .sync_from_peer(&endpoint_id, root_id, slice, std::path::Path::new(&under))
            .await
    }

    async fn pause(&self, root_id: Option<Uuid>) -> Result<DaemonStatus, DaemonError> {
        self.daemon.pause(root_id);
        Ok(self.daemon.status())
    }

    async fn resume(&self, root_id: Option<Uuid>) -> Result<DaemonStatus, DaemonError> {
        self.daemon.resume(root_id);
        Ok(self.daemon.status())
    }

    async fn hydrate(&self, root_id: Uuid, path: String) -> Result<(), DaemonError> {
        self.daemon.hydrate(root_id, path).await
    }

    async fn keep_only(
        &self,
        root_id: Uuid,
        patterns: Vec<String>,
    ) -> Result<crate::service::KeptReport, DaemonError> {
        self.daemon.keep_only(root_id, patterns).await
    }

    async fn kept(&self, root_id: Uuid) -> Result<Vec<String>, DaemonError> {
        self.daemon.kept(root_id).await
    }

    async fn dehydrate(&self, root_id: Uuid, path: String) -> Result<(), DaemonError> {
        self.daemon.dehydrate(root_id, path).await
    }

    async fn keep_both(&self, root_id: Uuid, path: String) -> Result<(), DaemonError> {
        self.daemon.keep_both(root_id, path).await
    }

    async fn checkpoint_now(&self, root_id: Uuid) -> Result<(), DaemonError> {
        self.daemon.checkpoint_now(root_id).await
    }

    async fn mount(&self, root_id: Uuid, mountpoint: String) -> Result<(), DaemonError> {
        self.daemon
            .mount(root_id, std::path::Path::new(&mountpoint))
            .await
    }

    async fn unmount(&self, root_id: Uuid) -> Result<(), DaemonError> {
        self.daemon.unmount(root_id)
    }

    async fn set_place(&self, root_id: Uuid, place: String) -> Result<(), DaemonError> {
        self.daemon.set_place(root_id, &place)
    }

    async fn mount_all(
        &self,
        under: String,
        flat: bool,
    ) -> Result<Vec<(String, Option<String>)>, DaemonError> {
        Ok(self
            .daemon
            .mount_all(std::path::Path::new(&under), flat)
            .await)
    }

    async fn mounts(&self) -> Result<Vec<(Uuid, String)>, DaemonError> {
        Ok(self
            .daemon
            .mounts()
            .into_iter()
            .map(|(id, at)| (id, at.to_string_lossy().into_owned()))
            .collect())
    }
}

/// The `#[subscribe]` contract: hand the stream host the hub. The
/// daemon publishes a fresh snapshot on every state change (a pull
/// starting/finishing, a choice or pause toggling).
impl crate::service::DaemonControlServiceStreamSource for DaemonControl {
    fn status_events_hub(&self) -> &architect::PubSub<DaemonEvent> {
        self.daemon.events().pubsub()
    }
}
