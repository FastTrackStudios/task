//! File / image attachments. Stored off-message (under
//! `<state>/agent/attachments/<sha256>`); messages carry
//! `AttachmentRef` pointers.
//!
//! Matches Hermes's `pending_attachments` shape:
//! `{name, path, mime, size, is_image}`. Backends share
//! the same on-disk store so a single attachment can be
//! reused across messages without re-upload.

use chrono::{DateTime, Utc};
use facet::Facet;

/// Pointer to one stored attachment. Surface-side: messages
/// reference these by id; backends fetch bytes from the
/// attachment store on demand.
#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
pub struct AttachmentRef {
    /// Stable id (sha256-derived).
    pub id: String,
    /// Original filename (preserved for display).
    pub name: String,
    /// MIME type.
    pub mime: String,
    /// Size in bytes.
    pub size: u64,
    /// Whether the attachment is an image (`mime`
    /// starts with `image/`). Backends populate
    /// regardless of MIME guesswork.
    pub is_image: bool,
    /// SHA-256 hex of the bytes. Matches `id`.
    pub sha256: String,
    pub uploaded_at: DateTime<Utc>,
}

/// Full attachment metadata + bytes — returned from
/// `read_attachment`. The bytes are usually only fetched
/// when the consumer needs them (e.g. an image preview,
/// tool input).
#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
pub struct Attachment {
    pub r#ref: AttachmentRef,
    pub bytes: Vec<u8>,
}

/// Kind classification (UI helper).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Facet)]
#[repr(C)]
pub enum AttachmentKind {
    Image,
    Pdf,
    Markdown,
    Text,
    Audio,
    Video,
    Archive,
    Other,
}
