//! Phase 7 — attachments (blob storage + signed URL HTTP routes).
//!
//! Layout:
//!
//! - [`object_store`] — `ObjectStore` trait + `LocalFsStore` impl.
//!   New backends (Nextcloud, S3) implement the same trait; the
//!   service is generic over it.
//! - [`signed_url`] — `BlobToken` issuance + verification via the
//!   server's existing `ServerKeypair` (Phase 3). Two purposes:
//!   `Upload` (carries `upload_id`) and `Download` (carries
//!   `content_hash`).
//! - [`service`] — `AttachmentServiceImpl` — implements the
//!   `attachments_proto::AttachmentService` vox trait. Holds an
//!   `UploadSessionMap` keyed by `upload_id` until
//!   `complete_upload` commits the row.
//! - [`routes`] — the two axum handlers: `PUT /blobs/upload` and
//!   `GET /blobs/download/{hash}`. Both verify a `?token=…` in
//!   the query string before doing real work.

pub mod object_store;
pub mod routes;
pub mod service;
pub mod signed_url;

pub use object_store::{LocalFsStore, ObjectStore, default_blob_root};
pub use routes::attachment_router;
pub use service::{AttachmentServiceImpl, UploadSessionMap};
pub use signed_url::{BlobToken, BlobTokenPurpose};

/// The blob routes reach each hosted org through the live org map, so
/// a token is always served by the org it was minted in, including an
/// org created after the router was built.
impl routes::BlobOrgs for crate::AppState {
    fn attachments(&self, slug: &str) -> Option<std::sync::Arc<AttachmentServiceImpl>> {
        self.org(slug)
            .map(|org| std::sync::Arc::clone(&org.attachments))
    }

    fn identity(
        &self,
        slug: &str,
    ) -> Option<std::sync::Arc<dyn architect_permissions::IdentityResolver>> {
        self.org(slug)
            .map(|org| org.permissions.identity_resolver())
    }
}

/// Sniff an audio container mime from a blob's magic bytes — the catalog
/// is in-memory only, so after a server restart uploaded blobs have no
/// recorded mime; the media paths (vox `stat`, HTTP download) fall back to
/// this so players get a usable content type instead of octet-stream.
pub fn sniff_mime(head: &[u8]) -> &'static str {
    if head.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        // EBML — webm/mkv; our ingests are audio.
        "audio/webm"
    } else if head.starts_with(b"OggS") {
        "audio/ogg"
    } else if head.starts_with(b"fLaC") {
        "audio/flac"
    } else if head.starts_with(b"RIFF") {
        "audio/wav"
    } else if head.starts_with(b"ID3") || head.starts_with(&[0xFF, 0xFB]) {
        "audio/mpeg"
    } else if head.len() > 11 && &head[4..12] == b"ftypM4A " {
        "audio/mp4"
    } else if head.starts_with(b"%PDF") {
        "application/pdf"
    } else {
        "application/octet-stream"
    }
}
