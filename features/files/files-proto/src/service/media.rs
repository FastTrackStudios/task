//! Bytes, renditions and handoff — `files.scale.large-media`,
//! `files.handoff.editor`.
//!
//! Nothing on this surface returns file content inline. A read yields a
//! [`ByteTicket`] redeemed on the byte lane, which streams and honours
//! ranges — so seeking the middle of a multi-gigabyte video fetches the
//! chunks under the playhead and never the file.

use chrono::{DateTime, Utc};
use facet::Facet;
use serde::{Deserialize, Serialize};

use crate::error::FilesFault;
use crate::id::{ContentId, RootId, VersionId};
use crate::model::{RenditionInfo, RenditionKind};
use crate::path::RootPath;

/// Permission to move bytes, redeemed on the byte lane.
///
/// Short-lived and single-purpose. Bytes never ride the RPC surface: a
/// `Vec<u8>` return type is how a 244 GB project becomes an out-of-memory
/// error, and a ticket keeps range requests, resumption and peer-to-peer
/// transfer available without any of them touching this trait.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct ByteTicket {
    pub token: String,
    /// Total length when known. `None` for a stream generated as it is
    /// sent, such as an archive.
    pub length: Option<u64>,
    /// Whether the lane will honour a range request for this ticket.
    pub seekable: bool,
    pub content_type: String,
    pub expires_at: DateTime<Utc>,
}

/// A region of a file: a time range, a page, or a rectangle.
///
/// One scheme, three consumers — search hits, review annotations and
/// resource annotations. `files.index.regions` requires exactly this: a
/// second addressing scheme for the same idea is the failure mode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(u8)]
pub enum Region {
    /// Seconds from the start, `[start, end)`.
    Time { start_ms: u64, end_ms: u64 },
    /// A page, 1-indexed.
    Page { page: u32 },
    /// A rectangle in normalised 0..1 coordinates, optionally at a time.
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        at_ms: Option<u64>,
    },
    /// A byte range, for formats with no better anchor.
    Bytes { start: u64, end: u64 },
    /// The whole file.
    Whole,
}

/// A byte range, inclusive at both ends, relative to a ticket.
///
/// Inclusive because HTTP's `Range` is, and the browser adapter's whole
/// job is to answer `Range` without translating a convention twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct ByteRange {
    pub first: u64,
    pub last: u64,
}

/// What to read, and how much of it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct ByteRequest {
    /// A token from [`ByteTicket`].
    pub token: String,
    /// `None` reads the whole ticket.
    pub range: Option<ByteRange>,
}

/// One frame of a byte stream.
///
/// A frame rather than a bare `Vec<u8>` because a stream has to be able
/// to say three things a raw byte sequence cannot: how long it will be,
/// that it finished, and that it did not.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(u8)]
pub enum ByteFrame {
    /// Always first. Carries what an HTTP adapter needs for its headers
    /// before any body arrives.
    Opened {
        /// Length of what will follow, honouring the request's range.
        length: u64,
        /// Length of the whole object, for a `Content-Range`.
        total: u64,
        content_type: String,
    },
    /// Bytes at `offset`, relative to the start of the request.
    Chunk { offset: u64, bytes: Vec<u8> },
    /// Everything asked for was sent.
    Done,
    /// It stopped part way.
    ///
    /// This is why bytes ride vox rather than the fallback HTTP route:
    /// there, the status is already on the wire when the read begins, so
    /// a mid-stream failure can only truncate the body and the client
    /// sees a short read it must infer from `Content-Length`. Here the
    /// stream says so.
    Failed(crate::error::FilesFault),
}

/// Where a handoff should land.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Facet)]
#[repr(u8)]
pub enum HandoffTarget {
    /// A bin of clips.
    Bin,
    /// A timeline in the order given.
    Timeline,
}

/// One item in a handoff: content, and which part of it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct HandoffItem {
    pub root_id: RootId,
    pub path: RootPath,
    /// Region bounds survive the trip: a hit covering 0:40–0:52 arrives
    /// as that range, not as the whole clip.
    pub region: Region,
}

/// A handoff an editor can collect. Content stays in place; nothing is
/// copied.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct Handoff {
    pub name: String,
    pub target: HandoffTarget,
    pub items: Vec<HandoffItem>,
    /// Redeemed by the editor-side integration.
    pub token: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(u8)]
pub enum MediaEvent {
    /// A rendition finished and is now servable.
    RenditionReady(RenditionInfo),
    RenditionFailed(RenditionInfo),
    /// A handoff was collected by its editor.
    HandoffCollected(String),
}

#[architect::rpc]
pub trait MediaService {
    /// A ticket for a file's current content.
    async fn read(&self, root_id: RootId, path: RootPath) -> Result<ByteTicket, FilesFault>;

    /// A ticket for a file's content at a past version.
    async fn read_at(
        &self,
        root_id: RootId,
        path: RootPath,
        version: VersionId,
    ) -> Result<ByteTicket, FilesFault>;

    /// A ticket for content by address, with no path context. The
    /// federation read path: bytes already held locally resolve without
    /// reaching their origin server at all.
    async fn read_content(&self, content: ContentId) -> Result<ByteTicket, FilesFault>;

    /// Renditions available for a file, and their state.
    async fn renditions(
        &self,
        root_id: RootId,
        path: RootPath,
    ) -> Result<Vec<RenditionInfo>, FilesFault>;

    /// A ticket for one rendition, requesting it if absent.
    async fn rendition(
        &self,
        root_id: RootId,
        path: RootPath,
        kind: RenditionKind,
    ) -> Result<ByteTicket, FilesFault>;

    /// Hand a selection to an editor as a bin or a timeline.
    async fn handoff(
        &self,
        name: String,
        target: HandoffTarget,
        items: Vec<HandoffItem>,
    ) -> Result<Handoff, FilesFault>;

    /// **The byte lane.** Redeem a [`ByteTicket`] as a stream of frames.
    ///
    /// Bytes ride vox like everything else, which is the design's whole
    /// transport claim: a native client — CLI, desktop, iOS, TS — needs
    /// no HTTP at all, and peer-to-peer rides the same iroh/QUIC the
    /// chunk store already uses. The browser is the one exception, and
    /// it is a transport *adapter* rather than an API: a service worker
    /// intercepts `fetch`, answers `Range` itself, and feeds a
    /// `ReadableStream` from this.
    ///
    /// Flow control is vox's, and it is the reason this is safe on a
    /// 244 GB file: the channel carries a fixed credit, and the sink's
    /// `send` awaits when the mailbox is full, so a client that stops
    /// reading stops the server producing. A stream that pushed without
    /// waiting would move the memory blow-up from the client to us.
    #[subscribe]
    fn bytes(&self, request: ByteRequest) -> ByteFrame;
}
