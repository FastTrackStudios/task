//! `workflows-orchestrator` — the concrete coding-workflow runtime.
//!
//! Sits on top of [`workflows_proto`]'s entities + value types and
//! drives a [`WorkSession`](workflows_proto::WorkSession) through the
//! `task code` loop, persisting the audit log to a per-org JSON
//! store.
//!
//! There is intentionally **no generic `Workflow` trait** here yet.
//! Coding is the one workflow that exists; the right abstraction is
//! the one extracted from two real implementations, so the trait is
//! deferred until a second domain (writing, music, …) arrives. See
//! the note in [`workflows_proto::resume`].
//!
//! Likewise the capability gate (agent → allowed transitions) is
//! deferred: there's a single trusted agent class today, so gating
//! would be speculative. The seam is the [`CodingWorkflow`] verb set
//! — wrap or extend it when an untrusted agent class appears.

pub mod coding;
pub mod runner;
pub mod store;

pub use coding::{CodingState, CodingWorkflow};
pub use runner::{IterationOutcome, RunEnd, SessionRun};
pub use store::WorkflowStore;
