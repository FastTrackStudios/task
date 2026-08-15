#![allow(unexpected_cfgs)]

//! `workstream` — the project-scoped parent-with-swarm
//! construct that replaces the `epic` tag.
//!
//! The wasm-clean wire surface ([`Workstream`] / [`Status`] /
//! [`WorkstreamRollup`] + the [`WorkstreamService`] RPC trait)
//! lives in the sibling [`workstream_proto`] crate; this crate
//! sits on top of it and owns the vault-backed side:
//! - [`parse_page`] / [`looks_like_workstream`] — vault page →
//!   `Workstream`
//! - [`serialize_workstream`] / [`write_workstream`] /
//!   [`default_workstream_path`] — writer + path helper
//! - [`rollup`] — pure derived-progress engine over
//!   `task::TaskInfo` rows
//! - [`WorkstreamBackend`] — server impl of
//!   [`WorkstreamService`] (CRUD + rollup + event stream)
//!
//! See `Workstream` doc-comments for the lead / members /
//! rollup design.

pub mod model;
pub mod rollup;
pub mod service;

// `entity` / `parse` / `write` / `backend` all reach the shared
// `vault-entity` support layer, which walks `std::fs` (and pulls a
// file watcher). Wasm consumers take `workstream-proto` directly;
// `rollup` is pure and stays available everywhere.
#[cfg(not(target_arch = "wasm32"))]
pub mod backend;
#[cfg(not(target_arch = "wasm32"))]
pub mod entity;
#[cfg(not(target_arch = "wasm32"))]
pub mod parse;
#[cfg(not(target_arch = "wasm32"))]
pub mod write;

pub use model::{AgentRefList, Links, Status, Workstream};
pub use rollup::{estimate_points, rollup, rollup_tasks, rollup_with, subtask_rollup};
pub use service::{
    StateGroupCounts, WorkstreamError, WorkstreamEvent, WorkstreamRollup, WorkstreamService,
    WorkstreamWithRollup,
};

#[cfg(not(target_arch = "wasm32"))]
pub use backend::WorkstreamBackend;
#[cfg(not(target_arch = "wasm32"))]
pub use entity::Workstreams;
#[cfg(not(target_arch = "wasm32"))]
pub use parse::{ParseError, looks_like_workstream, parse_page, parse_workstream};
#[cfg(not(target_arch = "wasm32"))]
pub use write::{WriteError, default_workstream_path, serialize_workstream, write_workstream};

#[cfg(feature = "vox")]
pub use workstream_proto::{
    WorkstreamDispatcher, WorkstreamServiceRpc, WorkstreamServiceStreamSource,
};
#[cfg(feature = "vox")]
pub use workstream_proto::{
    WorkstreamServiceBridge, WorkstreamServiceClient, WorkstreamServiceStreamClient,
    serve_workstream_service, workstream_service_descriptor, workstream_service_layer,
    workstream_service_stream_layer, workstream_stream_descriptor,
};
