// architect's rpc/derive macros emit cfg-gated blocks; allow at crate
// scope.
#![allow(unexpected_cfgs)]

//! Wire contract for the tagging feature.
//!
//! Owns [`TagIcon`] (the user-chosen icon a tag carries), the [`Tag`]
//! registry entity (name → icon/color decoration), and the
//! [`service::TagService`] CRUD surface. Tags are referenced by **name**
//! from entity markdown frontmatter (`tags: [food]`); this registry just
//! decorates those names so the calendar / lists can show an icon.
//!
//! Wasm-clean baseline (Facet types) so the web UI's icon picker +
//! render helper bind directly; enable `vox` for the architect-emitted
//! RPC client + dispatcher. Evolved from the half-built `label` feature
//! — the tagging-and-icons design.

pub mod error;
pub mod service;
pub mod tag;
pub mod tag_icon;

pub use error::TagError;
pub use service::TagService;
pub use tag::Tag;
pub use tag_icon::TagIcon;

// architect-emitted vox bits: the async client / dispatcher / descriptor
// for the capability. Mount sites stitch the descriptor + `serve` into
// the org router.
#[cfg(feature = "vox")]
pub use service::tags::{
    Service as TagServiceBridge, TagServiceClient, TagServiceRpc, TagServiceRpcDispatcher,
    layer as tag_service_layer, serve as serve_tag_service,
};
