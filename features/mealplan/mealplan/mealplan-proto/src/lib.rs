// architect's Entity / JsonField derives emit cfg-gated blocks;
// allow at crate scope.
#![allow(unexpected_cfgs)]

//! Wire contract for the mealplan facade.
//!
//! Owns the wasm-clean [`Meal`] calendar model (+ `PantryDeduction`
//! / `Slot` / `Status`), the recipe-fulfillment wire types
//! ([`Fulfillment`] / [`Shortage`] / …), and the [`MealplanService`]
//! `#[architect::rpc]` trait so the web UI can bind the client
//! directly. The sibling `mealplan` crate sits on top and owns the
//! parse / serialize / scan side, the disk-backed `Store`, the
//! cook + fulfillment domain logic, and the `nutrition_total`
//! aggregation — re-exporting this proto so existing
//! `mealplan::Meal` / `mealplan::Fulfillment` paths keep working.

pub mod fulfillment;
pub mod model;
pub mod service;
pub mod shopping;

pub use fulfillment::{
    CookReceipt, DeductionLine, Fulfillment, HaveLine, Shortage, ShortageReason, SkipReason,
    SkippedIngredient, SubstitutionSource, SubstitutionSuggestion,
};
pub use model::{Meal, PantryDeduction, PantryDeductions, Slot, Status, StringList};
pub use service::{MealplanError, MealplanService, MealplanServiceRpc};
pub use shopping::{
    EntryStatus, ShoppingEntries, ShoppingEntry, ShoppingError, ShoppingList, ShoppingService,
    ShoppingServiceRpc,
};

#[cfg(feature = "vox")]
pub use shopping::{
    Service as ShoppingServiceBridge, ShoppingServiceClient,
    ShoppingServiceRpcDispatcher as ShoppingDispatcher, layer as shopping_service_layer,
    serve as serve_shopping_service,
    shopping_service_rpc_service_descriptor as shopping_service_descriptor,
};

#[cfg(feature = "vox")]
pub use service::{
    MealplanServiceClient, MealplanServiceRpcDispatcher as MealplanDispatcher,
    Service as MealplanServiceBridge, layer as mealplan_service_layer,
    mealplan_service_rpc_service_descriptor as mealplan_service_descriptor,
    serve as serve_mealplan_service,
};
