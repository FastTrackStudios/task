// architect's rpc macro emits cfg-gated blocks; allow at crate scope.
#![allow(unexpected_cfgs)]

//! Wasm-clean wire contract for the generic **ordered-collection** primitive.
//!
//! A [`Collection`] is an ordered list of [`NodeRef`] items, each carrying
//! a lexorank `rank`. A song Library, a Setlist, a Show, and a Playlist
//! are all instances of this with a different [`CollectionKind`] — one
//! primitive instead of a service per list-shaped thing.
//!
//! The item reference type is reused wholesale from `links_proto`
//! ([`NodeRef`] + [`NodeKind`], which already knows `Song`). The sibling
//! `collection` crate owns the backing JSONL store and the lexorank
//! ordering. See `plans/knowledge-primitives.md`.

pub mod model;
pub mod service;

pub use links_proto::{NodeKind, NodeRef};
pub use model::{Collection, CollectionItem, CollectionKind};
pub use service::{CollectionError, CollectionService, Placement};

#[cfg(feature = "vox")]
pub use service::{
    CollectionServiceClient, CollectionServiceRpcDispatcher as CollectionDispatcher,
    Service as CollectionServiceBridge,
    collection_service_rpc_service_descriptor as collection_service_descriptor,
    layer as collection_service_layer, serve as serve_collection_service,
};
