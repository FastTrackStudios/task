//! The guest lane — a client with a link and no account.
//!
//! Separate from every other trait here because the principal is
//! different. Everywhere else the caller is a member of the org and the
//! backend carries their identity; here the caller is anonymous and the
//! link *is* the authorisation, scoped to one review and nothing else.
//!
//! What a client sees is deliverables, organised by scope and medium —
//! never the working tree that produced them. Anything marked internal is
//! unreachable and unlisted, not merely hidden.

use chrono::{DateTime, Utc};
use facet::Facet;
use serde::{Deserialize, Serialize};

use crate::error::FilesFault;
use crate::id::{CommentId, ReviewId, RootId, VersionId};
use crate::model::{AnnotationStroke, Review, ReviewComment};
use crate::path::RootPath;
use crate::service::media::{ByteTicket, Region};

/// A comment being placed, before it has an id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct NewComment {
    pub review: ReviewId,
    /// Which version was being watched. A comment belongs to the version
    /// it was made against, so a new one landing does not silently
    /// re-point it.
    pub version: VersionId,
    /// Where in the media. The same region scheme search hits use.
    pub region: Region,
    pub body: String,
    /// Optional drawing over the frame.
    pub strokes: Vec<AnnotationStroke>,
    /// A name the guest gave, since they have no account.
    pub author: String,
}

/// What a guest is allowed to do, resolved from the link.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct GuestScope {
    pub review: ReviewId,
    pub can_comment: bool,
    /// When false, the guest is served a proxy rendition only — never
    /// the source bytes.
    pub can_download: bool,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(u8)]
pub enum ReviewEvent {
    Created(Review),
    CommentAdded(ReviewComment),
    CommentDeleted(ReviewComment),
    /// A guest opened or downloaded something. What the sender learns.
    Accessed(ReviewId),
}

#[architect::rpc]
pub trait ReviewService {
    /// What this link permits. The guest's first call.
    async fn scope(&self) -> Result<GuestScope, FilesFault>;

    /// The review, with its versions.
    async fn review(&self, review: ReviewId) -> Result<Review, FilesFault>;

    /// A ticket for the media at a version. Serves a proxy rather than
    /// the source when download is withheld.
    async fn playback(
        &self,
        review: ReviewId,
        version: VersionId,
    ) -> Result<ByteTicket, FilesFault>;

    /// Comments, newest last.
    async fn comments(&self, review: ReviewId) -> Result<Vec<ReviewComment>, FilesFault>;

    /// Leave a comment against the version being watched.
    async fn comment(&self, comment: NewComment) -> Result<ReviewComment, FilesFault>;

    /// Remove one's own comment.
    async fn delete_comment(&self, comment: CommentId) -> Result<ReviewComment, FilesFault>;

    /// Find or create the review for a file — the member-side entry
    /// point, not the guest's.
    async fn for_file(&self, root_id: RootId, path: RootPath) -> Result<Review, FilesFault>;
}
