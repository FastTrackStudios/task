//! Wire types for the Files placement layer (issue #262). Vocabulary is
//! the Task glossary (`apps/task/CONTEXT.md`):
//!
//! - **Storage Location** — a named place Files can live, deployment-scoped
//!   (the operator registers them; orgs reach them only through grants),
//!   declaring its capability classes and spoken for by exactly one agent.
//! - **Storage grant** — an org's admission onto a location: a capability
//!   subset, a logical-byte quota, and a path prefix that is the org's own
//!   subtree on that (possibly shared) volume.
//! - **Storage agent** — the process that speaks for a location. One
//!   protocol, three hostings; agents announce their volumes, the operator
//!   approves.
//!
//! Placement is two axes, deliberately independent (ADR 0001): a root's
//! **live tree** sits wholly on ONE location (whose agent owns the
//! authoritative repo), while its version-store **blobs** may be replicated
//! onto any number of blob-capable locations.

use chrono::{DateTime, Utc};
use facet::Facet;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// What a Storage Location is able to hold (glossary "Storage Location":
/// "Each location declares its capability classes"). A grant may name a
/// subset of its location's classes, never a superset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Facet)]
#[repr(u8)]
pub enum CapabilityClass {
    /// A POSIX volume that can host a root's live tree (and, with it,
    /// the authoritative version-store repo).
    LiveTrees,
    /// Get/put blob storage — version-store blobs and archive copies,
    /// never a live tree.
    Blobs,
}

/// The physical flavour of a location. Only shapes policy (removable
/// locations are replica-first, expected-offline is health rather than
/// error — issue #275); it never widens what a grant permits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Facet)]
#[repr(u8)]
pub enum LocationKind {
    /// A volume on a server the deployment owns.
    ServerVolume,
    /// A drive that comes and goes with a workstation (issue #275).
    Removable,
    /// Object storage (S3-compatible) — blobs only.
    ObjectStore,
}

/// Health of a location, as its agent last reported it. `ExpectedOffline`
/// is the removable-drive state: unplugged is health, not error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Facet)]
#[repr(u8)]
pub enum LocationHealth {
    Online,
    Offline,
    ExpectedOffline,
}

/// Which of the three hostings a storage agent is (glossary "Storage
/// agent"). Only [`AgentHosting::InServer`] is implemented in this
/// ticket; the other two are the same protocol reached over the wire
/// (issues #265, #275).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Facet)]
#[repr(u8)]
pub enum AgentHosting {
    /// In-process in task-server, speaking for the server's own volumes.
    InServer,
    /// The desktop app's headless agent (removable drives, replicas).
    DesktopApp,
    /// A standalone agent on a NAS / storage box.
    Standalone,
}

/// Where an announced agent stands with the operator. A `Pending` (or
/// `Rejected`) agent's volumes are NOT registered as locations, so
/// nothing can be placed on them — that is what approval is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Facet)]
#[repr(u8)]
pub enum AgentStatus {
    Pending,
    Approved,
    Rejected,
}

/// One volume an agent offers when it announces itself. `key` is the
/// agent's own stable name for the volume — re-announcing the same
/// `(agent_id, key)` updates the announcement rather than duplicating it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct AnnouncedVolume {
    pub key: String,
    pub name: String,
    pub kind: LocationKind,
    /// The volume's root directory, as the *agent* sees it. Every grant's
    /// path prefix is relative to this.
    pub root_path: String,
    pub capabilities: Vec<CapabilityClass>,
    pub capacity_bytes: Option<u64>,
}

