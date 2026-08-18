// architect's `Entity` derive emits cfg-gated blocks; allow
// at crate scope.
#![allow(unexpected_cfgs)]

//! `project` — first-party project feature.
//!
//! The wasm-clean wire surface ([`ProjectInfo`] / [`Status`], the
//! per-project state registry, and the [`ProjectService`] RPC
//! trait) lives in the sibling [`project_proto`] crate; this crate
//! sits on top of it and owns the vault-backed side (parse /
//! serialize / scan / write / [`ProjectBackend`]). Every proto item
//! is re-exported here at its historical `project::…` path.
//!
//! Projects are plain markdown pages living under
//! `Projects/*.md` in the vault. Frontmatter carries the
//! identity (stable `id:` UUID), display fields (title,
//! description, color), status, and the billing knobs that
//! downstream features (`timer`, `finances`,
//! `agent-dispatch`) need to know about a project without
//! opening a database.
//!
//! ## Why on disk, not in a database
//!
//! Same answer as `task`: the markdown file is the user's
//! mental model and the lowest-friction surface for editing.
//! Downstream features that need stable references use the
//! `id:` field in the frontmatter (a UUID set on first save),
//! not the file path — so renaming a project file doesn't
//! break the timer rows or invoice lines that reference it.
//!
//! ## Surface
//!
//! - [`ProjectInfo`] — the parsed project model.
//! - [`Status`] — a configurable enum (default set mirrors
//!   common project lifecycles).
//! - [`parse_page`] / [`parse_str`] — `VaultPage` → `ProjectInfo`.
//! - [`serialize_project`] / [`write_project`] —
//!   `ProjectInfo` → markdown bytes on disk.
//! - [`scan_vault`] — collect every `type: project` page from
//!   a `vault::Vault`.
//! - [`looks_like_project`] — discriminator used by the
//!   scanner.

pub mod model;
/// Parts and capabilities — re-exported from the wasm-clean proto
/// crate, like [`model`].
pub mod parts {
    pub use project_proto::parts::{
        Audience, Capabilities, Capability, Deliverable, DeliverableItem, Deliverables, Medium,
        Part, Parts, Piece, Scope,
    };
}
pub mod service;
pub mod states;

// FS-dependent modules. `entity` / `parse` reach the shared
// `vault-entity` support layer, which walks `std::fs` (and pulls a
// file watcher), as do `backend` / `scan` / `write`. Browser
// consumers take `project-proto` instead of this crate, so none of
// it needs a target gate.
pub mod backend;
pub mod entity;
pub mod parse;
pub mod scan;
pub mod write;

pub use entity::Projects;
pub use model::{ProjectInfo, Status};
pub use parse::{ParseError, looks_like_project, parse_page, parse_str};
pub use parts::{
    Audience, Capabilities, Capability, Deliverable, DeliverableItem, Deliverables, Medium, Part,
    Parts, Piece, Scope,
};
pub use service::{ProjectError, ProjectEvent, ProjectService, ProjectServiceRpc};
#[cfg(feature = "vox")]
pub use service::{
    ProjectServiceClient, ProjectServiceRpcDispatcher as ProjectDispatcher,
    Service as ProjectServiceBridge, layer as project_service_layer,
    project_service_rpc_service_descriptor as project_service_descriptor,
    serve as serve_project_service,
};
// `#[subscribe] fn events` stream sibling — live project changes.
// Mount `project_service_stream_layer(backend)` next to the base
// service; subscribers drive a `ProjectServiceStreamClient`.
#[cfg(feature = "vox")]
pub use service::{
    ProjectServiceStream, ProjectServiceStreamClient, ProjectServiceStreamSource,
    project_service_stream_service_descriptor as project_stream_descriptor,
    stream_layer as project_service_stream_layer, stream_serve as serve_project_service_stream,
};
pub use states::{StateDef, StateGroup, StatesConfig, default_states, resolve_state_group};

/// Verify-command resolution (see [`project_proto::verify`]).
pub use project_proto::verify;

pub use backend::ProjectBackend;
pub use scan::scan_vault;
pub use write::{WriteError, serialize_project, write_project};
