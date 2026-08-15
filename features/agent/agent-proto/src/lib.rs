//! `agent-proto` — wire contract for the agent feature.
//!
//! Task's port of the LLM-agent integration patterns from
//! [`hermes-webui`](https://github.com/nesquena/hermes-webui)
//! (in-process Hermes) and
//! [`CodexMonitor`](https://github.com/Dimillian/CodexMonitor)
//! (external Codex CLI monitor).
//!
//! ## Two backend shapes
//!
//! - **In-process** — the backend embeds an agent runtime
//!   (e.g. Hermes Rust SDK, an OpenAI-API client) and answers
//!   `dispatch_turn` synchronously by spinning up a worker
//!   that emits [`event::AgentEvent`]s.
//! - **External-monitor** — the backend watches an external
//!   CLI's on-disk session logs and translates them into the
//!   same [`event::AgentEvent`] stream.
//!
//! ## Capabilities, not one trait
//!
//! There is **no** umbrella `Agents` trait. Each capability
//! is its own `#[architect::rpc]`-decorated trait under
//! [`service`]; backends implement only what they support,
//! and callers list what they need:
//!
//! ```ignore
//! fn drive_chat<A: Sessions + TurnDispatch + Subscriptions>(
//!     agent: &A,
//!     // ...
//! ) { /* ... */ }
//! ```
//!
//! The trait set covers:
//!
//! - [`service::sessions::Sessions`] — session lifecycle.
//! - [`service::turn_dispatch::TurnDispatch`] — dispatch /
//!   cancel / resume turns.
//! - [`service::threads::Threads`] — message history +
//!   curator notes.
//! - [`service::tool_calls::ToolCalls`] — tool-call audit.
//! - [`service::reasoning::Reasoning`] — extended-thinking
//!   access.
//! - [`service::attachments::Attachments`] — file / image
//!   store.
//! - [`service::approvals::Approvals`] — mid-turn permission
//!   gating.
//! - [`service::questions::Questions`] — mid-turn structured
//!   questions.
//! - [`service::tasks::AgentTaskQueue`] — queues + agent tasks
//!   plus links + comments. Modeled on hermes-webui's kanban;
//!   see `plans/agent-dispatch.md`.
//! - [`service::profiles::Profiles`] — agent identities.
//! - [`service::projects::Projects`] — workspace registry.
//! - [`service::backends::Backends`] — backend registry.
//! - [`service::subscriptions::Subscriptions`] — live event
//!   streams.
//! - [`service::external_import::ExternalImport`] — import
//!   external-CLI session logs.
//!
//! Each trait emits its own async client + dispatcher +
//! descriptor through the `vox` feature.
//!
//! ## Type modules
//!
//! - [`backend`], [`profile`], [`project`], [`session`],
//!   [`message`], [`tool`], [`reasoning`], [`attachment`],
//!   [`approval`], [`question`], [`tasks`] — typed value
//!   objects the traits operate on.
//! - [`event`] — `AgentEvent` streaming union.
//! - [`error`] — `AgentError`.
//! - [`paths`] — disk layout for backends.

pub mod approval;
pub mod attachment;
pub mod backend;
pub mod error;
pub mod event;
pub mod message;
pub mod paths;
pub mod profile;
pub mod project;
pub mod question;
pub mod reasoning;
pub mod routing;
pub mod run;
pub mod run_event;
pub mod runner;
pub mod service;
pub mod session;
pub mod supervisor;
pub mod tasks;
pub mod tool;

pub use error::AgentError;
pub use event::{AgentEvent, AgentEventEnvelope};
pub use routing::{Refusal, TicketRef, malformed, requirements, takeable, unroutable};
pub use run::{FinishRun, Run, RunFilter, RunStatus, StartRun};
pub use run_event::{RunEvent, RunEventEnvelope, RunSnapshot, append_tail};
pub use runner::{
    Capability, CapabilityError, RunnerProfile, RunnerScope, TicketRequirements, Unroutable,
    parse_capabilities, unsatisfiable_capability,
};
pub use supervisor::{MAX_RESTARTS, NO_PROGRESS_AFTER, Recovery};

// Convenience re-exports of the per-capability traits.
// Concrete RPC scaffolding (per-trait `Rpc` mirror,
// `Client`, `Dispatcher`, descriptor function) lives in
// each `service::<name>` module.
pub use service::{
    AgentTaskQueue, Approvals, Attachments, Backends, ExternalImport, Profiles, Projects,
    Questions, Reasoning, Sessions, Subscriptions, Threads, ToolCalls, TurnDispatch,
};