/// A storage agent announcing itself and its volumes to the coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct AgentAnnouncement {
    /// Stable per-agent id. Re-announcing under the same id updates the
    /// existing record and never resets an approval — which is exactly
    /// why it cannot be the only thing a re-announcement presents: ids
    /// are published by `list_agents` and ride every
    /// `StorageLocationInfo`, so an id alone would let anyone rewrite an
    /// approved agent's volume list (PR #284 review).
    pub agent_id: Uuid,
    /// The agent's enrollment secret, required whenever `agent_id` is
    /// already known. `None` enrolls a NEW agent, and the coordinator
    /// mints the secret in its [`AgentEnrollment`] reply — the one time
    /// it is ever transmitted.
    pub token: Option<String>,
    pub hosting: AgentHosting,
    pub label: String,
    pub volumes: Vec<AnnouncedVolume>,
}

/// What an agent proves it is on every call after enrollment. The token
/// is a per-agent secret; the id alone is public.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct AgentCredential {
    pub agent_id: Uuid,
    pub token: String,
}

/// The reply to an announcement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct AgentEnrollment {
    pub agent: AgentInfo,
    /// Set only when this announcement enrolled a NEW agent: the secret
    /// it must present from now on. The coordinator keeps only a hash,
    /// so an agent that loses this must be re-enrolled by the operator.
    pub token: Option<String>,
}

/// A known storage agent, as the coordinator sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct AgentInfo {
    pub id: Uuid,
    pub hosting: AgentHosting,
    pub label: String,
    pub status: AgentStatus,
    pub volumes: Vec<AnnouncedVolume>,
    pub last_seen: DateTime<Utc>,
}

/// A registered Storage Location — an approved agent's volume, admitted
/// into the deployment's registry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct StorageLocationInfo {
    pub id: Uuid,
    pub name: String,
    pub kind: LocationKind,
    /// The single agent that speaks for this location.
    pub agent_id: Uuid,
    /// That agent's own key for the underlying volume.
    pub volume_key: String,
    pub root_path: String,
    pub capabilities: Vec<CapabilityClass>,
    pub capacity_bytes: Option<u64>,
    pub health: LocationHealth,
    pub registered_at: DateTime<Utc>,
}

/// Operator input for [`crate::service::StorageAdminService::issue_grant`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct GrantSpec {
    /// Org slug — grants are per (org, location).
    pub org: String,
    pub location_id: Uuid,
    /// Must be a subset of the location's own capabilities.
    pub capabilities: Vec<CapabilityClass>,
    /// Logical bytes — the bytes this org's roots reference on this
    /// location, dedup savings belonging to the operator (issue #230).
    pub quota_bytes: u64,
    /// The org's own subtree under the location's `root_path`. Relative,
    /// never escaping; everything the org places lives inside it.
    pub path_prefix: String,
}

/// An org's admission onto a location.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct StorageGrantInfo {
    pub id: Uuid,
    pub org: String,
    pub location_id: Uuid,
    pub capabilities: Vec<CapabilityClass>,
    pub quota_bytes: u64,
    /// Logical bytes currently charged against this grant — the sum over
    /// the org's placements on this location, refreshed by
    /// [`crate::service::StorageService::refresh_usage`].
    pub used_bytes: u64,
    pub path_prefix: String,
    pub granted_at: DateTime<Utc>,
}

/// Where a root's live tree is bound. One root, one location — the agent
/// hosting it owns the authoritative repo (ADR 0001).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct LiveTreeBinding {
    pub location_id: Uuid,
    /// Path under the grant's prefix, chosen by the org.
    pub relative_path: String,
    /// `<location root>/<grant prefix>/<relative path>` — the live tree
    /// as the hosting agent sees it.
    pub absolute_path: String,
    /// True once the hosting agent has created the tree and initialized
    /// the authoritative version-store repo inside it.
    pub repo_initialized: bool,
}

/// One blob replica of a root's version store on a blob-capable
/// location. Placement is a separate axis from the live tree: the same
/// root may have replicas on locations that could never host it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct BlobReplica {
    pub location_id: Uuid,
    /// Blob-store directory under the grant's prefix on that location.
    pub absolute_path: String,
    /// Distinct version-store files present in the replica as of
    /// `synced_at`.
    pub files_present: u64,
    /// Logical bytes those files represent.
    pub logical_bytes: u64,
    pub synced_at: Option<DateTime<Utc>>,
}

