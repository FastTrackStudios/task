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
use crate::parts::{Deliverable, DeliverableItem, Part, Piece};

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

    // ── Parts ───────────────────────────────────────────────
    //
    // Four verbs and no store. A part lives in its project's
    // frontmatter (`project.part.unit`: "no directory, no marker, no
    // capabilities of its own"), so every one of these is a read,
    // an edit and a save of one page.
    //
    // They are on the project lane rather than a lane of their own for
    // the same reason: a lane implies a thing with an address, and the
    // whole claim about a part is that it is not one. `parts` returns
    // the list because a caller holding a project id should not have to
    // fetch the page to read it.

    /// This project's parts, in declaration order.
    fn parts(&self, project: Uuid) -> Result<Vec<Part>, ProjectError>;

    /// Name a new part. Returns it, with the id it was given.
    ///
    /// `AlreadyExists` when the project already has a part of that name
    /// — case-insensitively, because one song with two spellings is a
    /// project whose setlist references are ambiguous.
    fn add_part(&self, project: Uuid, name: &str) -> Result<Part, ProjectError>;

    /// Rename a part, keeping its id.
    ///
    /// The id is the point: everything attached to a part addresses it
    /// by id, so renaming is a display change and nothing else has to
    /// hear about it.
    fn rename_part(&self, project: Uuid, part: Uuid, name: &str) -> Result<Part, ProjectError>;

    /// Remove a part.
    ///
    /// Nothing cascades, because nothing is owned: a part has no files
    /// and no page. What it may have is *references* — and those are
    /// somebody else's rows, which this lane cannot see and must not
    /// silently invalidate. Removing a referenced part is allowed and
    /// leaves the references dangling, which is the same contract
    /// `delete` has for a project.
    fn remove_part(&self, project: Uuid, part: Uuid) -> Result<(), ProjectError>;

    /// Every piece of this project's work, in the project's own order.
    ///
    /// `project.part.listing`. Parts and promoted subprojects in one
    /// list, each saying which it is — so a track listing reads it and
    /// ignores the flag, and a "promote" button reads the flag.
    ///
    /// Order survives promotion: an album's fourth track is its fourth
    /// track before and after it grows a page.
    fn pieces(&self, project: Uuid) -> Result<Vec<Piece>, ProjectError>;

    /// Turn a part into a subproject, keeping its id.
    ///
    /// `project.part.promotion`. The new project *is* the part — same
    /// id, so every link, deliverable, setlist reference and time entry
    /// already attached keeps resolving without being told. It leaves
    /// the parent's part list as it gains a page, because a piece is on
    /// exactly one side of the line at a time.
    fn promote_part(&self, project: Uuid, part: Uuid) -> Result<ProjectInfo, ProjectError>;

    /// Turn a subproject back into a part of its parent, keeping its id.
    ///
    /// `project.part.demotable`. Refused when the subproject has
    /// subprojects of its own, since a part cannot have them. Content —
    /// files, tasks, time, deliverables — is not an obstacle: a part
    /// carries exactly those.
    fn demote_project(&self, project: Uuid) -> Result<Part, ProjectError>;

    // ── Deliverables ────────────────────────────────────────

    /// What this project has declared it owes.
    ///
    /// Declarations, not files: `project.deliverable.scope` wants five
    /// of these for a concert, not twenty-one.
    fn deliverables(&self, project: Uuid) -> Result<Vec<Deliverable>, ProjectError>;

    /// Declare one. `AlreadyExists` on a duplicate name.
    fn declare_deliverable(
        &self,
        project: Uuid,
        deliverable: Deliverable,
    ) -> Result<Deliverable, ProjectError>;

    /// Withdraw a declaration.
    ///
    /// Withdraws what the project *owes*; it does not touch content
    /// anything was bound to, for the same reason removing a capability
    /// does not — a declaration is a statement about obligation, and
    /// deleting the work on the strength of one being retracted is not
    /// a thing a person asking for this could mean.
    fn withdraw_deliverable(&self, project: Uuid, deliverable: Uuid) -> Result<(), ProjectError>;

    /// Every declaration expanded against the project's pieces.
    ///
    /// A per-part audio declaration over ten songs is ten items here,
    /// and eleven the moment an eleventh song is named — derived on
    /// read, so "stays in step" is not a job anybody has to remember to
    /// run. Unaffected by whether a piece is promoted.
    ///
    /// The **member** view: everything, including what is internal.
    fn deliverable_items(&self, project: Uuid) -> Result<Vec<DeliverableItem>, ProjectError>;

    /// The same expansion, as a client may see it.
    ///
    /// A separate verb rather than an audience parameter, because
    /// `project.deliverable.client-view` says nothing marked internal is
    /// *reachable* from a client's view — and a parameter is a thing a
    /// caller can get wrong, once, quietly. There is nothing to pass
    /// here, so there is nothing to pass incorrectly.
    ///
    /// Ordered by scope then medium, which is the organisation the rule
    /// asks for: the whole performance, then a specific song.
    fn client_deliverables(&self, project: Uuid) -> Result<Vec<DeliverableItem>, ProjectError>;

    /// Every project change, as it happens — fires on each
    /// successful create / update / rename / delete. See
    /// [`ProjectEvent`] for the fetch-once-then-fold subscriber
    /// contract.
    #[subscribe]
    fn events(&self) -> ProjectEvent;
}
