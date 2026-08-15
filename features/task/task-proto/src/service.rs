//! `TaskService` — wire surface for reading + mutating tasks.
//!
//! Same shape as `ProjectService` / `GoalService` / `LocationsService`:
//! `create` / `update` take the full record, `rename` is the
//! only path-changing op, `delete` removes the file.
//!
//! `list()` is intentionally unfiltered (full org snapshot —
//! the subscriber contract on [`TaskEvent`] depends on it);
//! [`TaskService::query`] is the server-side filtered /
//! paginated sibling for clients that don't want the whole
//! list. The CLI layers further client-side filters (`--tag`,
//! `--context`, `--milestone`) on top.

use facet::Facet;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::model::TaskInfo;
use crate::relations::ReverseRelation;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet, Error)]
#[repr(u8)]
pub enum TaskError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("io: {0}")]
    Io(String),
}

/// Outcome of an atomic [`TaskService::try_claim`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(u8)]
pub enum ClaimResult {
    /// The caller now holds the claim (was unclaimed, or stolen).
    Won,
    /// The caller already held it — no-op.
    AlreadyMine,
    /// Another actor holds it; `holder` is their short label.
    Lost { holder: String },
}

/// Server-side filter + page window for [`TaskService::query`].
/// All fields optional — the zero value selects everything, in
/// which case `query` equals `list`. Filters AND together;
/// `limit`/`offset` apply *after* filtering, over a stable
/// path-ordered view, so pages don't shear between calls when
/// the vault is quiet.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Facet)]
pub struct TaskListFilter {
    /// Keep tasks whose `project_id` matches.
    pub project: Option<Uuid>,
    /// Keep tasks attached to this workstream
    /// (`workflow.workstream`).
    pub workstream: Option<Uuid>,
    /// Keep tasks whose raw `status` matches
    /// (ASCII-case-insensitive). Raw string match — state-group
    /// classification stays a rollup concern.
    pub status: Option<String>,
    /// Keep only open (non-terminal) tasks. Terminal status
    /// classification is the domain's — see [`crate::status_is_open`]
    /// — so callers don't hand-roll (and drift on) the done set.
    #[serde(default)]
    pub open_only: bool,
    /// Keep tasks due on or before this date (ISO `YYYY-MM-DD`),
    /// counting either the hard `due` or the soft `scheduled` date —
    /// see [`crate::is_due_on_or_before`]. Powers agenda / brief
    /// "what's due today" queries.
    #[serde(default)]
    pub due_on_or_before: Option<String>,
    /// Contextual relevance (see [`crate::relevance`]): keep only
    /// tasks relevant under this context, then rank active-project /
    /// due-today rows first (rank sorts within the stable path
    /// order). Combine with [`Self::open_only`] for the canonical
    /// "Active + Relevant" view.
    #[serde(default)]
    pub relevance: Option<crate::relevance::RelevanceContext>,
    /// Page size. `None` = no cap.
    pub limit: Option<u32>,
    /// Rows to skip (after filtering). `None` = 0.
    pub offset: Option<u32>,
}

/// One entry of [`TaskService::reverse_relations_batch`]: the
/// queried task id and its incoming edges. A list of pairs
/// rather than a map for wire/codegen friendliness (TS gets a
/// plain array).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
pub struct TaskReverseRelations {
    /// The task whose incoming edges these are.
    pub id: Uuid,
    /// Incoming edges (who points at `id`, with what kind).
    /// Empty when nothing references the task.
    pub relations: Vec<ReverseRelation>,
}

