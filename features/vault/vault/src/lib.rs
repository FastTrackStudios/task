//! `vault` — facade for the vault feature.
//!
//! Every consumer outside `features/vault/` should depend on this
//! crate. Internal vault-feature crates (`vault-live`,
//! `vault-obsidian`, eventually `vault-graph`) depend on each
//! other directly and skip the facade.
//!
//! Feature flags:
//! - `live` (default) — pull in the filesystem backend, in-memory
//!   `Vault` snapshot, watcher, and block index from `vault-live`.
//! - `obsidian` (default) — pull in the Obsidian translation
//!   layer (parsed helpers, `open_as_vault`, `open_as_backend`).
//! - `vox` (default) — propagate `vault-proto`'s vox feature so
//!   the architect-emitted `VaultSyncClient` + dispatcher are
//!   available.
//!
//! Wasm consumers typically want
//! `default-features = false, features = ["vox"]` — that ships
//! the wire trait + payloads + generated client without dragging
//! the native fs deps in.

// ── Wire trait + payload types ────────────────────────────────────
// Unconditional; these are wasm-clean and the smallest unit any
// consumer is likely to need.
pub use vault_proto::{
    FileBytes, IfMatch, Manifest, ManifestEntry, PutAck, VaultSync, VaultSyncError,
};

/// Wire change-event type (`Put` / `Delete` / `Resync`). Aliased
/// because `vault_live::VaultEvent` (the filesystem-watcher event)
/// collides at the facade boundary; this name keeps the two
/// distinguishable when both features are on.
pub use vault_proto::VaultEvent as SyncEvent;

// architect-emitted vox bits (client, server bridge, descriptor,
// mount verb). Gated behind the facade's `vox` feature, which
// propagates to vault-proto.
#[cfg(feature = "vox")]
pub use vault_proto::{
    Dispatcher as SyncDispatcher, Service as SyncService, VaultSyncClient, VaultSyncRpc,
    descriptor, layer, serve,
};

// ── Link-graph read service (`VaultGraph`) ────────────────────────
// Wire trait + payloads are wasm-clean and unconditional, like
// `VaultSync` above. The canonical backend (`GraphBackend`, over
// `LinkIndex`) ships with the obsidian layer below.
pub use vault_proto::{GraphLink, GraphUnresolved, TagCount, VaultGraph};

#[cfg(feature = "vox")]
pub use vault_proto::{
    VaultGraphClient, VaultGraphRpc, VaultGraphRpcDispatcher as GraphDispatcher,
    VaultGraphService as GraphService, vault_graph_layer as graph_layer,
    vault_graph_rpc_service_descriptor as graph_descriptor, vault_graph_serve as graph_serve,
};

#[cfg(feature = "obsidian")]
pub use vault_obsidian::GraphBackend;

// ── Live filesystem implementation ────────────────────────────────
/// Sub-module access into the live backend implementation, when
/// callers need `vault::sync::WatcherHandle` or the like.
#[cfg(feature = "live")]
pub use vault_live::sync;
#[cfg(feature = "live")]
pub use vault_live::watcher;
#[cfg(feature = "live")]
pub use vault_live::{
    Backend, BlockIndex, BlockLocation, LoadError, MutateError, PageSink, PropertyTypes, SaveError,
    Vault, VaultBase, VaultEntry, VaultEntryKind, VaultEvent, VaultLookupView, VaultPage,
    WatchError, append_to_page, bind_sink, create_page, delete_page, delete_page_at, move_page_at,
    save_page, save_page_at, sink_for, unbind_sink, walk_vault, watch, write_atomic,
};

// ── Obsidian translation layer ────────────────────────────────────
/// Obsidian-aware helpers: parsed page reads (outline, tasks,
/// properties, aliases), `.base` query DSL, link index, and the
/// `open_as_vault` / `open_as_backend` translation entry points.
#[cfg(feature = "obsidian")]
pub use vault_obsidian as obsidian;
