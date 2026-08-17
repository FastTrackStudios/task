//! `media-proto` — first-class media streaming over vox.
//!
//! The attachments flow (Phase 7) moves blob *bytes* out-of-band over
//! HTTP against signed URLs; that leaves large-media consumers (the
//! session player's stems, part bundles, sample previews) hand-rolling
//! HTTP Range plumbing next to the vox surface. [`AttachmentMediaService`] is the
//! architect-native alternative: content-addressed reads streamed as
//! [`MediaChunk`]s through a `Tx` lane, so browser, native, and CLI
//! clients all reach media through the same per-org vox router — same
//! origin, same auth path, no second protocol.
//!
//! Ranged reads are explicit (`start`/`len`) so a seeking player pulls
//! exactly the window it needs; `len = u64::MAX` means "to the end".
//! Chunk size is a server concern — consumers must tolerate any
//! chunking and reassemble by `offset`.

use facet::Facet;
use thiserror::Error;

/// Metadata for one content-addressed media blob.
#[derive(Debug, Clone, Facet)]
pub struct MediaInfo {
    /// SHA-256 hex — the blob's identity (same namespace as
    /// attachments; a stem uploaded as an attachment is readable
    /// here by the same hash).
    pub content_hash: String,
    pub size_bytes: u64,
    /// Best-effort mime type from the attachment catalog;
    /// `application/octet-stream` when unknown.
    pub mime_type: String,
    /// Original filename, empty when unknown.
    pub filename: String,
}

/// One streamed slice of a blob. Chunks arrive in order with
/// contiguous, monotonically increasing `offset`s; the stream closing
/// (without error) marks the end of the requested window.
#[derive(Debug, Clone, Facet)]
pub struct MediaChunk {
    /// Absolute byte offset of this chunk within the blob.
    pub offset: u64,
    pub bytes: Vec<u8>,
}

/// A short-lived signed grant for the filesystem media route
/// (`GET /org/{slug}/media/{path}?token=…`).
///
/// Why this exists alongside the content-addressed reads above: colocated
/// song media is served straight off disk by path, and the consumers are
/// browser `<audio>`/`<img>` elements, which cannot set an
/// `Authorization` header and cannot await an RPC inside `set_src`. So
/// the caller mints one grant over vox — where the permission gate can
/// see it — and appends the opaque token to the URLs it builds.
///
/// The grant covers a path PREFIX, because a song loads as a directory
/// (manifest + chart + N stems); one grant per song load rather than one
/// RPC per file.
#[derive(Debug, Clone, Facet)]
pub struct MediaGrant {
    /// Opaque signed token — append as `?token=`.
    pub token: String,
    /// Unix seconds after which the token stops verifying.
    pub expires_unix: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Facet, Error)]
#[repr(u8)]
pub enum MediaError {
    #[error("not found")]
    NotFound,
    #[error("invalid range: {0}")]
    InvalidRange(String),
    #[error("internal: {0}")]
    Internal(String),
}

#[cfg(feature = "vox")]
unsafe impl vox_types::Reborrow for MediaInfo {
    type Ref<'a> = MediaInfo;
}
#[cfg(feature = "vox")]
unsafe impl vox_types::Reborrow for MediaChunk {
    type Ref<'a> = MediaChunk;
}
#[cfg(feature = "vox")]
unsafe impl vox_types::Reborrow for MediaError {
    type Ref<'a> = MediaError;
}
#[cfg(feature = "vox")]
unsafe impl vox_types::Reborrow for MediaGrant {
    type Ref<'a> = MediaGrant;
}

/// Content-addressed media reads over vox. Mounted per org next to
/// `AttachmentService`; hashes are the same blob-store namespace.
#[cfg(feature = "vox")]
#[architect::rpc]
pub trait AttachmentMediaService {
    /// Size + mime for a blob — what a player needs before it
    /// requests windows.
    async fn stat(&self, content_hash: String) -> Result<MediaInfo, MediaError>;

    /// Stream `len` bytes starting at `start` (clamped to the blob's
    /// end; `len = u64::MAX` reads to the end) as ordered
    /// [`MediaChunk`]s on `tx`. Returns after the last chunk is
    /// queued; the closed stream is the end-of-window signal.
    async fn read(
        &self,
        content_hash: String,
        start: u64,
        len: u64,
        tx: architect::vox::Tx<MediaChunk>,
    ) -> Result<(), MediaError>;

    /// Mint a [`MediaGrant`] covering `prefix` under this org's
    /// filesystem media route.
    ///
    /// This call is the authorization point: it rides the per-org vox
    /// lane, so the permission gate decides whether the caller may have
    /// a grant at all. The HTTP route then only has to verify the
    /// signature — no session lookup on a hot media path, and no way for
    /// an unauthenticated caller to obtain a token in the first place.
    async fn media_grant(&self, prefix: String) -> Result<MediaGrant, MediaError>;
}

// The architect surface, under the names the rest of the workspace
// uses. `#[architect::rpc]` emits `<Trait>RpcDispatcher` and
// `<trait>_rpc_service_descriptor`; every other proto crate re-exports
// those under a shorter name, and mounting code reads better for it.
//
// `layer` and `serve` are emitted unqualified, which is why a module
// annotated with `#[architect::rpc]` holds exactly one trait.
#[cfg(feature = "vox")]
pub use self::{
    AttachmentMediaServiceRpcDispatcher as AttachmentMediaDispatcher,
    attachment_media_service_rpc_service_descriptor as attachment_media_descriptor,
};
