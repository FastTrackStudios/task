// architect's Entity derive emits cfg-gated blocks; allow
// at crate scope.
#![allow(unexpected_cfgs)]

//! Wire contract for the milestone feature.
//!
//! Milestones are project-execution checkpoints — markdown files
//! with YAML frontmatter (`type: milestone`). Each is bound to a
//! project (`project_id`, required) and optionally to a life-goal
//! (`goal_id`). Tasks point at milestones via `milestone_id`.
//!
//! This proto owns the **wire surface**: the [`Milestone`] model
//! (+ canonical [`Status`] enum) and the [`MilestoneService`] CRUD
//! trait the capture/management UIs (CLI, web) bind to. It is
//! wasm-clean so the web UI can talk to the service directly.
//!
//! The sibling `milestone` crate sits on top of this proto and
//! owns the parse / serialize / scan side plus the disk-backed
//! [`MilestoneBackend`] — exactly like `locations` sits on top of
//! `locations-proto` and `inbox` on `inbox-proto`.

pub mod model;
pub mod service;

pub use model::{Milestone, Status, Tags};
pub use service::{MilestoneError, MilestoneEvent, MilestoneService, MilestoneServiceRpc};

// architect-emitted vox bits: the async client / dispatcher /
// descriptor / serve helpers. Mount sites stitch the descriptor
// + `serve` into the org router; the web UI binds the client.
#[cfg(feature = "vox")]
pub use service::{
    MilestoneServiceClient, MilestoneServiceRpcDispatcher as MilestoneDispatcher,
    Service as MilestoneServiceBridge, layer as milestone_service_layer,
    milestone_service_rpc_service_descriptor as milestone_service_descriptor,
    serve as serve_milestone_service,
};

// `#[subscribe] fn events` stream sibling — live milestone changes.
// Mount `milestone_service_stream_layer(backend)` next to the base
// service; subscribers drive a `MilestoneServiceStreamClient`.
#[cfg(feature = "vox")]
pub use service::{
    MilestoneServiceStream, MilestoneServiceStreamClient, MilestoneServiceStreamSource,
    milestone_service_stream_service_descriptor as milestone_stream_descriptor,
    stream_layer as milestone_service_stream_layer, stream_serve as serve_milestone_service_stream,
};
