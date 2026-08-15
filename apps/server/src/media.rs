//! `MediaService` impl — content-addressed media streamed over vox.
//!
//! Thin layer over the attachments blob store: same hashes, same
//! catalog, but the bytes travel through the vox lane as
//! [`MediaChunk`]s instead of out-of-band HTTP. Mounted per org next
//! to `AttachmentService` (see `org_layer_router`), so a song stem
//! uploaded as an attachment is immediately streamable here.

use std::sync::Arc;

use media_proto::{MediaChunk, MediaError, MediaGrant, MediaInfo, MediaService};

use crate::attachments::AttachmentServiceImpl;

/// Per-read chunk size. Big enough to amortize framing, small enough
/// that a seeking player gets its first audio fast.
const CHUNK_BYTES: u64 = 256 * 1024;

/// How long a minted [`MediaGrant`] stays valid.
///
/// Long enough to load and play a set without re-minting mid-song, short
/// enough that a token leaked through a browser history, a shared link,
/// or a proxy log stops working the same day.
const GRANT_TTL_SECONDS: i64 = 6 * 60 * 60;

#[derive(Clone)]
pub struct MediaServiceImpl {
    /// Blob store + catalog live on the attachment service; media is
    /// a read-side view over the same namespace.
    attachments: Arc<AttachmentServiceImpl>,
    /// This org's slug — baked into every grant's subject so a token
    /// minted here can never read another org's files.
    slug: String,
    /// Signs grants; the HTTP media route verifies against the same key.
    keypair: crate::capability::ServerKeypair,
}

impl MediaServiceImpl {
    pub fn new(
        attachments: Arc<AttachmentServiceImpl>,
        slug: impl Into<String>,
        keypair: crate::capability::ServerKeypair,
    ) -> Self {
        Self {
            attachments,
            slug: slug.into(),
            keypair,
        }
    }
}

fn store_err(e: attachments_proto::AttachmentError) -> MediaError {
    match e {
        attachments_proto::AttachmentError::NotFound => MediaError::NotFound,
        other => MediaError::Internal(other.to_string()),
    }
}

impl MediaService for MediaServiceImpl {
    async fn stat(&self, content_hash: String) -> Result<MediaInfo, MediaError> {
        let size_bytes = self
            .attachments
            .store
            .blob_len(&content_hash)
            .await
            .map_err(store_err)?;
        let meta = self.attachments.catalog.get(&content_hash);
        let mime_type = match meta.as_ref() {
            Some(m) => m.mime_type.clone(),
            // Catalog is in-memory only — sniff restarts' orphaned blobs.
            None => {
                let head = self
                    .attachments
                    .store
                    .get_blob(&content_hash)
                    .await
                    .map_err(store_err)?;
                crate::attachments::sniff_mime(head.get(..16).unwrap_or(&head)).to_string()
            }
        };
        Ok(MediaInfo {
            content_hash,
            size_bytes,
            mime_type,
            filename: meta.map(|m| m.filename).unwrap_or_default(),
        })
    }

    async fn read(
        &self,
        content_hash: String,
        start: u64,
        len: u64,
        tx: vox::Tx<MediaChunk>,
    ) -> Result<(), MediaError> {
        let total = self
            .attachments
            .store
            .blob_len(&content_hash)
            .await
            .map_err(store_err)?;
        if start > total {
            return Err(MediaError::InvalidRange(format!(
                "start {start} past end {total}"
            )));
        }
        let end = start.saturating_add(len).min(total);
        let mut offset = start;
        while offset < end {
            let take = CHUNK_BYTES.min(end - offset);
            let bytes = self
                .attachments
                .store
                .get_blob_range(&content_hash, offset, take)
                .await
                .map_err(store_err)?;
            if bytes.is_empty() {
                break; // store shrank underneath us — end the window
            }
            let sent = bytes.len() as u64;
            if tx.send(MediaChunk { offset, bytes }).await.is_err() {
                // Receiver hung up (seek elsewhere, page closed) —
                // not an error, just stop reading.
                return Ok(());
            }
            offset += sent;
        }
        Ok(())
    }

    async fn media_grant(&self, prefix: String) -> Result<MediaGrant, MediaError> {
        // Traversal is rejected at MINT time as well as at serve time.
        // Belt and braces on purpose: this is the only place a `..` could
        // be baked into a *signed* subject, where it would then be
        // presented as legitimately granted.
        let prefix = prefix.trim_matches('/');
        if prefix.is_empty() || prefix.split('/').any(|s| s == ".." || s.is_empty()) {
            return Err(MediaError::InvalidRange(format!(
                "invalid media prefix `{prefix}`"
            )));
        }
        let expires_unix = chrono::Utc::now().timestamp() + GRANT_TTL_SECONDS;
        let token =
            crate::attachments::signed_url::BlobToken::media(&self.slug, prefix, expires_unix)
                .issue(&self.keypair);
        Ok(MediaGrant {
            token,
            expires_unix,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attachments::{LocalFsStore, ObjectStore};

    async fn service_with_blob(bytes: &[u8], hash: &str) -> (MediaServiceImpl, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store = LocalFsStore::new(tmp.path());
        store.put_blob(hash, bytes).await.expect("put");
        let attachments = Arc::new(AttachmentServiceImpl::new(
            crate::capability::ServerKeypair::generate_ephemeral(),
            Arc::new(store),
            String::new(),
        ));
        let keypair = attachments.keypair.clone();
        (MediaServiceImpl::new(attachments, "test-org", keypair), tmp)
    }

    #[tokio::test]
    async fn stat_reports_size() {
        let (svc, _tmp) = service_with_blob(&[7u8; 1000], "aa11").await;
        let info = svc.stat("aa11".into()).await.expect("stat");
        assert_eq!(info.size_bytes, 1000);
        assert_eq!(info.mime_type, "application/octet-stream");
        assert!(matches!(
            svc.stat("nope".into()).await,
            Err(MediaError::NotFound)
        ));
    }

    // NOTE: happy-path `read` streaming is covered by
    // `tests/media_stream_e2e.rs` over a real vox link — a
    // `vox::channel()` Tx is a wire construct whose `send` parks until
    // the channel is bound to a session, so it cannot be pumped
    // in-process here.

    #[tokio::test]
    async fn read_past_end_is_invalid_range() {
        let (svc, _tmp) = service_with_blob(b"xyz", "dd44").await;
        let (tx, _rx) = vox::channel::<MediaChunk>();
        assert!(matches!(
            svc.read("dd44".into(), 10, 1, tx).await,
            Err(MediaError::InvalidRange(_))
        ));
    }
}
