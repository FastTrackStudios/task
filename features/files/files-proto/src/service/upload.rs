//! Getting content in — `files.write.upload`.
//!
//! Uploads survive reality: chunked, resumable across a dropped
//! connection, transferring only chunks not already held. Content already
//! in the store transfers nothing at all, which is why an engineer
//! re-uploading a session they exported twice pays for it once.
//!
//! A collision **asks**. Three outcomes, none of them chosen for the
//! user — the one rule this lane exists to enforce.

use chrono::{DateTime, Utc};
use facet::Facet;
use serde::{Deserialize, Serialize};

use crate::error::FilesFault;
use crate::id::{ContentId, RootId, UploadId};
use crate::path::RootPath;
use crate::service::tree::CatalogueEntry;
use crate::service::write::OnConflict;

/// What the client wants to put where.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct UploadSpec {
    pub root_id: RootId,
    pub path: RootPath,
    pub size: u64,
    /// The client's own content address, when it computed one. Lets the
    /// server answer "already have it" before a byte moves.
    pub content: Option<ContentId>,
    pub modified_at: Option<DateTime<Utc>>,
}

/// What the server wants, having compared the spec against what it holds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct UploadPlan {
    pub upload_id: UploadId,
    /// Byte ranges still needed, in order. **Empty means the server
    /// already holds this content** — complete immediately and no bytes
    /// move.
    pub needed: Vec<ChunkRange>,
    /// Where to send them.
    pub lane: String,
    /// Set when the destination is occupied. The upload proceeds; the
    /// choice is made at [`UploadService::complete`], so a client can
    /// finish transferring while a human decides.
    pub conflict: Option<Conflict>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct ChunkRange {
    pub start: u64,
    pub end: u64,
}

/// The occupant of a destination, so a human can choose knowing what they
/// would displace.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct Conflict {
    pub existing: CatalogueEntry,
    /// The name [`OnConflict::KeepBoth`] would use, shown before
    /// choosing rather than after.
    pub keep_both_as: RootPath,
}

/// How far an upload has got, so resumption is a fact rather than a
/// guess.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct UploadProgress {
    pub upload_id: UploadId,
    pub received: u64,
    pub total: u64,
    /// Still outstanding. A client that lost its connection asks for
    /// this rather than restarting.
    pub needed: Vec<ChunkRange>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(u8)]
pub enum UploadEvent {
    Started(UploadId),
    Progressed(UploadProgress),
    Completed(CatalogueEntry),
    /// Abandoned or expired. Partial content is collected; no partial
    /// file was ever visible in the live tree.
    Aborted(UploadId),
}

#[architect::rpc]
pub trait UploadService {
    /// Ask what is needed. Returns a plan whose `needed` list is empty
    /// when the content is already held — dedup happens here, before any
    /// transfer, not after.
    async fn begin(&self, spec: UploadSpec) -> Result<UploadPlan, FilesFault>;

    /// What is still outstanding. The resumption path after a dropped
    /// connection.
    async fn progress(&self, upload_id: UploadId) -> Result<UploadProgress, FilesFault>;

    /// Land the upload, choosing how to resolve any collision.
    ///
    /// [`OnConflict::Replace`] records a new version rather than
    /// discarding the old. Fails with [`FilesFault::Exists`] under
    /// [`OnConflict::Fail`].
    async fn complete(
        &self,
        upload_id: UploadId,
        on_conflict: OnConflict,
    ) -> Result<CatalogueEntry, FilesFault>;

    /// Abandon it. Nothing partial was ever visible in the live tree.
    async fn abort(&self, upload_id: UploadId) -> Result<(), FilesFault>;

    /// Uploads still open for this principal, across devices — so a
    /// phone can finish what a laptop began.
    async fn pending(&self) -> Result<Vec<UploadProgress>, FilesFault>;
}
