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

use architect_permissions::{Action, PermissionEngine, Principal, Resource};
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
/// The default AND the ceiling for a download grant's life. The env
/// override (`TASK_BLOB_DOWNLOAD_TTL_SECONDS`) can only shorten it.
pub const DOWNLOAD_TTL_SECONDS: i64 = 60 * 60; // 1 hour

/// Env knob for the download grant's life, in seconds. Clamped to
/// `1..=DOWNLOAD_TTL_SECONDS`; unset or unparsable means the default.
pub const DOWNLOAD_TTL_ENV: &str = "TASK_BLOB_DOWNLOAD_TTL_SECONDS";

/// The download TTL from [`DOWNLOAD_TTL_ENV`], clamped so configuration
/// can shorten a grant but never lengthen it past the default.
#[must_use]
pub fn download_ttl_from_env() -> i64 {
    clamp_download_ttl(
        std::env::var(DOWNLOAD_TTL_ENV)
            .ok()
            .and_then(|v| v.trim().parse::<i64>().ok()),
    )
}

fn clamp_download_ttl(requested: Option<i64>) -> i64 {
    requested.map_or(DOWNLOAD_TTL_SECONDS, |s| s.clamp(1, DOWNLOAD_TTL_SECONDS))
}

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
    /// The org this service belongs to. Every token it mints is bound
    /// to it, and the blob routes serve a token from this org's store.
    pub slug: String,
    /// The org lane's permission engine — the same one the gate asks
    /// ([`crate::org_permission_engine`]). `get_download_url` asks it
    /// directly so the decision holds even when the gate is
    /// observe-only.
    pub engine: Arc<dyn PermissionEngine>,
    /// Life of a download grant, `1..=DOWNLOAD_TTL_SECONDS`.
    pub download_ttl_seconds: i64,
}

impl AttachmentServiceImpl {
    pub fn new(
        keypair: ServerKeypair,
        store: Arc<dyn ObjectStore>,
        public_base_url: impl Into<String>,
        slug: impl Into<String>,
    ) -> Self {
        Self {
            keypair,
            store,
            sessions: UploadSessionMap::new(),
            catalog: AttachmentCatalog::new(),
            public_base_url: public_base_url.into(),
            slug: slug.into(),
            engine: crate::org_permission_engine(),
            download_ttl_seconds: download_ttl_from_env(),
        }
    }

    /// Override the download TTL. Clamped like the env knob: it can
    /// shorten a grant, never lengthen it.
    #[must_use]
    pub fn with_download_ttl(mut self, seconds: i64) -> Self {
        self.download_ttl_seconds = clamp_download_ttl(Some(seconds));
        self
    }

    fn build_upload_url(&self, upload_id: Uuid, token: &str) -> String {
        format!(
            "{base}/org/{slug}/blobs/upload?upload_id={upload_id}&token={token}",
            base = self.public_base_url,
            slug = self.slug,
        )
    }

    fn build_download_url(&self, content_hash: &str, token: &str) -> String {
        format!(
            "{base}/org/{slug}/blobs/download/{content_hash}?token={token}",
            base = self.public_base_url,
            slug = self.slug,
        )
    }

