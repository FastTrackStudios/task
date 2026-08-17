//! `attachments-proto` — wire contract for the attachments feature.
//!
//! Per decentralized-foundation §13 Phase 7. Pattern:
//!
//! 1. Client calls [`AttachmentService::initiate_upload`] with
//!    filename + mime + size + the doc id the attachment belongs
//!    to.
//! 2. Server returns an [`UploadTicket`] — a signed HTTP PUT URL
//!    + an opaque `upload_id` it'll match against later.
//! 3. Client PUTs the bytes to the signed URL. The server hashes
//!    while reading, stores the bytes under `<sha256>`, and the
//!    HTTP response carries the computed `content_hash`.
//! 4. Client calls [`AttachmentService::complete_upload`] with
//!    `(upload_id, content_hash)` to confirm — the server returns
//!    an [`AttachmentMeta`] the client can persist into a Page's
//!    `kind: attachment` block frontmatter.
//! 5. Anyone who knows the content hash + has either no
//!    capability enforcement or an `AttachmentsOnly` /
//!    `doc-scoped` token can call
//!    [`AttachmentService::get_download_url`] to obtain a signed
//!    HTTP GET URL.
//!
//! The `kind: attachment` page convention (`content_hash` +
//! filename + `mime_type` in frontmatter) is documented in §13
//! Phase 7 but lives entirely in the Knowledge layer — proto
//! here doesn't enforce it.

use facet::Facet;
use thiserror::Error;
use uuid::Uuid;

/// Client → server. Request a signed PUT URL for a new attachment.
#[derive(Debug, Clone, Facet)]
pub struct InitiateUpload {
    /// Doc this attachment will be referenced from. Free-form to
    /// match `DocId` semantics in `project-proto`. The server
    /// records it on the upload session so capability checks have
    /// scope context later.
    pub doc_id: String,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: u64,
}

/// Server → client. The PUT target + the `upload_id` to send back
/// with `complete_upload`.
#[derive(Debug, Clone, Facet)]
pub struct UploadTicket {
    pub upload_id: Uuid,
    /// Absolute URL the client PUTs the bytes to.
    pub upload_url: String,
    /// `"PUT"` — kept on the wire so older clients that don't
    /// hardcode it stay forward-compatible.
    pub method: String,
    /// Unix seconds. After this point the signed URL won't verify.
    pub expires_unix: i64,
}

/// Client → server. Confirms the upload landed. The server has
/// already received + hashed the bytes; this call commits the
/// metadata into its in-memory map so subsequent
/// `get_download_url` calls succeed.
#[derive(Debug, Clone, Facet)]
pub struct CompleteUpload {
    pub upload_id: Uuid,
    /// SHA-256 hex of the bytes. Must match what the server
    /// computed during the PUT.
    pub content_hash: String,
}

/// Server → client. Persisted metadata for one blob. The client
/// writes a `kind: attachment` block carrying these fields in its
/// frontmatter.
#[derive(Debug, Clone, Facet)]
pub struct AttachmentMeta {
    pub content_hash: String,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: u64,
    pub doc_id: String,
}

/// Single-arg wrapper for `get_download_url`. Vox method args
/// take one typed value.
#[derive(Debug, Clone, Facet)]
pub struct ContentHashArg {
    pub content_hash: String,
}

/// Server → client. A short-lived signed HTTP GET URL.
#[derive(Debug, Clone, Facet)]
pub struct SignedDownloadUrl {
    pub url: String,
    pub expires_unix: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Facet, Error)]
#[repr(u8)]
pub enum AttachmentError {
    #[error("forbidden")]
    Forbidden,
    #[error("not found")]
    NotFound,
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("internal: {0}")]
    Internal(String),
}

#[cfg(feature = "vox")]
unsafe impl vox_types::Reborrow for InitiateUpload {
    type Ref<'a> = InitiateUpload;
}
#[cfg(feature = "vox")]
unsafe impl vox_types::Reborrow for UploadTicket {
    type Ref<'a> = UploadTicket;
}
#[cfg(feature = "vox")]
unsafe impl vox_types::Reborrow for CompleteUpload {
    type Ref<'a> = CompleteUpload;
}
#[cfg(feature = "vox")]
unsafe impl vox_types::Reborrow for AttachmentMeta {
    type Ref<'a> = AttachmentMeta;
}
#[cfg(feature = "vox")]
unsafe impl vox_types::Reborrow for ContentHashArg {
    type Ref<'a> = ContentHashArg;
}
#[cfg(feature = "vox")]
unsafe impl vox_types::Reborrow for SignedDownloadUrl {
    type Ref<'a> = SignedDownloadUrl;
}

#[cfg_attr(feature = "vox", architect::rpc)]
pub trait AttachmentService {
    async fn initiate_upload(&self, req: InitiateUpload) -> Result<UploadTicket, AttachmentError>;

    async fn complete_upload(&self, req: CompleteUpload)
    -> Result<AttachmentMeta, AttachmentError>;

    async fn get_download_url(
        &self,
        arg: ContentHashArg,
    ) -> Result<SignedDownloadUrl, AttachmentError>;
}

// The architect surface under the workspace's usual names — see
// `media-proto` for why `layer`/`serve` are emitted unqualified.
#[cfg(feature = "vox")]
pub use self::{
    AttachmentServiceRpcDispatcher as AttachmentDispatcher,
    attachment_service_rpc_service_descriptor as attachment_descriptor,
};
