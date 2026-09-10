// architect's rpc macro emits cfg-gated blocks; allow at crate scope.
#![allow(unexpected_cfgs)]

//! Wasm-clean wire contract for the generic **ordered-collection** primitive.
//!
//! A [`Collection`] is an ordered list of [`NodeRef`] items, each carrying
//! a lexorank `rank`, under a [`CollectionKind`] label the **caller**
//! supplies — one primitive instead of a service per list-shaped thing.
//!
//! A library, a setlist, a show and a playlist are all instances of this
//! with a different kind string. Those words are *not defined here*, and
//! ADR 0004 is why: a primitive that enumerates its consumers is not a
//! primitive, and an application should not need a release of the store
//! it is built on in order to name something in its own domain. What
//! this crate promises is ordering, membership and reference resolution;
//! what a kind *means* belongs to whoever wrote it. See
//! [`CollectionKind`] for the full argument and for the normalisation
//! rule that keeps two spellings of one word from splitting a store.
//!
//! The item reference type is reused wholesale from `links_proto`
//! ([`NodeRef`] + [`NodeKind`], which already knows `Song`). The sibling
//! `collection` crate owns the backing JSONL store and the lexorank
//! ordering.

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
