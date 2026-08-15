//! `AttachmentService` server-side implementation.
//!
//! Holds:
//!
//! - a clone of the server's `ServerKeypair` so it can mint signed
//!   HTTP URLs,
//! - an `Arc<dyn ObjectStore>` for the actual bytes,
//! - an in-memory `UploadSessionMap` keyed by `upload_id`. The map
//!   records what the client said they were uploading
//!   (filename / mime / `doc_id`) so `complete_upload` can stitch
//!   together the [`AttachmentMeta`] without re-asking.
//! - a "committed" map keyed by `content_hash` so
//!   `get_download_url` knows which blobs actually exist.
//!
//! The base HTTP URL is configurable so the same server boot can
//! work both on localhost (`http://127.0.0.1:9090`) and behind a
//! TLS reverse proxy (`https://example.com`). Default = an empty
//! string — the URL turns into a relative path. Phase 8 will
//! tighten this.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use attachments_proto::{
    AttachmentError, AttachmentMeta, AttachmentService, CompleteUpload, ContentHashArg,
    InitiateUpload, SignedDownloadUrl, UploadTicket,
};
use uuid::Uuid;

use crate::attachments::object_store::ObjectStore;
use crate::attachments::signed_url::BlobToken;
use crate::capability::ServerKeypair;

/// Lives on disk for `UPLOAD_TTL_SECONDS` after issuance. Long
/// enough for the client to flip out a `PUT` even on slow links;
/// short enough that abandoned tickets don't pile up.
pub const UPLOAD_TTL_SECONDS: i64 = 60 * 15; // 15 min
pub const DOWNLOAD_TTL_SECONDS: i64 = 60 * 60; // 1 hour

/// In-memory map of `upload_id -> pending metadata`. Cleared on
/// `complete_upload` for the matching id, naturally evicted by
/// expiry from anything that lingers. v1 keeps this in-process —
/// Phase 8 lifts it into the project doc when sessions need to
/// survive a restart.
#[derive(Debug, Default, Clone)]
pub struct UploadSessionMap {
    inner: Arc<Mutex<HashMap<Uuid, PendingUpload>>>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)] // reserved for the upcoming GC pass
pub(crate) struct PendingUpload {
    pub doc_id: String,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: u64,
    pub expires_unix: i64,
}

impl UploadSessionMap {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn insert(&self, id: Uuid, p: PendingUpload) {
        self.inner.lock().unwrap().insert(id, p);
    }

    fn take(&self, id: &Uuid) -> Option<PendingUpload> {
        self.inner.lock().unwrap().remove(id)
    }

    pub(crate) fn get_for_route(&self, id: &Uuid) -> Option<PendingUpload> {
        self.inner.lock().unwrap().get(id).cloned()
    }

    #[must_use]
    pub fn pending_count(&self) -> usize {
        self.inner.lock().unwrap().len()
    }
}

/// Committed attachment row: known to exist in the object store.
#[derive(Debug, Default, Clone)]
pub struct AttachmentCatalog {
    inner: Arc<Mutex<HashMap<String, AttachmentMeta>>>,
}

impl AttachmentCatalog {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, meta: AttachmentMeta) {
        self.inner
            .lock()
            .unwrap()
            .insert(meta.content_hash.clone(), meta);
    }

    #[must_use]
    pub fn get(&self, content_hash: &str) -> Option<AttachmentMeta> {
        self.inner.lock().unwrap().get(content_hash).cloned()
    }
}

#[derive(Clone)]
pub struct AttachmentServiceImpl {
    pub keypair: ServerKeypair,
    pub store: Arc<dyn ObjectStore>,
    pub sessions: UploadSessionMap,
    pub catalog: AttachmentCatalog,
    /// Base URL the signed URLs are prefixed with. May be empty —
    /// the resulting URL is then a relative path the client
    /// resolves against the WS endpoint's origin.
    pub public_base_url: String,
}