/// One task change, broadcast to every [`TaskService`] subscriber on
/// each successful mutation.
///
/// ## Subscriber contract (no snapshot variant, v1)
///
/// The stream carries *changes only* — there is no `Snapshot`
/// variant. A subscriber that wants the full board state fetches it
/// once via [`TaskService::list`] (after subscribing, so nothing is
/// missed in between) and then folds events into that local copy:
///
/// - [`TaskEvent::Upserted`] carries the **full post-write**
///   [`TaskInfo`] — replace (or insert) the row with a matching `id`.
///   Re-applying an event already reflected in the fetched list is
///   harmless (idempotent re-application).
/// - [`TaskEvent::Deleted`] — remove the row with that `id`.
///
/// `Upserted` fires for every write path: create, update, rename
/// (the new `path` is in the payload), and claim (assignees changed).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(u8)]
// Upserted carries the full ~600-byte TaskInfo by design (idempotent
// full-state payloads); boxing would complicate the facet wire shape
// for a clone-heavy but allocation-tolerant path.
#[allow(clippy::large_enum_variant)]
pub enum TaskEvent {
    /// A task was created or modified — the payload is the complete
    /// state after the write.
    Upserted(TaskInfo),
    /// The task with this id (and its backing file) was removed.
    Deleted(Uuid),
}

#[architect::rpc]
pub trait TaskService {
    /// Every task page under the org's vault.
    fn list(&self) -> Result<Vec<TaskInfo>, TaskError>;

    fn get(&self, id: Uuid) -> Result<TaskInfo, TaskError>;

    fn get_by_path(&self, path: &str) -> Result<TaskInfo, TaskError>;

    /// Create a task. Backend assigns `task.path`
    /// (default `tasks/<slug>.md`) and `task.id` when nil.
    fn create(&self, task: TaskInfo) -> Result<TaskInfo, TaskError>;

    /// Replace the task whose `id` matches. Path mutations
    /// ignored — use [`Self::rename`].
    fn update(&self, task: TaskInfo) -> Result<TaskInfo, TaskError>;

    /// Atomically claim a task for an agent. The backend
    /// serializes the read-check-write under a process lock, so
    /// concurrent callers can't both win the same task — exactly
    /// one gets [`ClaimResult::Won`]. `agent` is a JSON-encoded
    /// `workflows_proto::AgentRef`. `force` steals an existing
    /// claim. This is the parallel-agent primitive: no TOCTOU
    /// window like the client-side optimistic version.
    fn try_claim(&self, id: Uuid, agent: String, force: bool) -> Result<ClaimResult, TaskError>;

    /// Reverse (incoming) relation edges of one task — "what
    /// blocks / duplicates / implements THIS", with blocked-by
    /// merged from both encodings (typed `relations` on other
    /// tasks targeting `id`, plus legacy `blockers` lists). The
    /// backend builds the in-memory reverse index from the same
    /// task list `list()` serves (see `crate::relations`).
    fn reverse_relations(&self, id: Uuid) -> Result<Vec<ReverseRelation>, TaskError>;

    /// Batch sibling of [`Self::reverse_relations`]: the reverse
    /// index for many tasks in one round-trip, so list rows can
    /// show blocked-by / implemented-by without N calls. One
    /// entry per requested id, requested order, missing /
    /// unknown ids included with empty `relations` (a task
    /// nothing references is indistinguishable from one that
    /// doesn't exist — both are "no incoming edges").
    fn reverse_relations_batch(
        &self,
        ids: Vec<Uuid>,
    ) -> Result<Vec<TaskReverseRelations>, TaskError>;

    /// Server-side filtered + paginated [`Self::list`]. The
    /// default [`TaskListFilter`] returns everything; see the
    /// filter type for AND semantics and page-window ordering.
    fn query(&self, filter: TaskListFilter) -> Result<Vec<TaskInfo>, TaskError>;

    /// Move the backing markdown file. `id` preserved.
    fn rename(&self, id: Uuid, new_path: &str) -> Result<TaskInfo, TaskError>;

    /// Remove the backing file.
    fn delete(&self, id: Uuid) -> Result<(), TaskError>;

    /// Every task change, as it happens — fires on each successful
    /// create / update / rename / claim / delete. See [`TaskEvent`]
    /// for the fetch-once-then-fold subscriber contract.
    #[subscribe]
    fn events(&self) -> TaskEvent;
}