/// How far a placement has got. In-server hosting completes inline, so a
/// placement on a server volume is `Hosted` by the time `place_root`
/// returns; a remote agent's placement stays `Pending` until it reports
/// back through
/// [`crate::service::StorageAgentService::complete_directive`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Facet)]
#[repr(u8)]
pub enum PlacementStatus {
    Pending,
    Hosted,
    Failed,
}

/// A root's placement across both axes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct RootPlacement {
    pub root_id: Uuid,
    pub org: String,
    pub status: PlacementStatus,
    /// `None` until a live tree is placed (a root may exist as blobs
    /// only — an archived root whose live tree was released).
    pub live_tree: Option<LiveTreeBinding>,
    /// Logical bytes the root's live tree references, as of the last
    /// [`crate::service::StorageService::refresh_usage`].
    pub logical_bytes: u64,
    pub replicas: Vec<BlobReplica>,
    /// Set when `status` is [`PlacementStatus::Failed`].
    pub failure: Option<String>,
}

/// An org's quota position on one location.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct GrantUsage {
    pub location_id: Uuid,
    pub quota_bytes: u64,
    pub used_bytes: u64,
    /// Placements (live trees + replicas) this org holds on the location.
    pub placements: u32,
}

/// One volume's health, as reported by its agent's heartbeat.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct VolumeHealth {
    pub volume_key: String,
    pub health: LocationHealth,
}

/// A path an agent is asked to create, expressed as the boundary it may
/// not leave plus the path inside it.
///
/// Both halves travel together because confinement has to happen where
/// the filesystem is — at the agent, before the first `mkdir`. A
/// directive carrying only a resolved absolute path would leave a remote
/// hosting no way to enforce the grant's prefix at all, and would leave
/// the coordinator checking after the writes had already landed (PR #284
/// review).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct ConfinedPath {
    /// The org's granted subtree on this location: `<location
    /// root>/<grant prefix>`. The agent creates it if missing and must
    /// refuse anything that resolves outside it.
    pub boundary: String,
    /// Path under `boundary`, relative and `..`-free.
    pub relative: String,
}

/// Work the coordinator hands to an agent. Directives carry their
/// `agent_id` and subscribers filter client-side — the monorepo's
/// `#[subscribe]`-stream idiom (root CLAUDE.md), not a per-agent channel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct AgentDirective {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub kind: DirectiveKind,
}

/// The two placement jobs an agent carries out. Both are data movement:
/// the coordinator plans, the agent transfers — it is never the data path
/// itself (issue #230).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(u8)]
pub enum DirectiveKind {
    /// Create `target` — confined to its boundary — and initialize the
    /// authoritative version-store repo inside it.
    HostLiveTree {
        root_id: Uuid,
        org: String,
        target: ConfinedPath,
    },
    /// Copy every version-store blob the root's live tree references
    /// from `source_path` (a live tree the agent already hosts) into
    /// `dest` (a blob store, confined to the destination grant's
    /// prefix).
    ReplicateBlobs {
        root_id: Uuid,
        org: String,
        source_path: String,
        dest: ConfinedPath,
    },
    /// Re-measure the logical bytes the root's live tree references.
    /// Quota is charged in logical bytes, and only the agent holding the
    /// authoritative repo can count them.
    MeasureLiveTree {
        root_id: Uuid,
        org: String,
        live_tree_path: String,
    },
}

/// What an agent reports back when a directive finishes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(u8)]
pub enum DirectiveOutcome {
    Hosted {
        repo_initialized: bool,
        /// The path the agent actually created, after its own
        /// confinement check — this, not the coordinator's guess, is
        /// what the placement records.
        absolute_path: String,
    },
    Replicated {
        files_present: u64,
        logical_bytes: u64,
        absolute_path: String,
    },
    Measured {
        files: u64,
        logical_bytes: u64,
    },
    Failed {
        reason: String,
    },
}