impl AttachmentServiceImpl {
    pub fn new(
        keypair: ServerKeypair,
        store: Arc<dyn ObjectStore>,
        public_base_url: impl Into<String>,
    ) -> Self {
        Self {
            keypair,
            store,
            sessions: UploadSessionMap::new(),
            catalog: AttachmentCatalog::new(),
            public_base_url: public_base_url.into(),
        }
    }

    fn build_upload_url(&self, upload_id: Uuid, token: &str) -> String {
        format!(
            "{base}/blobs/upload?upload_id={upload_id}&token={token}",
            base = self.public_base_url,
        )
    }

    fn build_download_url(&self, content_hash: &str, token: &str) -> String {
        format!(
            "{base}/blobs/download/{content_hash}?token={token}",
            base = self.public_base_url,
        )
    }

    /// Crate-private side door so the `PUT /blobs/upload` route
    /// can look up + consume the pending session it was issued
    /// for.
    pub(crate) fn take_pending(&self, upload_id: &Uuid) -> Option<PendingUpload> {
        self.sessions.take(upload_id)
    }

    pub(crate) fn peek_pending(&self, upload_id: &Uuid) -> Option<PendingUpload> {
        self.sessions.get_for_route(upload_id)
    }

    pub(crate) fn record_commit(&self, meta: AttachmentMeta) {
        self.catalog.insert(meta);
    }
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl AttachmentService for AttachmentServiceImpl {
    async fn initiate_upload(&self, req: InitiateUpload) -> Result<UploadTicket, AttachmentError> {
        if req.filename.is_empty() {
            return Err(AttachmentError::InvalidInput("filename empty".into()));
        }
        if req.doc_id.is_empty() {
            return Err(AttachmentError::InvalidInput("doc_id empty".into()));
        }
        let upload_id = Uuid::new_v4();
        let expires_unix = now_unix() + UPLOAD_TTL_SECONDS;
        let token = BlobToken::upload(upload_id, expires_unix).issue(&self.keypair);
        self.sessions.insert(
            upload_id,
            PendingUpload {
                doc_id: req.doc_id,
                filename: req.filename,
                mime_type: req.mime_type,
                size_bytes: req.size_bytes,
                expires_unix,
            },
        );
        Ok(UploadTicket {
            upload_id,
            upload_url: self.build_upload_url(upload_id, &token),
            method: "PUT".into(),
            expires_unix,
        })
    }

    async fn complete_upload(
        &self,
        req: CompleteUpload,
    ) -> Result<AttachmentMeta, AttachmentError> {
        // The PUT handler removes the pending session AND writes
        // the catalog row before returning to the client, so by
        // the time the client calls `complete_upload` the catalog
        // should already have the entry. Look it up; if missing,
        // the client called complete before the PUT — Forbidden
        // covers that and "upload_id no longer pending" cleanly.
        if let Some(meta) = self.catalog.get(&req.content_hash) {
            return Ok(meta);
        }
        // Fallback: caller may invoke complete_upload before the
        // catalog write has flushed. Best-effort verification —
        // bytes exist in the store? Re-emit metadata from the
        // (still-pending or just-taken) session.
        if !self.store.blob_exists(&req.content_hash).await? {
            return Err(AttachmentError::NotFound);
        }
        // We've lost the pending session — return a minimal meta
        // row keyed by the hash. Phase 8 will track sessions in
        // the project doc so this branch shouldn't fire.
        Err(AttachmentError::Internal(
            "catalog row missing — PUT may have failed".into(),
        ))
    }

    async fn get_download_url(
        &self,
        arg: ContentHashArg,
    ) -> Result<SignedDownloadUrl, AttachmentError> {
        if !self.store.blob_exists(&arg.content_hash).await? {
            return Err(AttachmentError::NotFound);
        }
        let expires_unix = now_unix() + DOWNLOAD_TTL_SECONDS;
        let token =
            BlobToken::download(arg.content_hash.clone(), expires_unix).issue(&self.keypair);
        let url = self.build_download_url(&arg.content_hash, &token);
        Ok(SignedDownloadUrl { url, expires_unix })
    }
}
