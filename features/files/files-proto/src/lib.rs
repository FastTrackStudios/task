// architect's Entity/rpc derives emit cfg-gated blocks; allow at crate
// scope (same convention as `milestone-proto` / `task-proto`).
#![allow(unexpected_cfgs)]
// `#[architect::rpc]` writes `async fn` into its traits. Without the
// `vox` feature the macro leaves them bare, so the lint fires once per
// method — 119 times — for traits only ever implemented in this
// workspace, which is the case the lint's own note says to suppress.
#![allow(async_fn_in_trait)]

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
pub mod error;
pub mod id;
pub mod model;
pub mod path;
pub mod service;

pub use consts::{GIT_DIR, MARKER_FILE, STORE_DIR};
pub use model::{
    AnnotationPoint, AnnotationStroke, BrowseEntry, ChainEntry, CheckpointInfo, DivergenceChoice,
    DivergenceInfo, DivergenceSide, FileRootInfo, GcReport, HydrationChange, HydrationReport,
    NamedVersion, NewReviewComment, ProjectVersion, RenditionInfo, RenditionKind, RestartMode,
    Review, ReviewComment, RootFlavor, SavePoint, SnapshotInfo, TreeNode, VersionRef,
};
// v1, re-exported at the crate root it has always occupied so downstream
// is untouched. `service::FilesEvent` is the *new* nested stream; the
// root re-export flips to it when the last lane has migrated.
pub use service::legacy::{FilesError, FilesEvent, FilesService};

// The lane traits — the target surface. See [`service`] for the map from
// each to its section of `features/files/spec/files.md`.
pub use error::FilesFault;
pub use id::{
    ActivityId, CommentId, ContentId, DeviceId, GrantId, PrincipalId, ProjectVersionId, ReviewId,
    RootId, ShareId, SnapshotId, UploadId, VersionId,
};
pub use path::{PathError, RootPath, TreePath};
pub use service::{
    AccessService, CurationService, MediaService, OrganiseService, ReviewService, RootsService,
    SearchService, SyncService, TreeService, UploadService, VersionService, WriteService,
};

// architect-emitted vox bits: the async client / dispatcher / descriptor
// / serve helpers. Mount sites stitch the descriptor + `serve` into the
// org router; the web UI binds the client.
#[cfg(feature = "vox")]
pub use service::legacy::{
    FilesServiceClient, FilesServiceRpcDispatcher as FilesDispatcher,
    Service as FilesServiceBridge,
    files_service_rpc_service_descriptor as files_service_descriptor, layer as files_service_layer,
    serve as serve_files_service,
};

// `#[subscribe] fn events` stream sibling — live root/checkpoint
// changes. Mount `files_service_stream_layer(backend)` next to the base
// service; subscribers drive a `FilesServiceStreamClient`.
#[cfg(feature = "vox")]
pub use service::legacy::{
    FilesServiceStream, FilesServiceStreamClient, FilesServiceStreamSource,
    files_service_stream_service_descriptor as files_stream_descriptor,
    stream_layer as files_service_stream_layer, stream_serve as serve_files_service_stream,
};

// The v2 lanes' architect-emitted vox bits, one group per module.
// Each lane mounts independently — a descriptor plus `serve` stitched
// into the org router — so migrating one does not disturb the rest.
// A lane is granted in `permits.rs` in the same change that mounts it,
// or every one of its methods fails closed in production.
#[cfg(feature = "vox")]
pub use service::roots::{
    RootsServiceClient, RootsServiceRpcDispatcher as RootsDispatcher,
    roots_service_rpc_service_descriptor as roots_descriptor,
    layer as roots_layer, serve as serve_roots,
};
#[cfg(feature = "vox")]
pub use service::tree::{
    TreeServiceClient, TreeServiceRpcDispatcher as TreeDispatcher,
    tree_service_rpc_service_descriptor as tree_descriptor,
    layer as tree_layer, serve as serve_tree,
};
#[cfg(feature = "vox")]
pub use service::write::{
    WriteServiceClient, WriteServiceRpcDispatcher as WriteDispatcher,
    write_service_rpc_service_descriptor as write_descriptor,
    layer as write_layer, serve as serve_write,
};
#[cfg(feature = "vox")]
pub use service::upload::{
    UploadServiceClient, UploadServiceRpcDispatcher as UploadDispatcher,
    upload_service_rpc_service_descriptor as upload_descriptor,
    layer as upload_layer, serve as serve_upload,
};
#[cfg(feature = "vox")]
pub use service::version::{
    VersionServiceClient, VersionServiceRpcDispatcher as VersionDispatcher,
    version_service_rpc_service_descriptor as version_descriptor,
    layer as version_layer, serve as serve_version,
};
#[cfg(feature = "vox")]
pub use service::curation::{
    CurationServiceClient, CurationServiceRpcDispatcher as CurationDispatcher,
    curation_service_rpc_service_descriptor as curation_descriptor,
    layer as curation_layer, serve as serve_curation,
};
#[cfg(feature = "vox")]
pub use service::sync::{
    SyncServiceClient, SyncServiceRpcDispatcher as SyncDispatcher,
    sync_service_rpc_service_descriptor as sync_descriptor,
    layer as sync_layer, serve as serve_sync,
};
#[cfg(feature = "vox")]
pub use service::media::{
    MediaServiceClient, MediaServiceRpcDispatcher as MediaDispatcher,
    media_service_rpc_service_descriptor as media_descriptor,
    layer as media_layer, serve as serve_media,
};
#[cfg(feature = "vox")]
pub use service::search::{
    SearchServiceClient, SearchServiceRpcDispatcher as SearchDispatcher,
    search_service_rpc_service_descriptor as search_descriptor,
    layer as search_layer, serve as serve_search,
};
#[cfg(feature = "vox")]
pub use service::access::{
    AccessServiceClient, AccessServiceRpcDispatcher as AccessDispatcher,
    access_service_rpc_service_descriptor as access_descriptor,
    layer as access_layer, serve as serve_access,
};
#[cfg(feature = "vox")]
pub use service::organise::{
    OrganiseServiceClient, OrganiseServiceRpcDispatcher as OrganiseDispatcher,
    organise_service_rpc_service_descriptor as organise_descriptor,
    layer as organise_layer, serve as serve_organise,
};
#[cfg(feature = "vox")]
pub use service::review::{
    ReviewServiceClient, ReviewServiceRpcDispatcher as ReviewDispatcher,
    review_service_rpc_service_descriptor as review_descriptor,
    layer as review_layer, serve as serve_review,
};
