// architect's Entity derive emits cfg-gated blocks; allow
// at crate scope.
#![allow(unexpected_cfgs)]

//! `vault-proto` — wire contract for the vault-sync
//! feature.
//!
//! - [`Manifest`] / [`ManifestEntry`] — the file listing
//! - [`FileBytes`] / [`PutAck`] / [`IfMatch`] — file payloads
//!   + the conditional-write mode
//! - [`VaultEvent`] — live change events for subscribers
//! - [`VaultSyncError`] — trait-boundary error type
//! - [`VaultSync`] — the service trait, decorated with
//!   `#[architect::rpc]`
//!
//! The architect macro derives the async vox face from the
//! sync `VaultSync` trait: backends impl `VaultSync` directly,
//! in-process callers use it as a plain sync API, and remote
//! callers reach the same surface via the auto-emitted
//! [`VaultSyncClient`] over vox.
//!
//! Mount the server-side backend with [`serve`], or compose
//! through [`Service`] into an `architect::Services` bundle.

mod base_view;
mod collab;
mod error;
mod event;
mod file;
mod graph;
mod manifest;
mod page;
mod page_meta;
mod service;

// Per-file CRDT collaboration identity: the uuid-v5 doc-id scheme,
// the doc's container names, and the `open_collab` ack payload.
pub use base_view::{BaseGroup, BaseRowView, BaseView};
pub use collab::{
    COLLAB_META_CONTAINER, COLLAB_META_FLUSHED_SHA, COLLAB_TEXT_CONTAINER, CollabAck,
    VAULT_DOC_NAMESPACE, collab_doc_id,
};
pub use error::VaultSyncError;
// Link-graph read surface (separate service from `VaultSync`).
// The prelude carries the architect-emitted vox bits under
// glob-safe names: `VaultGraphClient`, `VaultGraphRpcDispatcher`,
// `vault_graph_rpc_service_descriptor`, `vault_graph_serve`,
// `vault_graph_layer`, `VaultGraphService`.
pub use event::{VaultChange, VaultEvent};
pub use file::{FileBytes, IfMatch, PutAck};
pub use graph::prelude::*;
pub use graph::{GraphLink, GraphUnresolved, TagCount};
pub use manifest::{Manifest, ManifestEntry};
pub use page::VaultPage;
pub use page_meta::{FolderIndex, PageMeta};
pub use service::{VaultSync, VaultSyncRpc};

// architect-emitted vox bits from the auto-generated mirror
// trait. Re-exported with shorter aliases (`Dispatcher`,
// `descriptor`) so consumer mounting code reads
// `vault_proto::descriptor()` and
// `vault_proto::serve(state)` rather than juggling the
// underscored mirror names directly. Mirrors the daw
// `marker::serve(Reaper)` shape.
#[cfg(feature = "vox")]
pub use service::{
    Service, VaultSyncClient, VaultSyncRpcDispatcher as Dispatcher, layer, serve,
    vault_sync_rpc_service_descriptor as descriptor,
};

// `#[subscribe] fn changes` stream sibling — live vault changes.
// Mount `vault_proto::stream_layer(backend)` next to
// `vault_proto::serve(backend)`; clients subscribe through
// `VaultSyncStreamClient::changes(tx)` (see `architect::use_stream`).
#[cfg(feature = "vox")]
pub use service::{
    VaultSyncStreamClient, VaultSyncStreamSource, stream_layer, stream_serve,
    vault_sync_stream_service_descriptor as stream_descriptor,
};
