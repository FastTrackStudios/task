#![allow(clippy::large_futures)]
//! Server-side persistence + lifecycle layer for agent-task queues.
//!
//! Sits on top of the SeaORM `Model`/`Entity` items emitted by
//! `#[derive(architect::Entity)]` on `agent-proto`'s [`Queue`],
//! [`AgentTask`], [`AgentTaskLink`], [`AgentTaskComment`]. We
//! re-export the architect-emitted `<Name>RepoStorage<C>` types
//! for callers that want plain CRUD, and add a [`Store`] that
//! owns the operations CRUD can't express:
//!
//! - **Atomic claim**: `claim_agent_task` flips status to
//!   `running` only when the row is in `ready` AND unclaimed,
//!   under a single `UPDATE` so two workers can't double-claim.
//! - **Dependency-checked completion**: `complete_agent_task`
//!   rejects when an incoming `blocks` link still points at a
//!   non-`done` task.
//! - **Status guard**: `set_agent_task_status` refuses
//!   `"running"` — the only path in is `claim_agent_task`.
//! - **Snapshot read**: `read_queue` packages queue metadata +
//!   filtered tasks + the latest event-log watermark in one
//!   round-trip so a subscriber can resume the live tail.
//!
//! ## Crate layout
//!
//! - `migrations` — SeaORM `Migrator` for the five tables. Run
//!   against any `ConnectionTrait`.
//! - `store` — [`Store`] type implementing
//!   `agent_proto::service::tasks::AgentTaskQueue`.
//! - `entity` — re-exports the architect-emitted SeaORM items
//!   from `agent-proto`.
//!
//! See `plans/agent-dispatch.md` for the design.

pub mod entity;
pub mod error;
pub mod migrations;
mod store;

pub use error::TasksDbError;
pub use migrations::Migrator;
pub use store::Store;
