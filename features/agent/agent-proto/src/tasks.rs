//! Agent tasks — dispatchable units of work owned by an agent.
//!
//! Modeled on hermes-webui's queue (`api/kanban_bridge.py`) but
//! with visualization-neutral naming: an `AgentTask` is a row in
//! a `Queue`, with a fixed lifecycle ([`AgentTaskStatus`]).
//! Kanban is one possible rendering (see `view-kanban`); timeline,
//! agenda, and graph views are future renderings of the same data.
//!
//! The dispatcher owns the `running` status — clients cannot set
//! it directly. Clients call `claim` (typically the server) and
//! the impl atomically flips status to `running` if the agent
//! task is free. This matches Hermes' contract and prevents two
//! workers double-claiming the same unit of work.
//!
//! ## Storage
//!
//! All four entities — [`Queue`], [`AgentTask`], [`AgentTaskLink`],
//! [`AgentTaskComment`] — are `#[derive(architect::Entity)]`. That
//! gets us, in one definition:
//!
//! - The wasm-clean wire struct (facet-derived).
//! - `<Name>Create` / `<Name>Update` / `<Name>List` payloads.
//! - The `<Name>Repo` `#[vox::service]` trait for CRUD over RPC.
//! - With `--features server`, the SeaORM `Model`/`Entity`/`Column`/
//!   `Relation`/`ActiveModel`, plus `<Name>RepoStorage<C>` glue.
//!
//! Domain-specific operations (claim, complete, set status — the
//! atomic / lifecycle-aware writes) live on the [`AgentTaskQueue`]
//! trait in `service::tasks`, implemented in the `agent-tasks`
//! crate against the architect-emitted storage.
//!
//! See the `agent-dispatch` crate for the full design.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Newtype around a list of label strings. Wrapped so we can
/// derive `architect::JsonField` on it — orphan rules forbid
/// implementing `From<Vec<String>>` for `sea_orm::Value`
/// directly, so the Vec lives behind a named struct.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField,
    ::facet::Facet,
    Serialize,
    Deserialize,
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
)]
#[repr(transparent)]
pub struct Labels(pub Vec<String>);

impl From<Vec<String>> for Labels {
    fn from(v: Vec<String>) -> Self {
        Self(v)
    }
}

impl From<Labels> for Vec<String> {
    fn from(v: Labels) -> Self {
        v.0
    }
}

/// Status lifecycle for an agent task. Kept as plain `&'static
/// str` constants + helpers rather than a sea-orm enum column —
/// SeaORM-mapped enums fight wasm builds and the gain is
/// marginal for a six-variant set. Storage stays a `String`.
pub mod status {
    pub const TRIAGE: &str = "triage";
    pub const READY: &str = "ready";
    pub const RUNNING: &str = "running";
    pub const BLOCKED: &str = "blocked";
    pub const DONE: &str = "done";
    pub const ARCHIVED: &str = "archived";

    /// All known statuses in lifecycle order. Useful for UI
    /// column ordering when something insists on grouping by
    /// status.
    pub const ALL: &[&str] = &[TRIAGE, READY, RUNNING, BLOCKED, DONE, ARCHIVED];

    /// `true` if a client UI is allowed to transition INTO this
    /// status directly. `running` returns `false` because the
    /// dispatcher owns the claim.
    #[must_use]
    pub fn client_settable(s: &str) -> bool {
        s != RUNNING
    }

    #[must_use]
    pub fn is_known(s: &str) -> bool {
        ALL.contains(&s)
    }
}

/// Kind discriminator for an [`AgentTaskLink`]. Stored as a
/// short string column.
pub mod link_kind {
    /// The `from` task blocks the `to` task — `to` can't move
    /// to `done` until `from` is `done`.
    pub const BLOCKS: &str = "blocks";
    /// Loose relationship — surfaced in the UI but not enforced.
    pub const RELATED: &str = "related";
    /// `to` was created as a follow-up to `from`.
    pub const SPAWNED_FROM: &str = "spawned_from";
}

// ────────────────────────────────────────────────────────────────────
// Queue — top-level grouping of agent tasks
// ────────────────────────────────────────────────────────────────────

/// A queue is a grouping of agent tasks — typically scoped to a
/// project. Each queue is one row in the `agent_queues` table.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, ::facet::Facet, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[architect(table_name = "agent_queues", repo)]
pub struct Queue {
    /// Stable slug. `"default"` is the canonical fallback.
    #[architect(primary_key, auto_increment = false)]
    pub id: String,
    #[architect(filterable, sortable, fulltext)]
    pub label: String,
    #[architect(exclude(create, update), on_create = Utc::now())]
    pub created_at: DateTime<Utc>,
    #[architect(exclude(create, update), on_create = Utc::now(), on_update = Utc::now())]
    pub updated_at: DateTime<Utc>,
}

/// A snapshot of a queue — agent tasks plus a watermark so
/// subscribers can detect drift. Not a stored entity; assembled
/// by the slim `AgentTaskQueue` trait.
#[derive(::facet::Facet, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[repr(C)]
pub struct QueueSnapshot {
    pub queue: Queue,
    pub tasks: Vec<AgentTask>,
    /// Latest `queue_events.id` observed when this snapshot
    /// was built. Subscribers pass it back on the event tail
    /// to resume without gaps.
    pub latest_event_id: u64,
    /// Whether the queue is read-only for the caller.
    pub read_only: bool,
}

