//! Bridge between task notes and agent-task queues.
//!
//! See `plans/agent-dispatch.md`. This crate is the orchestration
//! layer that turns a user action ("dispatch this task note to an
//! agent queue") into two coordinated writes:
//!
//! 1. INSERT into `agent_tasks` with `source_task` pointing back
//!    at the markdown file (via [`agent_tasks::Store`]).
//! 2. Patch the task note's frontmatter: append the new agent
//!    task id to `dispatchedAgentTasks: [...]` (via
//!    `task::write_task`).
//!
//! On completion the reverse happens — `complete_dispatched`
//! moves the row to `done`, and we update the source task note:
//! - **One-shot tasks**: status → `done`, `completedDate` stamped.
//! - **Recurring tasks**: today's date appended to
//!   `complete_instances`.
//!
//! All writes go through the vault filesystem and the SeaORM
//! connection; the crate is **server-side** (per
//! `plans/agent-dispatch.md` decision: the dispatcher runs on the
//! server, clients call it over vox).

#![allow(clippy::large_futures)]

mod cron;
mod error;
mod ops;

pub use cron::{ScheduleReport, WeeklyDigest, digest_since, schedule_recurring, weekly_digest};
pub use error::DispatchError;
pub use ops::{DispatchOptions, DispatchOutcome, complete_dispatched, dispatch};
