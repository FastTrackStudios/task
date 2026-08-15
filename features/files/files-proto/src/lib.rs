// architect's Entity/rpc derives emit cfg-gated blocks; allow at crate
// scope (same convention as `milestone-proto` / `task-proto`).
#![allow(unexpected_cfgs)]

//! Wire contract for the Files feature (GitHub issue #255, ADR 0001 —
//! `apps/task/docs/adr/0001-files-version-store-jj-cas.md`). This
//! ticket (#259) is the RPC surface v1: create a File Root from an
//! existing folder, browse it, read a file's version chain, trigger a
//! checkpoint on demand. Issue #261 adds the curated half — Named
//! Versions and Project Versions, which are *Vault* entities
//! referencing `(root id, change id)` rather than store constructs,
//! plus the Vault-protected GC pass that makes them immortal.
//!
//! This proto owns the wasm-clean wire surface — [`model`]'s types plus
//! the [`service::FilesService`] trait. The sibling `files` crate sits
//! on top and owns the version-store-backed [`FilesBackend`](../files/struct.FilesBackend.html)
//! side, exactly like `milestone` sits on top of `milestone-proto`.

pub mod consts;
pub mod model;
pub mod service;

pub use consts::{GIT_DIR, MARKER_FILE, STORE_DIR};
pub use model::{
    AnnotationPoint, AnnotationStroke, BrowseEntry, ChainEntry, CheckpointInfo, DivergenceChoice,
    DivergenceInfo, DivergenceSide, FileRootInfo, GcReport, HydrationChange, HydrationReport,
    NamedVersion, NewReviewComment, ProjectVersion, RenditionInfo, RenditionKind, RestartMode,
    Review, ReviewComment, RootFlavor, SavePoint, SnapshotInfo, TreeNode, VersionRef,
};
pub use service::{FilesError, FilesEvent, FilesService};

// architect-emitted vox bits: the async client / dispatcher / descriptor
// / serve helpers. Mount sites stitch the descriptor + `serve` into the
// org router; the web UI binds the client.
#[cfg(feature = "vox")]
pub use service::{
    FilesServiceClient, FilesServiceRpcDispatcher as FilesDispatcher,
    Service as FilesServiceBridge,
    files_service_rpc_service_descriptor as files_service_descriptor, layer as files_service_layer,
    serve as serve_files_service,
};

// `#[subscribe] fn events` stream sibling — live root/checkpoint
// changes. Mount `files_service_stream_layer(backend)` next to the base
// service; subscribers drive a `FilesServiceStreamClient`.
#[cfg(feature = "vox")]
pub use service::{
    FilesServiceStream, FilesServiceStreamClient, FilesServiceStreamSource,
    files_service_stream_service_descriptor as files_stream_descriptor,
    stream_layer as files_service_stream_layer, stream_serve as serve_files_service_stream,
};