// ────────────────────────────────────────────────────────────────────
// AgentTask — dispatchable unit of work
// ────────────────────────────────────────────────────────────────────

/// One dispatchable unit of agent work. One row in the
/// `agent_tasks` table.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, ::facet::Facet, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[architect(table_name = "agent_tasks", repo)]
pub struct AgentTask {
    #[architect(primary_key, auto_increment = false, on_create = Uuid::new_v4())]
    pub id: Uuid,

    /// FK to [`Queue::id`]. Stringly-typed because Queue uses a
    /// slug as its primary key.
    #[architect(filterable)]
    pub queue_id: String,

    #[architect(filterable, sortable, fulltext)]
    pub title: String,

    /// Markdown body — instructions to the agent.
    #[architect(fulltext)]
    pub description: String,

    /// One of [`status::ALL`]. Free-form so a backend can add
    /// finer states without a schema bump.
    #[architect(filterable, sortable)]
    pub status: String,

    /// 0 = highest, 4 = lowest. Matches Hermes' P0..P4.
    #[architect(filterable, sortable)]
    pub priority: i32,

    /// Free-form labels. Stored as a JSON column on the server.
    #[architect(json)]
    pub labels: Labels,

    /// Vault-relative path of the source task note, if this
    /// agent task was dispatched from one. Empty otherwise.
    #[architect(filterable)]
    pub source_task: String,

    /// Vault-relative path of the project note this agent task
    /// belongs to. Empty if standalone.
    #[architect(filterable)]
    pub project: String,

    /// Which backend should claim this. Empty = any.
    #[architect(filterable)]
    pub agent_profile: String,

    /// Worker handle currently holding the claim. Empty = free.
    pub claimed_by: String,

    /// When the claim expires. Dispatchers reclaim past this.
    pub claim_expires_at: Option<DateTime<Utc>>,

    /// PID of the worker process, when known. 0 = unknown.
    pub worker_pid: i32,

    /// Transcript / output blob attached at completion.
    /// Empty until `complete` is called.
    pub result_blob: String,

    /// Optional session this agent task is being worked in.
    /// Empty when no session is linked.
    pub linked_session_id: String,

    #[architect(exclude(create, update), on_create = Utc::now())]
    pub created_at: DateTime<Utc>,
    #[architect(exclude(create, update), on_create = Utc::now(), on_update = Utc::now())]
    pub updated_at: DateTime<Utc>,
    pub archived_at: Option<DateTime<Utc>>,
}

// ────────────────────────────────────────────────────────────────────
// AgentTaskLink — dependency / relationship between agent tasks
// ────────────────────────────────────────────────────────────────────

/// Edge between two agent tasks. Composite primary key is
/// `(from_task, to_task, kind)`.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::Entity, ::facet::Facet, Serialize, Deserialize, Clone, Debug, PartialEq, Eq,
)]
#[architect(table_name = "agent_task_links", repo)]
pub struct AgentTaskLink {
    /// Surrogate row id. The (from_task, to_task, kind) triple
    /// is enforced as unique at the migration level.
    #[architect(primary_key, auto_increment = false, on_create = Uuid::new_v4())]
    pub id: Uuid,
    #[architect(filterable)]
    pub from_task: Uuid,
    #[architect(filterable)]
    pub to_task: Uuid,
    /// One of [`link_kind`].
    #[architect(filterable)]
    pub kind: String,
    #[architect(exclude(create, update), on_create = Utc::now())]
    pub created_at: DateTime<Utc>,
}

// ────────────────────────────────────────────────────────────────────
// AgentTaskComment — append-only thread per agent task
// ────────────────────────────────────────────────────────────────────

#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::Entity, ::facet::Facet, Serialize, Deserialize, Clone, Debug, PartialEq, Eq,
)]
#[architect(table_name = "agent_task_comments", repo)]
pub struct AgentTaskComment {
    #[architect(primary_key, auto_increment = false, on_create = Uuid::new_v4())]
    pub id: Uuid,
    #[architect(filterable)]
    pub agent_task_id: Uuid,
    /// `"user"` for human comments, or the agent profile name
    /// for agent-emitted comments.
    #[architect(filterable)]
    pub author: String,
    pub body: String,
    #[architect(exclude(create, update), on_create = Utc::now())]
    pub created_at: DateTime<Utc>,
}

// ────────────────────────────────────────────────────────────────────
// Filter — read-side projection
// ────────────────────────────────────────────────────────────────────

/// Filter applied when reading a queue snapshot via the slim
/// `AgentTaskQueue::read_queue` op. The architect-emitted
/// `AgentTaskList` covers single-entity filtering; this carries
/// queue-wide flags that combine multiple architect filters
/// behind one call.
#[derive(::facet::Facet, Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[repr(C)]
pub struct QueueFilter {
    /// Restrict to a specific assignee. Empty = all.
    pub assignee: String,
    /// Include archived agent tasks.
    pub include_archived: bool,
    /// `"only_mine"` shortcut — caller's handle (resolved
    /// by the backend). Empty = no filter.
    pub only_handle: String,
    /// Restrict to agent tasks linked to a session.
    pub linked_session_id: String,
    /// Restrict to a specific agent profile. Empty = all.
    pub agent_profile: String,
}