    /// May the caller of the RPC running on this task download
    /// `content_hash` from this org? Decided HERE, not only by the gate,
    /// because the gate is observe-only unless
    /// `TASK_ENFORCE_PERMISSIONS=1` and would let an anonymous caller
    /// through to mint. The gate has already recorded its would-deny;
    /// this refuses regardless of mode (fail closed).
    ///
    /// The resource is the object that owns the blob: the attaching
    /// document when this org's catalog knows it, else the blob itself.
    /// Returns the principal the grant is bound to.
    fn authorize_download(&self, content_hash: &str) -> Result<Principal, AttachmentError> {
        use architect_telemetry::wide;

        // No gate scope means nobody resolved an identity for this call.
        // That is "no identity", never a default.
        let Some(who) = architect::permissions_gate::caller() else {
            wide::set("attachments.download_auth", "no-caller");
            tracing::warn!(
                org.slug = self.slug,
                "attachments: get_download_url with no resolved caller, refusing"
            );
            return Err(AttachmentError::Forbidden);
        };
        if matches!(who, Principal::Anonymous) {
            wide::set("attachments.download_auth", "anonymous");
            tracing::warn!(
                org.slug = self.slug,
                "attachments: anonymous get_download_url refused"
            );
            return Err(AttachmentError::Forbidden);
        }
        let owner = self
            .catalog
            .get(content_hash)
            .map_or_else(|| content_hash.to_owned(), |m| m.doc_id);
        let resource = Resource::new(format!("attachments/{owner}"));
        let decision = self
            .engine
            .check(&who, &resource, &Action::new(Action::DOWNLOAD));
        if !decision.allowed() {
            wide::set("attachments.download_auth", "denied");
            tracing::warn!(
                org.slug = self.slug,
                principal = who.describe(),
                resource = resource.as_str(),
                "attachments: get_download_url denied"
            );
            return Err(AttachmentError::Forbidden);
        }
        wide::set("attachments.download_auth", "allowed");
        Ok(who)
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
        let token =
            BlobToken::upload(self.slug.clone(), upload_id, expires_unix).issue(&self.keypair);
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
        // Authorize BEFORE touching the store, so a refused caller cannot
        // use NotFound-vs-Forbidden to probe which hashes exist.
        let who = self.authorize_download(&arg.content_hash)?;
        if !self.store.blob_exists(&arg.content_hash).await? {
            return Err(AttachmentError::NotFound);
        }
        let expires_unix = now_unix() + self.download_ttl_seconds;
        let token = BlobToken::download(
            self.slug.clone(),
            who.describe(),
            arg.content_hash.clone(),
            expires_unix,
        )
        .issue(&self.keypair);
        let url = self.build_download_url(&arg.content_hash, &token);
        Ok(SignedDownloadUrl { url, expires_unix })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attachments::LocalFsStore;

    fn service(tmp: &tempfile::TempDir) -> AttachmentServiceImpl {
        AttachmentServiceImpl::new(
            ServerKeypair::generate_ephemeral(),
            Arc::new(LocalFsStore::new(tmp.path())),
            String::new(),
            "acme",
        )
    }

    #[test]
    fn download_ttl_can_shorten_but_never_lengthen() {
        assert_eq!(clamp_download_ttl(None), DOWNLOAD_TTL_SECONDS);
        assert_eq!(clamp_download_ttl(Some(60)), 60);
        assert_eq!(
            clamp_download_ttl(Some(DOWNLOAD_TTL_SECONDS * 24)),
            DOWNLOAD_TTL_SECONDS
        );
        assert_eq!(clamp_download_ttl(Some(0)), 1);
        assert_eq!(clamp_download_ttl(Some(-5)), 1);
    }

    #[tokio::test]
    async fn no_gate_caller_means_no_grant() {
        // Off the gate's request task nobody resolved an identity. That
        // must read as "no identity", never as a permissive default.
        let tmp = tempfile::tempdir().unwrap();
        let svc = service(&tmp);
        svc.store.put_blob("abc", b"bytes").await.unwrap();
        let err = svc
            .get_download_url(ContentHashArg {
                content_hash: "abc".into(),
            })
            .await
            .expect_err("no caller, no grant");
        assert_eq!(err, AttachmentError::Forbidden);
    }

    #[tokio::test]
    async fn upload_ticket_is_bound_to_the_org_and_named_under_it() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = service(&tmp);
        let ticket = svc
            .initiate_upload(InitiateUpload {
                doc_id: "Songs/A.md".into(),
                filename: "a.ogg".into(),
                mime_type: "audio/ogg".into(),
                size_bytes: 3,
            })
            .await
            .unwrap();
        assert!(
            ticket.upload_url.starts_with("/org/acme/blobs/upload?"),
            "{}",
            ticket.upload_url
        );
        let token = ticket.upload_url.split("token=").nth(1).unwrap();
        let tok = BlobToken::verify(token, &svc.keypair, now_unix()).unwrap();
        assert!(tok.allows_upload("acme", ticket.upload_id));
        assert!(!tok.allows_upload("other", ticket.upload_id));
    }
}
