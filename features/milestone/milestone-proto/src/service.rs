//! `MilestoneService` — wire surface for reading + mutating
//! project milestones. Same shape as the other entity services.

use facet::Facet;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::model::Milestone;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet, Error)]
#[repr(u8)]
pub enum MilestoneError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("io: {0}")]
    Io(String),
}

/// One milestone change, broadcast to every [`MilestoneService`]
/// subscriber on each successful mutation.
///
/// ## Subscriber contract (no snapshot variant, v1)
///
/// The stream carries *changes only* — there is no `Snapshot`
/// variant. A subscriber that wants the full state fetches it once
/// via [`MilestoneService::list`] (after subscribing, so nothing is
/// missed in between) and then folds events into that local copy:
///
/// - [`MilestoneEvent::Upserted`] carries the **full post-write**
///   [`Milestone`] — replace (or insert) the row with a matching
///   `id`. Re-applying an event already reflected in the fetched
///   list is harmless (idempotent re-application).
/// - [`MilestoneEvent::Deleted`] — remove the row with that `id`.
///
/// `Upserted` fires for every write path: create, update, and rename
/// (the new `path` is in the payload).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(u8)]
// Upserted carries the full Milestone by design (idempotent
// full-state payloads) — same trade-off as `task_proto::TaskEvent`.
#[allow(clippy::large_enum_variant)]
pub enum MilestoneEvent {
    /// A milestone was created or modified — the payload is the
    /// complete state after the write.
    Upserted(Milestone),
    /// The milestone with this id (and its backing file) was
    /// removed.
    Deleted(Uuid),
}

#[architect::rpc]
pub trait MilestoneService {
    /// Every milestone under the org's vault. Filter
    /// client-side by `project_id` / `goal_id` / status.
    fn list(&self) -> Result<Vec<Milestone>, MilestoneError>;

    fn get(&self, id: Uuid) -> Result<Milestone, MilestoneError>;

    fn get_by_path(&self, path: &str) -> Result<Milestone, MilestoneError>;

    /// Create a milestone. `project_id` is required. Backend
    /// resolves the project's slug to derive the default
    /// `path` (`Projects/<slug>/milestones/<ms-slug>.md`)
    /// when `path` is empty.
    fn create(&self, milestone: Milestone) -> Result<Milestone, MilestoneError>;

    fn update(&self, milestone: Milestone) -> Result<Milestone, MilestoneError>;

    fn rename(&self, id: Uuid, new_path: &str) -> Result<Milestone, MilestoneError>;

    /// Remove. Refuses if any task carries this `milestone_id`
    /// — clear the link on those tasks first.
    fn delete(&self, id: Uuid) -> Result<(), MilestoneError>;

    /// Every milestone change, as it happens — fires on each
    /// successful create / update / rename / delete. See
    /// [`MilestoneEvent`] for the fetch-once-then-fold subscriber
    /// contract.
    #[subscribe]
    fn events(&self) -> MilestoneEvent;
}
