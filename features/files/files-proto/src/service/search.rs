//! Finding things by what is in them — `files.index.*`.
//!
//! The query surface across everything belongs to charter #17. This lane
//! is what Files owes it: content extracted as bytes arrive, results that
//! address regions rather than files, and indexes that are ordinary
//! portable files.
//!
//! Extraction runs on hardware the operator controls, using open models,
//! on the machine holding the bytes. A third-party service may be
//! configured as an addition and must never be a prerequisite: with no
//! external credential set, everything here still works.

use chrono::{DateTime, Utc};
use facet::Facet;

use crate::error::FilesFault;
use crate::id::RootId;
use crate::path::RootPath;
use crate::service::media::Region;

/// What was pulled out of a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Facet)]
#[repr(u8)]
pub enum Extract {
    /// Text of a document or PDF.
    Text,
    /// Speech transcribed from audio or video.
    Speech,
    /// A description of what is visually in a shot.
    Vision,
    /// Camera, lens, codec, timecode.
    Technical,
}

/// How far extraction has got for one file.
#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(C)]
pub struct ExtractState {
    pub root_id: RootId,
    pub path: RootPath,
    pub kind: Extract,
    pub done: bool,
    /// Failure degrades this file's findability and never blocks its
    /// storage, sync or playback.
    pub failed: Option<String>,
    /// Where the portable sidecar lives — plain, readable without this
    /// application, regenerable from the source.
    pub sidecar: Option<RootPath>,
    pub updated_at: DateTime<Utc>,
}

/// A query. Free text, optionally narrowed.
#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(C)]
pub struct Query {
    pub text: String,
    /// Restrict to one root. `None` searches everything reachable.
    pub root_id: Option<RootId>,
    /// Restrict to a subtree.
    pub under: Option<RootPath>,
    pub kinds: Vec<Extract>,
    pub limit: Option<u32>,
}

/// One hit.
///
/// A hit addresses a **region**, not a file: the seconds of footage with
/// a dog in them, the page of the PDF, the block of the note. Opening it
/// lands there rather than at the top of a two-hour file.
#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(C)]
pub struct Hit {
    pub root_id: RootId,
    pub path: RootPath,
    pub region: Region,
    pub kind: Extract,
    /// The matching text, or the description that matched.
    pub excerpt: String,
    pub score: f32,
}

#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(u8)]
pub enum SearchEvent {
    /// A file became searchable, or failed to.
    Extracted(ExtractState),
}

#[architect::rpc]
pub trait SearchService {
    /// Search extracted content. Results are regions.
    async fn search(&self, query: Query) -> Result<Vec<Hit>, FilesFault>;

    /// Extraction state for one file, across kinds.
    async fn extract_state(
        &self,
        root_id: RootId,
        path: RootPath,
    ) -> Result<Vec<ExtractState>, FilesFault>;

    /// What is still outstanding in a root — the honest answer to "is it
    /// all searchable yet".
    async fn pending(&self, root_id: RootId) -> Result<Vec<ExtractState>, FilesFault>;

    /// Request extraction now, ahead of the queue. Re-running on
    /// unchanged content is a no-op.
    async fn extract(
        &self,
        root_id: RootId,
        paths: Vec<RootPath>,
        kinds: Vec<Extract>,
    ) -> Result<Vec<ExtractState>, FilesFault>;
}
