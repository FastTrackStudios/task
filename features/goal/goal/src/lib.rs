//! `goal` — long-term aspirations + their decomposition.
//!
//! The wasm-clean wire surface ([`Goal`] / [`Kind`] / [`Status`] +
//! the [`GoalService`] RPC trait and its `#[subscribe]` events
//! stream) lives in the sibling [`goal_proto`] crate; this crate
//! sits on top of it and owns the vault-backed side:
//!
//! - [`parse_page`] / [`looks_like_goal`] — vault page → `Goal`
//! - [`serialize_goal`] / [`write_goal`] / [`default_goal_path`] —
//!   writer + path helper
//! - [`GoalBackend`] — server impl of [`GoalService`]
//!
//! Markdown frontmatter stays the source of truth; goal pages live at
//! `vault/Goals/<slug>.md` (top-level) and
//! `vault/Goals/<parent-slug>/<slug>.md` (nested decompositions) —
//! the `parent_id` field is what the DB reads, the folder layout is
//! for human navigation. See `plans/cyclic-life-calendar.md` for the
//! planning system goals plug into.
//!
//! This crate is native-only (it walks the vault via `std::fs`); wasm
//! UI consumers take `goal-proto` directly — which is why the old
//! `cfg(not(target_arch = "wasm32"))` gates are gone.

pub mod model;
pub mod service;

mod backend;
pub mod entity;
mod parse;
mod write;

pub use model::{Goal, Kind, Status, Tags};
pub use service::{GoalError, GoalEvent, GoalService, GoalServiceRpc};

pub use backend::GoalBackend;
pub use entity::Goals;
pub use parse::{ParseError, looks_like_goal, parse_goal, parse_page};
pub use write::{WriteError, default_goal_path, serialize_goal, write_goal};

#[cfg(feature = "vox")]
pub use goal_proto::{
    GoalDispatcher, GoalServiceBridge, GoalServiceClient, goal_service_descriptor,
    goal_service_layer, serve_goal_service,
};
// `#[subscribe] fn events` stream sibling — live goal changes.
#[cfg(feature = "vox")]
pub use goal_proto::{
    GoalServiceStream, GoalServiceStreamClient, GoalServiceStreamSource, goal_stream_descriptor,
    goal_service_stream_layer, serve_goal_service_stream,
};
