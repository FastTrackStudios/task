// architect's Entity derive emits cfg-gated blocks; allow at crate scope.
#![allow(unexpected_cfgs)]

//! `workflows-proto` — cross-domain workflow primitives.
//!
//! Every concrete workflow in Task (coding, writing, music,
//! research, …) lives over the same four entities:
//!
//! - [`WorkSession`] — one instance of an agent (or human)
//!   doing some work on a subject. Holds the current state +
//!   a scratchpad.
//! - [`Transition`] — a state change with attribution. Every
//!   move from one workflow state to another emits a row.
//! - [`Activity`] — a non-state event in the session (commit,
//!   comment, tool call). The full audit log.
//! - [`Handoff`] — a parked session's "where I left off" note.
//!   Lets a different agent (or session) `resume` cleanly.
//!
//! The [`AgentRef`] type names *who did the thing* — a human
//! by user_id, or an agent by name + model version. Lives here
//! (rather than in `task-proto`) because every workflow needs
//! it and a circular dep would otherwise form.
//!
//! The `Workflow` trait + `Orchestrator` runtime that drive
//! these entities live in the sibling `workflows-orchestrator`
//! crate. This crate ships only the types so wasm consumers
//! (the future workflow-UI components) stay light.

pub mod activity;
pub mod agent;
pub mod error;
pub mod goal;
pub mod handoff;
pub mod resume;
pub mod session;
pub mod transition;

pub use activity::{Activity, ActivityId, ActivityKind};
pub use agent::AgentRef;
pub use error::WorkflowError;
pub use goal::{GoalSession, Subgoals};
pub use handoff::{Handoff, HandoffId, HandoffReason, HandoffStatus};
pub use resume::{RelatedRef, ResumeContext, TransitionState};
pub use session::{SessionId, SessionStatus, SubjectRef, WorkSession, WorkflowKind};
pub use transition::{Transition, TransitionId};
