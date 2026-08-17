// architect's Entity derive emits cfg-gated blocks; allow
// at crate scope.
#![allow(unexpected_cfgs)]

//! `mount-proto` — wire contract for the per-machine project
//! content mount registry.
//!
//! The federated task platform separates *identity* (home org),
//! *metadata* (per-org SQLite DBs), and *content* (markdown,
//! attachments, media) onto three independent axes. Content
//! location is per-machine and gets registered through this
//! crate: a `Mount` row says "this project's bytes live at
//! this path on this machine, served by this backend."
//!
//! - [`Mount`] — `architect::Entity` keyed by the project's
//!   UUID; one row per project + machine.
//! - [`BackendKind`] — discriminates the active impl. Initial
//!   set is `Filesystem`; `Nextcloud` / `VoxProxy` are
//!   reserved variants for Phases 3+.
//! - [`ContentBackend`] — the per-backend access trait
//!   implemented by sibling crates (`mount` for the FS impl,
//!   future `mount-nextcloud` / `mount-vox-proxy`).
//!
//! See federated-platform Phase 2 for the
//! design context.

mod backend;
mod entry;
mod error;
mod mount;

pub use backend::{BackendKind, ContentBackend};
pub use entry::Entry;
pub use error::MountError;
pub use mount::Mount;
