// architect's Entity derive emits cfg-gated blocks; allow
// at crate scope.
#![allow(unexpected_cfgs)]

//! `pantry` — typed view of food-on-hand pages in a
//! `vault::Vault`.
//!
//! A pantry page IS an inventory page (`type: item` +
//! `tags: [..., pantry]`). One physical thing maps to one
//! markdown file visible in both the gear inventory list
//! and the food list. The crate adds the food-specific
//! surface — qty / unit / expiry / nutrition / restock
//! threshold — on top of inventory's identity + locator
//! fields, and exposes typed convenience mutators
//! ([`PantryService::consume`], [`PantryService::restock`],
//! [`PantryService::open`]) used by mealplan when meals are
//! cooked or groceries arrive.
//!
//! Nutrition is reused from the cookbook crate so recipes,
//! pantry stock, and (future) fitness calorie logs all
//! speak the same currency.

#![cfg(not(target_arch = "wasm32"))]

pub mod entity;
pub mod lookup;
pub mod model;
pub mod parse;
pub mod scan;
pub mod service;
pub mod store;
pub mod units;
pub mod write;

pub use entity::PantryItems;
pub use lookup::{LookupError, lookup_external};
pub use model::{
    DueType, FoodCategory, PantryItem, PantryItemDraft, StockEntry, SubReason, Substitution,
};
pub use parse::{ParseError, looks_like_pantry_item, parse_page};
pub use scan::{expired, expiring_within, low_stock, scan_vault};
pub use service::{
    BarcodeResolution, ConsumeReceipt, EntryDebit, PantryError, PantryService, PantryServiceRpc,
};

#[cfg(feature = "vox")]
pub use service::{
    PantryServiceClient, PantryServiceRpcDispatcher as PantryDispatcher,
    Service as PantryServiceBridge, layer as pantry_service_layer,
    pantry_service_rpc_service_descriptor as pantry_service_descriptor,
    serve as serve_pantry_service,
};
pub use store::Store;
pub use units::{CountUnit, MassUnit, PackageUnit, Unit, VolumeUnit, convert_str};
pub use write::{WriteError, default_pantry_path, serialize_pantry_item, write_pantry_item};
