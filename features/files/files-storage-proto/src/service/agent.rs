//! The storage-agent protocol (glossary "Storage agent"): one protocol,
//! three hostings. An agent announces its volumes, heartbeats their
//! health, subscribes to the directive stream, and reports each
//! directive's outcome. The in-server hosting speaks this same protocol
//! in-process; a desktop or standalone agent speaks it over vox.
//!
//! The coordinator plans and the agent transfers — the coordinator is
//! never itself the data path (issue #230), which is why directives carry
//! paths rather than bytes.
//!
//! # Identity
//!
//! An agent id is **public**: it is returned by `list_agents`, rides
//! every `StorageLocationInfo` (visible on the org lane), and appears in
//! every directive on the subscribe stream. So the id alone can never be
//! the credential — otherwise anyone could re-announce an approved agent
//! with a `root_path` of `/`, or complete another agent's directives
//! (PR #284 review). Enrollment mints a per-agent secret
//! ([`crate::model::AgentEnrollment::token`]), and every later call
//! presents it in an [`AgentCredential`]. The coordinator stores only a
//! hash.
//!
//! When device-key identity lands with the remote hostings (issue #265),
//! the credential becomes the key-proof and this shape stays: identity
//! is something the caller proves, not something it asserts.

use uuid::Uuid;

use crate::error::StorageError;
use crate::model::{
    AgentAnnouncement, AgentCredential, AgentDirective, AgentEnrollment, AgentInfo,
    DirectiveOutcome, VolumeHealth,
};

#[architect::rpc]
pub trait StorageAgentService {
    /// Announce (or re-announce) this agent and its volumes.
    ///
    /// A brand-new `agent_id` (no `token`) enrolls: the agent lands
    /// [`crate::model::AgentStatus::Pending`] — the operator approves
    /// before any of its volumes becomes a location — and the reply
    /// carries the secret it must present from then on. Re-announcing a
    /// known id REQUIRES that secret, and never resets an approval.
    async fn announce(
        &self,
        announcement: AgentAnnouncement,
    ) -> Result<AgentEnrollment, StorageError>;

    /// Report liveness plus per-volume health. `Offline` /
    /// `ExpectedOffline` propagate to the volume's registered location
    /// (a removable drive being unplugged is health, not error).
    async fn heartbeat(
        &self,
        credential: AgentCredential,
        volumes: Vec<VolumeHealth>,
    ) -> Result<AgentInfo, StorageError>;

    /// Directives still outstanding for this agent — the catch-up read it
    /// does on connect, before folding in the live stream.
    async fn pending_directives(
        &self,
        credential: AgentCredential,
    ) -> Result<Vec<AgentDirective>, StorageError>;

    /// Report a directive finished. This is what flips a placement from
    /// [`crate::model::PlacementStatus::Pending`] to `Hosted`, or records
    /// a replica's contents. A directive can only be completed by the
    /// agent it was issued to.
    async fn complete_directive(
        &self,
        credential: AgentCredential,
        directive_id: Uuid,
        outcome: DirectiveOutcome,
    ) -> Result<(), StorageError>;

    /// Every directive the coordinator issues. Directives carry their
    /// `agent_id`; a subscriber keeps its own and ignores the rest —
    /// the monorepo's `#[subscribe]`-stream idiom (root CLAUDE.md),
    /// not a per-agent channel. Directives name paths, never contents,
    /// so the stream itself carries nothing an agent could not already
    /// read from `list_locations`.
    #[subscribe]
    fn directives(&self) -> AgentDirective;
}
