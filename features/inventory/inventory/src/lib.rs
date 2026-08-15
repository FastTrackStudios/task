// architect's Entity derive emits cfg-gated blocks; allow
// at crate scope.
#![allow(unexpected_cfgs)]

//! `inventory` — typed view of gear/equipment pages in a
//! `vault::Vault`.
//!
//! Items are markdown files with YAML frontmatter
//! (`type: item`) recording name, category, location,
//! condition, lifecycle status, repair tasks, and free-form
//! notes. Each item references its location by `location_id`
//! (uuid) so location renames don't break the link — see the
//! `locations` crate for the place model.
//!
//! Surface:
//! - [`Item`] / [`Condition`] / [`Status`] — parsed model
//!   + canonical enums.
//! - [`Items`] — the `vault_entity::VaultEntity` mapping
//!   (frontmatter keys ↔ `Item` fields).
//! - [`parse_page`] / [`looks_like_item`] — `VaultPage` →
//!   `Item`.
//! - [`serialize_item`] / [`write_item`] — `Item` → markdown
//!   bytes (+ disk write helper).
//! - [`scan_vault`] / [`items_at`] /
//!   [`items_needing_attention`] — bulk views.
//! - [`InventoryService`] — `#[architect::rpc]` trait giving
//!   agents a uniform call surface over vox.
//! - [`Store`] — in-process `InventoryService` impl backed by
//!   a `vault::Vault` (mutex-shared with the locations store).

#![cfg(not(target_arch = "wasm32"))]

pub mod entity;
pub mod model;
pub mod parse;
pub mod scan;
pub mod service;
pub mod store;
pub mod write;

pub use entity::Items;
pub use model::{Condition, Item, Status};
pub use parse::{ParseError, looks_like_item, parse_page};
pub use scan::{items_at, items_needing_attention, scan_vault};
pub use service::{InventoryError, InventoryService};
pub use store::Store;
pub use write::{WriteError, default_item_path, serialize_item, write_item};

#[cfg(feature = "vox")]
pub use inventory_proto::{
    InventoryServiceBridge, InventoryServiceClient, inventory_service_descriptor,
    inventory_service_layer, serve_inventory_service,
};
#[cfg(feature = "vox")]
pub use service::{InventoryServiceRpc, InventoryServiceRpcDispatcher as InventoryDispatcher};
