// architect's rpc/HasDispatcher derives emit cfg-gated blocks.
#![allow(unexpected_cfgs)]

//! `links` — backing store for the generalized typed-link primitive.
//!
//! Sits on top of the wasm-clean [`links_proto`] contract (same split as
//! `locations` over `locations-proto`). [`Store`] is a JSONL-file-backed
//! [`links_proto::LinksService`] impl.

#![cfg(not(target_arch = "wasm32"))]

pub mod store;

pub use links_proto::{
    Confidence, LinksError, LinksService, NodeKind, NodeRef, Provenance, Relation, TypedLink,
    Visibility,
};
pub use store::Store;

// architect-emitted vox bits, re-exported for the server mount.
#[cfg(feature = "vox")]
pub use links_proto::{links_service_descriptor, serve_links_service};
