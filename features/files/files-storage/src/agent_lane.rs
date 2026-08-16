//! The agent lane's backend: the coordinator's side of the storage-agent
//! protocol. An agent — whichever of the three hostings it is — enrolls
//! here, heartbeats here, reads its outstanding directives here,
//! subscribes to new ones here, and reports outcomes here.
//!
//! The in-server hosting is not a special case of this protocol; it is
//! the same protocol with the round trip elided (see
//! [`StorageCore::register_local_agent`](crate::StorageCore::register_local_agent)),
//! which is exactly why a desktop or standalone agent can be added later
//! without the coordinator learning a second vocabulary.
//!
//! Identity is proved, not asserted: every method after enrollment takes
//! an [`AgentCredential`] whose secret the coordinator only ever stored a
//! hash of. See the trait's own module doc for why the id alone could
//! never do the job.

use std::sync::Arc;

use files_storage_proto::{
    AgentAnnouncement, AgentCredential, AgentDirective, AgentEnrollment, AgentInfo,
    DirectiveOutcome, StorageAgentService, StorageError, VolumeHealth,
};
use uuid::Uuid;

use crate::core::StorageCore;
use crate::error::panicked;

#[derive(Clone, architect::HasDispatcher)]
pub struct StorageAgentBackend {
    core: Arc<StorageCore>,
}

impl std::fmt::Debug for StorageAgentBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StorageAgentBackend")
            .finish_non_exhaustive()
    }
}

impl StorageAgentBackend {
    #[must_use]
    pub fn new(core: Arc<StorageCore>) -> Self {
        Self { core }
    }

    async fn run<T, F>(&self, f: F) -> Result<T, StorageError>
    where
        F: FnOnce(Arc<StorageCore>) -> Result<T, StorageError> + Send + 'static,
        T: Send + 'static,
    {
        let core = self.core.clone();
        files_store::blocking(move || f(core), panicked).await
    }
}

impl StorageAgentService for StorageAgentBackend {
    async fn announce(
        &self,
        announcement: AgentAnnouncement,
    ) -> Result<AgentEnrollment, StorageError> {
        self.run(move |core| core.announce(announcement)).await
    }

    async fn heartbeat(
        &self,
        credential: AgentCredential,
        volumes: Vec<VolumeHealth>,
    ) -> Result<AgentInfo, StorageError> {
        self.run(move |core| core.heartbeat(&credential, volumes))
            .await
    }

    async fn pending_directives(
        &self,
        credential: AgentCredential,
    ) -> Result<Vec<AgentDirective>, StorageError> {
        self.run(move |core| core.pending_directives(&credential))
            .await
    }

    async fn complete_directive(
        &self,
        credential: AgentCredential,
        directive_id: Uuid,
        outcome: DirectiveOutcome,
    ) -> Result<(), StorageError> {
        self.run(move |core| core.complete_directive(&credential, directive_id, outcome))
            .await
    }
}

/// The `#[subscribe]` backend contract for the directive stream. One hub
/// for every agent — directives carry their `agent_id` and each agent
/// keeps its own.
impl files_storage_proto::service::agent::StorageAgentServiceStreamSource for StorageAgentBackend {
    fn directives_hub(&self) -> &architect::PubSub<AgentDirective> {
        self.core.directives_hub()
    }
}
