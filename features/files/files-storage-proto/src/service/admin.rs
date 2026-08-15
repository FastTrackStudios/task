//! The operator lane: the deployment's Storage Location registry, agent
//! approval, and per-org grants. Mounted on the server lane, never on an
//! org router — the registry is deployment-scoped and orgs never own
//! locations (issue #230's resolution).
//!
//! # Authorization
//!
//! Every method takes `session_token` as its first argument and the
//! implementation validates it, exactly like the sibling services on
//! that lane (`OrgManagementService`, `IdentityService`): `/server/vox`
//! has no permission gate in front of it — the ws handler says so — so a
//! service mounted there that takes no credential is reachable by
//! anybody who can open the socket. That is not a theoretical hole for
//! this lane in particular: `issue_grant` admits an org to a volume with
//! an arbitrary quota and prefix, and `approve_agent` puts an agent into
//! the data path (PR #284 review).
//!
//! In-process transports (the embedded CLI) pass a `local_trusted`
//! backend which skips validation, again matching the sibling services.

use uuid::Uuid;

use crate::error::StorageError;
use crate::model::{AgentInfo, GrantSpec, StorageGrantInfo, StorageLocationInfo};

#[architect::rpc]
pub trait StorageAdminService {
    /// Every agent the coordinator knows, approved or not.
    async fn list_agents(&self, session_token: String) -> Result<Vec<AgentInfo>, StorageError>;

    /// Approve (or reject) an announced agent. Approving registers every
    /// volume it announced as a Storage Location — and brings its
    /// locations back `Online` if a previous rejection had taken them
    /// down. Rejecting leaves volumes unregistered, so nothing can ever
    /// be placed on them, and takes any existing locations offline
    /// rather than deleting placements: approval keeps a rogue agent out
    /// of the data path, it is not a delete button.
    async fn approve_agent(
        &self,
        session_token: String,
        agent_id: Uuid,
        approved: bool,
    ) -> Result<AgentInfo, StorageError>;

    /// Admit ONE of an approved agent's announced volumes into the
    /// registry — the granular half of [`StorageAdminService::approve_agent`],
    /// for a volume announced after approval (a drive plugged in later).
    /// Fails with [`StorageError::AgentNotApproved`] if the agent is
    /// pending.
    async fn register_location(
        &self,
        session_token: String,
        agent_id: Uuid,
        volume_key: String,
    ) -> Result<StorageLocationInfo, StorageError>;

    /// Every registered location in the deployment.
    async fn list_locations(
        &self,
        session_token: String,
    ) -> Result<Vec<StorageLocationInfo>, StorageError>;

    /// Admit an org onto a location. `spec.capabilities` must be a subset
    /// of the location's own; `spec.path_prefix` becomes the org's
    /// subtree there. Re-issuing for the same (org, location) replaces
    /// the grant's terms, keeping its id and measured usage.
    async fn issue_grant(
        &self,
        session_token: String,
        spec: GrantSpec,
    ) -> Result<StorageGrantInfo, StorageError>;

    /// Withdraw an org's admission. Existing placements are left on disk
    /// (a revoke never deletes data) but the org can no longer place or
    /// replicate onto the location.
    async fn revoke_grant(&self, session_token: String, grant_id: Uuid)
    -> Result<(), StorageError>;

    /// Grants across the deployment, optionally narrowed to one org slug.
    async fn list_grants(
        &self,
        session_token: String,
        org: Option<String>,
    ) -> Result<Vec<StorageGrantInfo>, StorageError>;
}
