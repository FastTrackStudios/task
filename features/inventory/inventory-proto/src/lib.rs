// architect's Entity derive emits cfg-gated blocks; allow
// at crate scope.
#![allow(unexpected_cfgs)]

//! Wire contract for the inventory feature.
//!
//! Items are gear/equipment pages — markdown files with YAML
//! frontmatter (`type: item`) recording name, category,
//! location, condition, lifecycle status, repair tasks, and
//! free-form notes. Each item references its location by
//! `location_id` (uuid) so location renames don't break the
//! link.
//!
//! This proto owns the **wire surface**: the [`Item`] model
//! (+ canonical [`Condition`] / [`Status`] enums) and the
//! [`InventoryService`] CRUD trait the capture/management UIs
//! (CLI, web) bind to. It is wasm-clean so the web UI can talk
//! to the service directly.
//!
//! The sibling `inventory` crate sits on top of this proto and
//! owns the parse / serialize / scan side plus the disk-backed
//! [`Store`](../inventory/struct.Store.html) backend — exactly
//! like `locations` sits on top of `locations-proto`.

pub mod model;
pub mod service;

pub use model::{Condition, Item, Status, StringList};
pub use service::{InventoryError, InventoryService, InventoryServiceRpc};

// architect-emitted vox bits: the async client / dispatcher /
// descriptor / serve helpers. Mount sites stitch the descriptor
// + `serve` into the org router; the web UI binds the client.
#[cfg(feature = "vox")]
pub use service::{
    InventoryServiceClient, InventoryServiceRpcDispatcher as InventoryDispatcher,
    Service as InventoryServiceBridge,
    inventory_service_rpc_service_descriptor as inventory_service_descriptor,
    layer as inventory_service_layer, serve as serve_inventory_service,
};
