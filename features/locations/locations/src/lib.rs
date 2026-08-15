// architect's Entity derive emits cfg-gated blocks; allow
// at crate scope.
#![allow(unexpected_cfgs)]

//! `locations` — typed view of place pages in a `vault::Vault`.
//!
//! Locations are markdown files with YAML frontmatter
//! (`type: location`). Each carries a stable `id` (uuid) so
//! other features (notably `inventory`) reference them by id
//! and renames don't break links.
//!
//! Surface:
//! - [`Location`] / [`Kind`] — the parsed model + canonical
//!   `kind` enum.
//! - [`Locations`] — the `vault_entity::VaultEntity` mapping
//!   (frontmatter keys ↔ `Location` fields).
//! - [`parse_page`] / [`looks_like_location`] — `VaultPage`
//!   → `Location`.
//! - [`serialize_location`] / [`write_location`] —
//!   `Location` → markdown bytes (+ disk write helper).
//! - [`scan_vault`] — collect every location page from a
//!   `vault::Vault`.
//! - [`LocationsService`] — `#[architect::rpc]` trait giving
//!   agents a uniform call surface over vox.
//! - [`Store`] — in-process `LocationsService` impl backed
//!   by a `vault::Vault`.

#![cfg(not(target_arch = "wasm32"))]

pub mod entity;
pub mod model;
pub mod parse;
pub mod scan;
pub mod service;
pub mod store;
pub mod write;

pub use entity::Locations;
pub use model::{Kind, Location};
pub use parse::{ParseError, looks_like_location, parse_page};
pub use scan::scan_vault;
pub use service::{LocationsError, LocationsService};
pub use store::Store;
pub use write::{WriteError, default_location_path, serialize_location, write_location};

#[cfg(feature = "vox")]
pub use locations_proto::{
    LocationsServiceBridge, LocationsServiceClient, locations_service_descriptor,
    locations_service_layer, serve_locations_service,
};
#[cfg(feature = "vox")]
pub use service::{LocationsServiceRpc, LocationsServiceRpcDispatcher as LocationsDispatcher};
