//! `GoalService` — wire surface for reading + mutating goals.
//!
//! Mirrors the `ProjectService` + `LocationsService` shape:
//! `create` / `update` take the full record, `rename` moves the
//! backing file while preserving `id`, `delete` removes both
//! the file and any DB row.

use facet::Facet;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::model::Goal;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet, Error)]
#[repr(u8)]
pub enum GoalError {
    #[error("not found: {0}")]
    NotFound(String),
    /// `create` collided with an existing file at the same
    /// vault-relative path.
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("io: {0}")]
    Io(String),
}

/// One goal change, broadcast to every [`GoalService`] subscriber on
/// each successful mutation.
///
/// ## Subscriber contract (no snapshot variant, v1)
///
/// The stream carries *changes only* — there is no `Snapshot`
/// variant. A subscriber that wants the full state fetches it once
/// via [`GoalService::list`] (after subscribing, so nothing is
/// missed in between) and then folds events into that local copy:
///
/// - [`GoalEvent::Upserted`] carries the **full post-write**
///   [`Goal`] — replace (or insert) the row with a matching `id`.
///   Re-applying an event already reflected in the fetched list is
///   harmless (idempotent re-application).
/// - [`GoalEvent::Deleted`] — remove the row with that `id`.
///
/// `Upserted` fires for every write path: create, update, and rename
/// (the new `path` is in the payload).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(u8)]
// Upserted carries the full Goal by design (idempotent full-state
// payloads) — same trade-off as `task_proto::TaskEvent`.
#[allow(clippy::large_enum_variant)]
pub enum GoalEvent {
    /// A goal was created or modified — the payload is the complete
    /// state after the write.
    Upserted(Goal),
    /// The goal with this id (and its backing file) was removed.
    Deleted(Uuid),
}

#[architect::rpc]
pub trait GoalService {
    /// Every `type: goal` page under the org's vault.
    fn list(&self) -> Result<Vec<Goal>, GoalError>;

    /// One goal by stable UUID.
    fn get(&self, id: Uuid) -> Result<Goal, GoalError>;

    /// One goal by vault-relative path.
    fn get_by_path(&self, path: &str) -> Result<Goal, GoalError>;

    /// Create a new goal. Backend assigns `goal.path`
    /// (`Goals/<slug>.md`) when empty, `goal.id` when nil.
    /// `AlreadyExists` on path collision.
    fn create(&self, goal: Goal) -> Result<Goal, GoalError>;

    /// Replace the goal whose `id` matches. `NotFound` when
    /// unknown. Path mutations are ignored — rename via
    /// [`Self::rename`].
    fn update(&self, goal: Goal) -> Result<Goal, GoalError>;

    /// Move the backing markdown file to a new vault-relative
    /// path. `id` preserved.
    fn rename(&self, id: Uuid, new_path: &str) -> Result<Goal, GoalError>;

    /// Remove the backing file. `NotFound` if already gone.
    /// Refuses if any other goal lists this one as `parent_id`.
    fn delete(&self, id: Uuid) -> Result<(), GoalError>;

    /// Every goal change, as it happens — fires on each successful
    /// create / update / rename / delete. See [`GoalEvent`] for the
    /// fetch-once-then-fold subscriber contract.
    #[subscribe]
    fn events(&self) -> GoalEvent;
}
