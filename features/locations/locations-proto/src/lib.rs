// architect's Entity derive emits cfg-gated blocks; allow
// at crate scope.
#![allow(unexpected_cfgs)]

//! Wire contract for the locations feature.
//!
//! Locations are place pages — markdown files with YAML
//! frontmatter (`type: location`). Each carries a stable `id`
//! (uuid) so other features (notably `inventory`) reference
//! them by id and renames don't break links.
//!
//! This proto owns the **wire surface**: the [`Location`] model
//! (+ canonical [`Kind`] enum) and the [`LocationsService`] CRUD
//! trait the capture/management UIs (CLI, web) bind to. It is
//! wasm-clean so the web UI can talk to the service directly.
//!
//! The sibling `locations` crate sits on top of this proto and
//! owns the parse / serialize / scan side plus the disk-backed
//! [`Store`] backend — exactly like `scheduling` sits on top of
//! `scheduling-proto` and `inbox` on `inbox-proto`.

pub mod model;
pub mod service;

pub use model::{Kind, Location};
pub use service::{LocationsError, LocationsService, LocationsServiceRpc};

// architect-emitted vox bits: the async client / dispatcher /
// descriptor / serve helpers. Mount sites stitch the descriptor
// + `serve` into the org router; the web UI binds the client.
#[cfg(feature = "vox")]
pub use service::{
    LocationsServiceClient, LocationsServiceRpcDispatcher as LocationsDispatcher,
    Service as LocationsServiceBridge, layer as locations_service_layer,
    locations_service_rpc_service_descriptor as locations_service_descriptor,
    serve as serve_locations_service,
};
