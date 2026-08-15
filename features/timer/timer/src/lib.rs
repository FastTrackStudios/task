#![allow(clippy::large_futures)]
//! Server-side timer impl.
//!
//! Sits on top of the SeaORM `Model`/`Entity` items emitted
//! by `#[derive(architect::Entity)]` on `timer-proto`'s
//! `Client`, `WorkSession`, `Tag`, `WorkSessionTag`,
//! `ProjectMemberRate`, `OrgMemberRate`. Re-exports the
//! architect-emitted `<Name>RepoStorage<C>` types for callers
//! that want plain CRUD; adds the [`Store`] that owns the
//! operations CRUD can't express:
//!
//! - Atomic active-timer (`start_timer` flips a row in
//!   under a partial unique index on `user_id WHERE end_time
//!   IS NULL`).
//! - Rate-cascade resolution: ProjectMemberRate then project
//!   markdown `defaultRateCents` then OrgMemberRate then 0.
//! - Snapshot semantics: `rate_cents` + `currency` are
//!   frozen on `stop_timer`, so retroactively changing a
//!   project's rate doesn't re-bill closed sessions.

pub mod entity;
pub mod error;
pub mod migrations;
mod rate_cascade;
pub mod store;

pub use error::TimerDbError;
pub use migrations::Migrator;
pub use rate_cascade::ResolvedRate;
pub use store::Store;
