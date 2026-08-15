//! `ProjectService` — wire surface for browsing projects.
//!
//! Read-only for now (the UI's primary need). Mutation
//! verbs follow when the project-editor route lands; the
//! trait shape mirrors `CookbookService` so the additions
//! land the same way.

use facet::Facet;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::model::ProjectInfo;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet, Error)]
#[repr(u8)]
pub enum ProjectError {
    /// Project with the given id / path doesn't exist on the
    /// server.
    #[error("not found: {0}")]
    NotFound(String),
    /// `create` collided with an existing file at the same
    /// vault-relative path.
    #[error("already exists: {0}")]
    AlreadyExists(String),
    /// Caller asked for a malformed id or path.
    #[error("bad request: {0}")]
    BadRequest(String),
    /// Anything else — filesystem hiccup, parse failure on a
    /// page the server thought was a project, etc.
    #[error("io: {0}")]
    Io(String),
}

/// One project change, broadcast to every [`ProjectService`]
/// subscriber on each successful mutation.
///
/// ## Subscriber contract (no snapshot variant, v1)
///
/// The stream carries *changes only* — there is no `Snapshot`
/// variant. A subscriber that wants the full board state fetches it
/// once via [`ProjectService::list`] (after subscribing, so nothing
/// is missed in between) and then folds events into that local copy:
///
/// - [`ProjectEvent::Upserted`] carries the **full post-write**
///   [`ProjectInfo`] — replace (or insert) the row with a matching
///   `id`. Re-applying an event already reflected in the fetched
///   list is harmless (idempotent re-application).
/// - [`ProjectEvent::Deleted`] — remove the row with that `id`.
///
/// `Upserted` fires for every write path: create, update, and rename
/// (the new `path` is in the payload).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(u8)]
// Upserted carries the full ProjectInfo by design (idempotent
// full-state payloads) — same trade-off as `task_proto::TaskEvent`.
#[allow(clippy::large_enum_variant)]
pub enum ProjectEvent {
    /// A project was created or modified — the payload is the
    /// complete state after the write.
    Upserted(ProjectInfo),
    /// The project with this id (and its backing file) was removed.
    Deleted(Uuid),
}

#[architect::rpc]
pub trait ProjectService {
    /// Every project page found under the org's vault. Order
    /// is implementation-defined; callers sort if they need
    /// stability (the typical UI groups by `parent_id` first
    /// anyway).
    fn list(&self) -> Result<Vec<ProjectInfo>, ProjectError>;

    /// One project by stable UUID. Use over [`Self::get_by_path`]
    /// when the caller already has the id — renaming the
    /// markdown file won't break the lookup.
    fn get(&self, id: Uuid) -> Result<ProjectInfo, ProjectError>;

    /// One project by vault-relative path. Convenience for
    /// the link-resolution layer, which sees paths long
    /// before it sees ids.
    fn get_by_path(&self, path: &str) -> Result<ProjectInfo, ProjectError>;

    /// Create a new project. The backend assigns
    /// `project.path` (default `Projects/<slug>.md`) when empty
    /// and `project.id` when nil. Returns the stored row.
    /// `AlreadyExists` on path collision.
    fn create(&self, project: ProjectInfo) -> Result<ProjectInfo, ProjectError>;

    /// Replace the project whose `id` matches. `NotFound` when
    /// unknown. Path mutations are ignored — rename via
    /// [`Self::rename`] so backing-file moves are explicit.
    fn update(&self, project: ProjectInfo) -> Result<ProjectInfo, ProjectError>;

    /// Move the backing markdown file to a new vault-relative
    /// path. `id` is preserved so downstream FKs (timer rows,
    /// links from other vault pages) survive.
    fn rename(&self, id: Uuid, new_path: &str) -> Result<ProjectInfo, ProjectError>;

    /// Remove the backing file. `NotFound` if the id is
    /// already gone. Refuses if any other project lists this
    /// one as `parent_id`.
    fn delete(&self, id: Uuid) -> Result<(), ProjectError>;

    /// Every project change, as it happens — fires on each
    /// successful create / update / rename / delete. See
    /// [`ProjectEvent`] for the fetch-once-then-fold subscriber
    /// contract.
    #[subscribe]
    fn events(&self) -> ProjectEvent;
}
