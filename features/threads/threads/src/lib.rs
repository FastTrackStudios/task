//! `threads` — facade for the threads feature.
//!
//! The universal conversation primitive: a [`Thread`] is a topic
//! anchored to any entity via `(entity_type, entity_id)` and a
//! [`Message`] is one entry in it. This crate is the single dependency
//! consumers reach for; it re-exports the trio behind feature gates:
//!
//! - always — the wire contract (`threads-proto`): entities, the
//!   [`ThreadsService`] trait, request payloads, and (with `vox`) the
//!   generated [`ThreadsServiceClient`] + service descriptors.
//! - `server` — the SeaORM-authoritative backend (`threads-db`):
//!   [`Store`] + [`Migrator`]. SeaORM means the DB is swappable.
//! - `ui` — the dumb Dioxus components (`threads-ui`).
//!
//! Wasm-clean under `default`/`vox`; pull `server` only on native.

pub use threads_proto::*;

/// The `threads-proto` crate itself, for callers that want the explicit
/// path (e.g. `threads::proto::service::...`).
pub use threads_proto as proto;

#[cfg(feature = "server")]
pub use threads_db::{self as db, Migrator, Store, ThreadsDbError, entity};

#[cfg(feature = "ui")]
pub use threads_ui as ui;
