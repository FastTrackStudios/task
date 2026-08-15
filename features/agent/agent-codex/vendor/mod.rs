// Vendored code carries upstream's dead-code surface; the
// wrapper at `src/lib.rs` only exercises a subset for now.
// Lint exemptions are scoped to this module so app code
// stays strict.
#![allow(
    dead_code,
    unused_imports,
    unused_variables,
    unused_mut,
    private_interfaces,
    clippy::all,
    clippy::pedantic
)]

//! Vendored from `nashsu/CodexMonitor` (BSD-licensed; see
//! upstream `LICENSE`). Mostly verbatim; the only changes
//! are import path rewrites (`crate::backend::events` →
//! `super::events`, etc.) and a stripped-down `types` shim
//! that exposes just the fields the vendored code touches.
//!
//! The wrapper in `agent-codex/src/lib.rs` is what
//! implements `agent_proto::Agents`; this module just
//! supplies the `WorkspaceSession` + `EventSink` primitives
//! it builds on.

pub(crate) mod app_server;
pub(crate) mod args;
pub(crate) mod events;
pub(crate) mod process_core;
pub(crate) mod types;
