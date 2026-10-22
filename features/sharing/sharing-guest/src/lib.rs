//! Guest views for cross-org project sharing.
//!
//! A grant guest — a person in another org holding a `SharingGrant` on
//! one root project — receives the owner's records through the grant
//! lane, and never in the owner's own shape. This crate is the shape
//! they receive instead:
//!
//! - [`guest_project`] — the one projection of a [`project_proto::ProjectInfo`]
//!   and everything nested in it (parts, deliverables), with the
//!   standalone part / piece / setlist / deliverable helpers beside it;
//! - [`guest_task`] — the same for a [`task_proto::TaskInfo`];
//! - [`link`] — the redaction helper every reference goes through.
//!
//! t[impl sharing.grant.exclusions] — money and operator internals are
//! not blanked, they are *absent*: the guest types have no field to
//! carry them. t[impl sharing.grant.absent] — what is outside the grant
//! is either not listed at all or, where a record points at it, the
//! opaque [`link::NOT_SHARED`] marker with no id, title or count.
//!
//! Everything is pure and wasm-clean. Which objects are in the grant is
//! the caller's predicate — the subtree resolver in the server — so this
//! crate reads no store and cannot drift from it.

pub mod link;
pub mod project;
pub mod task;

pub use link::{
    Linked, NOT_SHARED, ObjectRef, ScopeSet, processed_into_ref, redact_list, redact_one,
    redact_opt, redact_processed_into, redact_reverse_relations,
};
pub use project::{
    GuestPart, GuestProject, SHARED_PROJECT_FIELDS, STRIPPED_PROJECT_FIELDS, guest_deliverable,
    guest_deliverable_items, guest_part, guest_piece, guest_pieces, guest_project, guest_projects,
};
pub use task::{
    GuestTask, GuestWorkflow, SHARED_TASK_FIELDS, SHARED_WORKFLOW_FIELDS, STRIPPED_TASK_FIELDS,
    STRIPPED_WORKFLOW_FIELDS, guest_task, guest_tasks,
};
