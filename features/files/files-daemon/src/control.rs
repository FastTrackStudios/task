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

    async fn share(
        &self,
        path: String,
        name: Option<String>,
    ) -> Result<(Uuid, String), DaemonError> {
        let root = self.daemon.share(std::path::Path::new(&path), name).await?;
        Ok((root.id, root.name))
    }

    async fn pull_all(
        &self,
        endpoint_id: String,
        under: String,
    ) -> Result<Vec<String>, DaemonError> {
        self.daemon
            .pull_all(&endpoint_id, std::path::Path::new(&under))
            .await
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

    async fn checkpoint_now(&self, root_id: Uuid) -> Result<(), DaemonError> {
        self.daemon.checkpoint_now(root_id).await
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
