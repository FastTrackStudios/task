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

use crate::error::FilesFault;
use crate::id::{ContentId, RootId, UploadId};
use crate::path::RootPath;
use crate::service::tree::CatalogueEntry;
use crate::service::write::OnConflict;

/// What the client wants to put where.
#[derive(Debug, Clone, PartialEq, Facet)]
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
#[derive(Debug, Clone, PartialEq, Facet)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Facet)]
#[repr(C)]
pub struct ChunkRange {
    pub start: u64,
    pub end: u64,
}

/// The occupant of a destination, so a human can choose knowing what they
/// would displace.
#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(C)]
pub struct Conflict {
    pub existing: CatalogueEntry,
    /// The name [`OnConflict::KeepBoth`] would use, shown before
    /// choosing rather than after.
    pub keep_both_as: RootPath,
}

/// How far an upload has got, so resumption is a fact rather than a
/// guess.
#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(C)]
pub struct UploadProgress {
    pub upload_id: UploadId,
    pub received: u64,
    pub total: u64,
    /// Still outstanding. A client that lost its connection asks for
    /// this rather than restarting.
    pub needed: Vec<ChunkRange>,
}

/// One frame of an upload, travelling client to server.
///
/// The mirror of [`ByteFrame`](crate::service::media::ByteFrame), and the
/// direction that lane cannot serve: a subscription streams *out*.
/// Ingress rides the same transport by handing the server the receiving
/// end of a channel, which vox binds like any other argument — "in method
/// args, the handler holds it (handler receives ← caller)".
#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(u8)]
pub enum UploadFrame {
    /// Bytes at `offset` from the start of the file.
    ///
    /// Carrying the offset rather than assuming order is what makes a
    /// resumed upload cheap: a client that reconnects sends only the
    /// ranges `progress()` still reports outstanding, in whatever order
    /// it likes.
    Chunk { offset: u64, bytes: Vec<u8> },
    /// No more frames are coming. The session stays open — landing is
    /// still `complete`, because only the caller can answer a collision.
    Finished,
}

/// What the server made of a run of frames.
#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(C)]
pub struct Received {
    pub upload_id: UploadId,
    /// Bytes written by this run.
    pub written: u64,
    /// What is still outstanding afterwards. Empty means `complete` will
    /// not refuse for want of bytes.
    pub needed: Vec<ChunkRange>,
}

#[derive(Debug, Clone, PartialEq, Facet)]
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

    /// **The ingress byte lane.** Send an upload's bytes.
    ///
    /// The client makes a channel, keeps the sender, and passes the
    /// receiver here; vox binds it exactly as it binds the sink a
    /// subscription streams into, so bytes arrive over the same
    /// transport as every other call and no second protocol exists.
    ///
    /// Flow control runs the same way and matters more in this
    /// direction: the channel carries a fixed credit, so a client that
    /// sends faster than the server can write is made to wait rather
    /// than being allowed to buffer a 244 GB file into the server's
    /// memory.
    ///
    /// Landing is still [`UploadService::complete`]. This method moves
    /// bytes and decides nothing — a collision is the caller's to
    /// answer, and answering it here would make the sending of bytes
    /// the moment a file is displaced.
    async fn send_bytes(
        &self,
        upload_id: UploadId,
        frames: architect::vox::Rx<UploadFrame>,
    ) -> Result<Received, FilesFault>;
}
