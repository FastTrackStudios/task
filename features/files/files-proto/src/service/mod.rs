//! The Files RPC surface, one trait per spec section.
//!
//! This is the only Files API. It replaced a single v1 `FilesService` of
//! 37 ungrouped methods, which was deleted once every caller had moved
//! (ADR 0005).
//!
//! ## How this is split
//!
//! **The module tree mirrors the spec tree.** Each trait below owns one
//! section of `files.md`, so a requirement has exactly one plausible home
//! and `t[impl files.write.surface]` can only land in one file. Coverage
//! maps onto modules 1:1.
//!
//! | Module | Trait | Spec |
//! |---|---|---|
//! | [`roots`] | `RootsService` | `files.adopt.*`, root lifecycle |
//! | [`tree`] | `TreeService` | `files.catalogue.*`, `files.live.*` |
//! | [`write`] | `WriteService` | `files.write.surface` |
//! | [`upload`] | `UploadService` | `files.write.upload` |
//! | [`version`] | `VersionService` | `files.version.*`, `files.concurrency.*` |
//! | [`curation`] | `CurationService` | named and project versions |
//! | [`federation`] | `FederationService` | `files.topology.federation` |
//! | [`sync`] | `SyncService` | `files.facet.*`, `files.ignore.*`, `files.sync.*`, `files.device.*` |
//! | [`media`] | `MediaService` | renditions, `files.handoff.*` |
//! | [`search`] | `SearchService` | `files.index.*` |
//! | [`access`] | `AccessService` | `files.access.*` |
//! | [`organise`] | `OrganiseService` | `files.organise.*` |
//! | [`review`] | `ReviewService` | the guest lane |
//!
//! One trait per module is not a preference: `#[architect::rpc]` emits
//! unqualified `serve` / `layer` / descriptor helpers, so a module holds
//! exactly one service. Same constraint `files-storage-proto` works under.
//!
//! ## Conventions
//!
//! - **Every method takes 4 params or fewer** — Facet's constraint. Past
//!   two, prefer a request struct: `move` already sits at the ceiling
//!   with `(root, from, to, policy)`, so the next option would break the
//!   signature rather than extend it.
//! - **Confinement is structural, not parameterised.** The org is the
//!   backend's identity, never an argument — the discipline
//!   `files-storage`'s org lane already applies. [`RootPath`] carries the
//!   same idea down to paths.
//! - **A mutation returns the event it emitted**, so a caller can apply
//!   optimistically and fold the same type from the stream.
//! - **One stream, nested payloads.** [`FilesEvent`] stays a single
//!   subscription — subscribers want one connection — with a variant per
//!   lane rather than thirty flat ones.
//!
//! ⚠️ Every new method needs its `permits.rs` row in the same change, or
//! it fails closed in production.

pub mod access;
pub mod curation;
pub mod enrollment;
pub mod federation;
pub mod media;
pub mod organise;
pub mod review;
pub mod roots;
pub mod search;
pub mod sync;
pub mod tree;
pub mod upload;
pub mod version;
pub mod write;

use facet::Facet;

pub use access::AccessService;
pub use curation::CurationService;
pub use enrollment::DeviceEnrollmentService;
pub use federation::FederationService;
pub use media::MediaService;
pub use organise::OrganiseService;
pub use review::ReviewService;
pub use roots::RootsService;
pub use search::SearchService;
pub use sync::SyncService;
pub use tree::TreeService;
pub use upload::UploadService;
pub use version::VersionService;
pub use write::WriteService;

/// Everything that happens to a root, on one subscription.
///
/// Nested rather than flat: v1's ten variants would be thirty once the
/// spec's lanes land, and a subscriber that only cares about writes
/// should be able to match one arm rather than twelve.
///
/// The no-snapshot contract is unchanged — fetch current state once
/// *after* subscribing, then fold these in.
#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(u8)]
pub enum FilesEvent {
    Root(roots::RootEvent),
    Tree(tree::TreeEvent),
    Write(write::WriteEvent),
    Upload(upload::UploadEvent),
    Version(version::VersionEvent),
    Curation(curation::CurationEvent),
    Federation(federation::FederationEvent),
    Sync(sync::SyncEvent),
    Media(media::MediaEvent),
    Search(search::SearchEvent),
    Access(access::AccessEvent),
    Organise(organise::OrganiseEvent),
    Review(review::ReviewEvent),
}
