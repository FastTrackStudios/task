//! Resource Library + annotations — the data layer (no UI).
//!
//! A **resource** is a read-only primary source (a song, PDF, book,
//! video) that the vault/wiki annotate and link into without editing it.
//! This crate is the headless half of the resource-annotations design,
//! built on the Logseq **two-layer** model:
//!
//! - the compact **anchor string** (`song:slug#chorus.L1`, `…#t:90`,
//!   `resource:books/x#p3.h2`) lives in the shared link graph
//!   (`links_proto::NodeRef` / `links.jsonl`), while
//! - the **geometry / metadata** (PDF rects, colour, captured text,
//!   timestamp label) lives in a per-resource **sidecar**
//!   (`<resource>.annotations.json`) — Logseq's `<pdf>.edn`.
//!
//! Modules: [`types`] (wasm-clean data), [`manifest`] (load a `type:
//! resource` frontmatter), [`sidecar`] (read/write the annotation file),
//! [`build`] (specs → links + sidecar — the authoring path), [`resolve`]
//! (anchor → seek/region/span — the read path).

pub mod backend;
pub mod build;
pub mod manifest;
pub mod resolve;
pub mod scripture_refs;
pub mod sermon;
pub mod sidecar;
pub mod transcript;
pub mod types;
pub mod walker;

pub use backend::ResourcesBackend;
pub use build::{AnnotationSpec, Built, Target, build};
pub use manifest::{load_manifest, parse_manifest};
pub use resolve::{Resolved, resolve, resolve_with};
pub use sidecar::sidecar_path;
pub use transcript::{Transcript, TranscriptSegment, parse_json3};
pub use types::{Annotation, AnnotationFile, Geometry, MediaRef, Rect, Resource, ResourceKind};
pub use walker::{LoadedResource, walk};

/// Errors from loading manifests / sidecars.
#[derive(Debug, thiserror::Error)]
pub enum ResourceError {
    #[error("io: {0}")]
    Io(String),
    #[error("yaml: {0}")]
    Yaml(String),
    #[error("json: {0}")]
    Json(String),
    #[error("no frontmatter")]
    NoFrontmatter,
}
