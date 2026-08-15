#![allow(unexpected_cfgs)]

//! `milestone` — project-scoped, GitHub-Projects-style
//! checkpoint entity.
//!
//! The wasm-clean wire surface ([`Milestone`] / [`Status`] +
//! the [`MilestoneService`] RPC trait) lives in the sibling
//! [`milestone_proto`] crate; this crate sits on top of it and
//! owns the vault-backed side:
//! - [`parse_page`] / [`looks_like_milestone`] — vault page →
//!   `Milestone`
//! - [`serialize_milestone`] / [`write_milestone`] /
//!   [`default_milestone_path`] — writer + path helper
//! - [`MilestoneBackend`] — server impl of [`MilestoneService`]
//!
//! See `Milestone` doc-comments for the project / goal /
//! Forgejo-sync rollup design.

pub mod model;
pub mod service;

// `entity` / `parse` / `write` / `backend` all reach the shared
// `vault-entity` support layer, which walks `std::fs` (and pulls a
// file watcher). Wasm consumers take `milestone-proto` directly.
#[cfg(not(target_arch = "wasm32"))]
pub mod backend;
#[cfg(not(target_arch = "wasm32"))]
pub mod entity;
#[cfg(not(target_arch = "wasm32"))]
pub mod parse;
#[cfg(not(target_arch = "wasm32"))]
pub mod write;

pub use model::{Milestone, Status, Tags};
pub use service::{MilestoneError, MilestoneService};
pub use milestone_proto::MilestoneEvent;

#[cfg(not(target_arch = "wasm32"))]
pub use backend::MilestoneBackend;
#[cfg(not(target_arch = "wasm32"))]
pub use entity::Milestones;
#[cfg(not(target_arch = "wasm32"))]
pub use parse::{ParseError, looks_like_milestone, parse_milestone, parse_page};
#[cfg(not(target_arch = "wasm32"))]
pub use write::{WriteError, default_milestone_path, serialize_milestone, write_milestone};

#[cfg(feature = "vox")]
pub use milestone_proto::{
    MilestoneServiceBridge, MilestoneServiceClient, milestone_service_descriptor,
    milestone_service_layer, serve_milestone_service,
};
// `#[subscribe] fn events` stream sibling — live milestone changes.
#[cfg(feature = "vox")]
pub use milestone_proto::{
    MilestoneServiceStream, MilestoneServiceStreamClient, MilestoneServiceStreamSource,
    milestone_service_stream_layer, milestone_stream_descriptor, serve_milestone_service_stream,
};
#[cfg(feature = "vox")]
pub use service::{MilestoneServiceRpc, MilestoneServiceRpcDispatcher as MilestoneDispatcher};
