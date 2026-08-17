// architect's HasDispatcher/rpc derives emit cfg-gated blocks; allow at
// crate scope (same convention as `milestone` / `task`).
#![allow(unexpected_cfgs)]

//! Server-side half of the Files feature (issue #259, ADR 0001 —
//! `apps/task/docs/adr/0001-files-version-store-jj-cas.md`). The
//! wasm-clean wire surface ([`files_proto::FilesService`] + its model
//! types) lives in the sibling `files-proto` crate; this crate is
//! [`FilesBackend`] — the version-store-backed implementation — plus
//! the plumbing it needs: [`registry`] (root identity, persisted
//! alongside the version stores) and [`repo_open`] (opening/reopening
//! a root's jj repo).
//!
//! Issue #261 adds the curated half: [`VaultVersions`] — Named
//! Versions and Project Versions as ordinary Vault entities (see
//! [`entity`]) referencing `(root id, change id)` — and the GC pass
//! that resolves those references into the version store's protect
//! set, which is what makes a named deliverable immortal.
//!
//! Issue #260 adds the automatic half: `files_domain::cadence` decides when a
//! root's session takes an auto-snapshot and when it ends in a Session
//! checkpoint, [`ignore`] holds the per-root Ignore set that keeps junk
//! out of the store entirely, and [`certify`] is the stat sandwich that
//! stops a file being written right now from entering a version torn.

mod backend;
mod badges;
pub mod certify;
mod checkpoint;
mod consts;
mod content;
mod durable;
mod entity;
mod error;
mod git_root;
pub mod hydration;
pub mod ignore;
pub mod lane;
mod org_tree;
mod registry;
/// Opening (and reopening) a root's version-store repo. Public so a
/// test — or a future sibling crate, e.g. the cadence engine (#260) —
/// can reach the same repo the backend serves, without duplicating
/// jj-lib's loader wiring.
pub mod repo_open;
mod restart;
mod scan;
pub mod stub;
mod tarball;
mod transcode;
mod versions;
pub mod watcher;

pub use backend::{Captured, FilesBackend, LocationBoundaries, MaterializeReport, SyncTreeMeta};
// The cadence engine moved to `files-domain`: it is a state machine
// about time, and needs neither jj-lib nor the version store. Re-exported
// here so callers and mount sites are unaffected.
pub use entity::{NamedVersions, ProjectVersions};
pub use error::{Error, Result};
pub use files_domain::cadence;
pub use files_domain::cadence::{CadenceConfig, CadenceEngine, Clock, SystemClock, TestClock};
pub use watcher::{ActivitySink, RootWatcher};
// ── The facade ─────────────────────────────────────────────────────
//
// `files` is the only crate a consumer names. `files-proto` holds the
// wire contract and `files-domain` the pure decisions, but splitting a
// feature into crates is an internal matter — an app that has to know
// which of them a type lives in is coupled to a layout it should be
// free to ignore, and every such import is a rename waiting to break
// somebody else's build.
//
// So everything a caller needs is re-exported here, at the path it has
// in `files-proto`: `files::service::tree::TreeService`,
// `files::RootPath`, `files::id::PrincipalId`. The modules are
// re-exported wholesale rather than item by item, so a new type in a
// lane is reachable without a matching line here.
// `error` is not among them: this crate has its own `Error` for
// backend failures, and two things called `files::error` would be a
// coin toss at every call site. The wire fault is re-exported by name
// below as `FilesFault`, which is what callers actually reach for.
pub use files_proto::{id, model, path, service};

// The names callers reach for constantly, without a module path.
pub use files_proto::{FilesFault, PathError, RootId, RootPath, TreePath};

// The v2 lanes' architect surface: one client / dispatcher / descriptor
// / `layer` / `serve` per lane. Mount sites stitch these into the org
// router; the CLI and web UI bind the clients.
//
// Ungated: this crate has no `vox` feature — it depends on
// `files-proto/vox` unconditionally — so a `#[cfg(feature = "vox")]`
// here is always false, which is how these came to be missing from the
// facade in the first place.
pub use files_proto::{
    AccessServiceClient, CurationServiceClient, FederationServiceClient, MediaServiceClient,
    OrganiseServiceClient, ReviewServiceClient, RootsServiceClient, SearchServiceClient,
    SyncServiceClient, TreeServiceClient, UploadServiceClient, VersionServiceClient,
    WriteServiceClient,
};
pub use files_proto::{
    access_descriptor, access_layer, curation_descriptor, curation_layer, federation_descriptor,
    federation_layer, media_descriptor, media_layer, media_stream_descriptor, media_stream_layer,
    organise_descriptor, organise_layer, review_descriptor, review_layer, roots_descriptor,
    roots_layer, search_descriptor, search_layer, serve_access, serve_curation, serve_federation,
    serve_media, serve_organise, serve_review, serve_roots, serve_search, serve_sync, serve_tree,
    serve_upload, serve_version, serve_write, sync_descriptor, sync_layer, tree_descriptor,
    tree_layer, upload_descriptor, upload_layer, version_descriptor, version_layer,
    write_descriptor, write_layer,
};
// The engine-side rendition vocabulary (tag ↔ kind ↔ MIME) — the
// rendition streaming route (issue #270) parses its `{kind}` path
// segment with `from_tag` and serves `mime()` as the Content-Type.
pub use files_transcode::RenditionKind as TranscodeRenditionKind;
pub use versions::VaultVersions;

// A root's own internals, by name. Public because every *other* view of
// a live tree has to agree with `browse` about what is and is not part
// of the tree — the WebDAV bridge (`files-webdav`, issue #274) hides
// exactly these two so a mount shows the current head and nothing of
// the version store.
pub use consts::{MARKER_FILE, STORE_DIR};

pub use files_proto::{
    BrowseEntry, ChainEntry, CheckpointInfo, DivergenceChoice, DivergenceInfo, DivergenceSide,
    FileRootInfo, FilesError, FilesEvent, FilesService, GcReport, NamedVersion, ProjectVersion,
    RestartMode, RootFlavor, SavePoint, SnapshotInfo, VersionRef,
};

// architect-emitted vox bits: the async client / dispatcher / descriptor
// / serve helpers. Mount sites stitch the descriptor + `serve` into the
// org router; the CLI / web UI bind the client.
pub use files_proto::{
    FilesDispatcher, FilesServiceBridge, FilesServiceClient, files_service_descriptor,
    files_service_layer, serve_files_service,
};

// `#[subscribe] fn events` stream sibling — live root/checkpoint
// changes. Mount `files_service_stream_layer(backend)` next to the base
// service; subscribers drive a `FilesServiceStreamClient`.
pub use files_proto::{
    FilesServiceStream, FilesServiceStreamClient, FilesServiceStreamSource,
    files_service_stream_layer, files_stream_descriptor, serve_files_service_stream,
};
