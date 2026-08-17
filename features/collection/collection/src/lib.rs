// architect's rpc/HasDispatcher derives emit cfg-gated blocks.
#![allow(unexpected_cfgs)]

//! `collection` — backing store for the generic ordered-collection primitive.
//!
//! Sits on top of the wasm-clean [`collection_proto`] contract (same split
//! as `links` over `links-proto`). [`Store`] is a JSONL-file-backed
//! [`collection_proto::CollectionService`] impl; item ordering reuses
//! `vault_live::lexorank` fractional-index keys. See the
//! knowledge-primitives design.

#![cfg(not(target_arch = "wasm32"))]

pub mod store;

pub use collection_proto::{
    Collection, CollectionError, CollectionItem, CollectionKind, CollectionService, NodeKind,
    NodeRef, Placement,
};
pub use store::Store;

// architect-emitted vox bits, re-exported for the server mount.
#[cfg(feature = "vox")]
pub use collection_proto::{collection_service_descriptor, serve_collection_service};
