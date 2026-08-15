// architect's Entity derive emits cfg-gated blocks; allow
// at crate scope.
#![allow(unexpected_cfgs)]

//! Wire contract for the goal feature.
//!
//! Goals are long-term aspirations — markdown files with YAML
//! frontmatter (`type: goal`). A [`Goal`] carries a `target_date` and
//! a `kind` (`lifetime` / `yearly` / `quarterly` / `cycle` / `weekly`)
//! and decomposes into smaller goals via `parent_id`.
//!
//! This proto owns the **wire surface**: the [`Goal`] model (+
//! canonical [`Kind`] / [`Status`] enums) and the [`GoalService`] CRUD
//! trait with its `#[subscribe] fn events` stream. It is wasm-clean so
//! the web UI can talk to the service directly.
//!
//! The sibling `goal` crate sits on top of this proto and owns the
//! parse / serialize / scan side plus the disk-backed `GoalBackend` —
//! exactly like `milestone` sits on top of `milestone-proto` and
//! `task` on `task-proto`.

pub mod model;
pub mod service;

pub use model::{Goal, Kind, Status, Tags};
pub use service::{GoalError, GoalEvent, GoalService, GoalServiceRpc};

// architect-emitted vox bits: the async client / dispatcher /
// descriptor / serve helpers. Mount sites stitch the descriptor
// + `serve` into the org router; the web UI binds the client.
#[cfg(feature = "vox")]
pub use service::{
    GoalServiceClient, GoalServiceRpcDispatcher as GoalDispatcher, Service as GoalServiceBridge,
    goal_service_rpc_service_descriptor as goal_service_descriptor, layer as goal_service_layer,
    serve as serve_goal_service,
};

// `#[subscribe] fn events` stream sibling — live goal changes.
// Mount `goal_service_stream_layer(backend)` next to the base
// service; subscribers drive a `GoalServiceStreamClient`.
#[cfg(feature = "vox")]
pub use service::{
    GoalServiceStream, GoalServiceStreamClient, GoalServiceStreamSource,
    goal_service_stream_service_descriptor as goal_stream_descriptor,
    stream_layer as goal_service_stream_layer, stream_serve as serve_goal_service_stream,
};
