//! `WorkstreamService` — wire surface for reading + mutating
//! workstreams, plus the derived rollup and the live event
//! stream. Same shape as the other entity services
//! (`MilestoneService` for the CRUD half, `TaskService` for the
//! `#[subscribe]` half).

use facet::Facet;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::model::Workstream;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet, Error)]
#[repr(u8)]
pub enum WorkstreamError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("io: {0}")]
    Io(String),
}

/// Member counts per canonical state group (mirrors
/// `project::states::StateGroup`). Statuses resolve through the
/// owning project's state registry, so a custom Completed state
/// (`shipped-to-client`) lands in `completed`, not in a
/// string-matched bucket. Always sums to the rollup's `total` —
/// enough for a segmented progress bar without fetching the
/// member tasks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Facet)]
pub struct StateGroupCounts {
    /// Not yet triaged into actionable work.
    pub backlog: u32,
    /// Actionable but not begun (also the fallback for status
    /// names absent from the registry).
    pub unstarted: u32,
    /// Claimed / in-flight (includes `waiting`).
    pub started: u32,
    /// Finished successfully.
    pub completed: u32,
    /// Closed without delivery.
    pub cancelled: u32,
}

/// Derived progress over the tasks attached to one workstream
/// (`task.workflow.workstream == id`). Never stored — computed
/// from the org's task list on demand. The estimate sum uses the
/// XS/S/M/L/XL → 1/2/3/5/8 bucket weights (`Points` counts at
/// face value); tasks with no estimate contribute 0.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Facet)]
pub struct WorkstreamRollup {
    /// Tasks attached to this workstream.
    pub total: u32,
    /// Attached tasks in the `completed` state group (registry
    /// aware — equals `groups.completed`).
    pub done: u32,
    /// Attached tasks in the `started` state group (registry
    /// aware — equals `groups.started`).
    pub in_progress: u32,
    /// Attached, still-open tasks with at least one unresolved
    /// blocker (a blocker is resolved only when it exists and is
    /// done / cancelled — same rule as `task issue ready`).
    pub blocked: u32,
    /// Sum of estimate points across attached tasks.
    pub estimate_points_sum: u32,
    /// Per-state-group member counts. Sums to `total`.
    pub groups: StateGroupCounts,
}

/// `rollup(id)` payload: the workstream plus its derived
/// progress in one round-trip, so UIs don't recompute the
/// rollup client-side.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
pub struct WorkstreamWithRollup {
    pub workstream: Workstream,
    pub rollup: WorkstreamRollup,
}

/// One workstream change, broadcast to every subscriber on each
/// successful mutation. Same fetch-once-then-fold contract as
/// `task::TaskEvent`:
///
/// - [`WorkstreamEvent::Upserted`] carries the full post-write
///   [`Workstream`] — replace (or insert) the row with that `id`.
/// - [`WorkstreamEvent::Deleted`] — remove the row with that `id`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(u8)]
// Upserted carries the full entity by design (idempotent
// full-state payloads) — same call as TaskEvent.
#[allow(clippy::large_enum_variant)]
pub enum WorkstreamEvent {
    /// Created or modified — the payload is the complete state
    /// after the write.
    Upserted(Workstream),
    /// The workstream with this id (and its backing file) was
    /// removed.
    Deleted(Uuid),
}

#[architect::rpc]
pub trait WorkstreamService {
    /// Workstreams under the org's vault, optionally restricted
    /// to one project.
    fn list(&self, project: Option<Uuid>) -> Result<Vec<Workstream>, WorkstreamError>;

    fn get(&self, id: Uuid) -> Result<Workstream, WorkstreamError>;

    fn get_by_path(&self, path: &str) -> Result<Workstream, WorkstreamError>;

    /// Create a workstream. `project_id` is required. Backend
    /// resolves the project's slug to derive the default `path`
    /// (`Projects/<slug>/workstreams/<ws-slug>.md`) when `path`
    /// is empty.
    fn create(&self, workstream: Workstream) -> Result<Workstream, WorkstreamError>;

    fn update(&self, workstream: Workstream) -> Result<Workstream, WorkstreamError>;

    /// Set the lifecycle status. Validates against the canonical
    /// [`crate::Status`] set (aliases accepted, stored canonical).
    fn set_status(&self, id: Uuid, status: &str) -> Result<Workstream, WorkstreamError>;

    /// Remove. Refuses if any task carries this workstream in
    /// `workflow.workstream` — detach those tasks first.
    fn delete(&self, id: Uuid) -> Result<(), WorkstreamError>;

    /// The workstream plus its derived progress (done / total /
    /// in-progress / blocked / estimate sum) over the org's
    /// tasks, computed server-side.
    fn rollup(&self, id: Uuid) -> Result<WorkstreamWithRollup, WorkstreamError>;

    /// Every workstream change, as it happens — fires on each
    /// successful create / update / `set_status` / delete.
    #[subscribe]
    fn events(&self) -> WorkstreamEvent;
}
