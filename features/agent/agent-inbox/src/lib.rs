//! `agent-inbox` — wires `agent-proto` agent loops to the
//! inbox feature's **daily processing pass**.
//!
//! The crate is a *binding library*, not a backend — the same
//! shape as [`agent-wiki`](../agent-wiki). It supplies:
//!
//! - [`prompts`] — the batch-triage system prompt: given the
//!   open fleeting inbox items and the org's project list,
//!   propose exactly one `task` / `note` / `skip` action per
//!   item.
//! - [`parsers`] — the `---ITEM: <id>---` fenced-block parser
//!   that turns the LLM response into typed [`Proposal`]s.
//! - [`bridge`] — orchestration: run ONE Codex turn over a
//!   batch of items and return `Vec<Proposal>`.
//! - [`error`] — [`AgentInboxError`].
//!
//! The CLI (`task inbox process`) fetches the open items +
//! projects, calls [`bridge::run_process`], walks the user
//! through each proposal, and applies accepted ones through
//! the existing `TaskService` / inbox / vault surfaces —
//! nothing here writes anything.

pub mod bridge;
pub mod error;
pub mod parsers;
pub mod prompts;

pub use error::AgentInboxError;
pub use parsers::{Proposal, ProposalAction};
