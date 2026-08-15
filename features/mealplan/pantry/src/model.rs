//! The `PantryItem` model (and its `StockEntry` / `Substitution`
//! / `SubReason` / `PantryItemDraft` value types + the
//! JSON-column newtypes) now live in the wasm-clean
//! [`pantry_proto`] crate so the web UI can bind to the wire
//! contract directly. Re-exported here so the existing
//! `pantry::model::*` paths keep working.
//!
//! The `inventory::Item` interop helpers
//! ([`PantryItem::to_item`] / [`PantryItem::from_item`]) come
//! from `pantry-proto`'s `native` feature, which the native
//! `pantry` crate enables.

pub use pantry_proto::model::{
    DueType, FoodCategory, PantryItem, PantryItemDraft, StockEntries, StockEntry, StringList,
    SubReason, Substitution, Substitutions,
};
