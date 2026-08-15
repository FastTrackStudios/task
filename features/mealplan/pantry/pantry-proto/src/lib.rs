// architect's Entity / JsonField derives emit cfg-gated blocks;
// allow at crate scope.
#![allow(unexpected_cfgs)]

//! Wire contract for the pantry suite.
//!
//! Owns the wasm-clean [`PantryItem`] model (+ its `StockEntry` /
//! `Substitution` / `SubReason` / `PantryItemDraft` value types)
//! and the [`PantryService`] `#[architect::rpc]` trait so the web
//! UI can bind the client directly. The sibling `pantry` crate
//! sits on top and owns the parse / serialize / scan side plus the
//! disk-backed `Store` backend — exactly like `cookbook` sits on
//! top of `cookbook-proto`. The native `pantry` crate re-exports
//! this proto so CLI / server paths keep working.
//!
//! `Nutrition` is reused from `cookbook-proto`. The `native`
//! feature adds the `inventory::Item` interop helpers
//! ([`PantryItem::to_item`] / [`PantryItem::from_item`]); it stays
//! off for the wasm UI.

pub mod model;
pub mod service;

pub use model::{
    DueType, FoodCategory, PantryItem, PantryItemDraft, StockEntries, StockEntry, StringList,
    SubReason, Substitution, Substitutions,
};
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
