//! Phase 7 — HTTP routes for blob upload + download.
//!
//! Two endpoints, both verifying a `?token=…` signed by the
//! server's keypair before doing any work:
//!
//! - `PUT /blobs/upload?upload_id=<uuid>&token=<sig>` — reads the
//!   request body, hashes (sha256), persists to the `ObjectStore`
//!   under the hash. Looks up the pending session by `upload_id`,
//!   consumes it, writes a catalog row, and returns the hash in
//!   the response.
//! - `GET /blobs/download/{content_hash}?token=<sig>` — verifies
//!   the token's subject matches the path, streams the blob.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use attachments_proto::AttachmentMeta;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::{get, put};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::attachments::service::AttachmentServiceImpl;
use crate::attachments::signed_url::{BlobToken, BlobTokenPurpose};

/// Router carrying the two blob routes. Mounted by `task-server`'s
/// top-level router alongside `/health` + `/vox`.
pub fn attachment_router() -> Router<AttachmentRouteState> {
    Router::new()
        .route("/blobs/upload", put(handle_upload))
        .route("/blobs/download/{content_hash}", get(handle_download))
        // Stems and other media blow straight past axum's 2 MB default
        // body cap; uploads are pre-authorized by signed ticket, so a
        // generous cap is safe (multitrack WAVs run hundreds of MB).
        .layer(axum::extract::DefaultBodyLimit::max(1024 * 1024 * 1024))
}

/// Sliver of `AppState` the routes need. Avoids depending on the
/// full state shape so the attachments module stays decoupled.
#[derive(Clone)]
pub struct AttachmentRouteState {
    pub service: Arc<AttachmentServiceImpl>,
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Debug, Deserialize)]
pub struct UploadQuery {
    upload_id: Uuid,
    token: String,
}

async fn handle_upload(
    State(state): State<AttachmentRouteState>,
    Query(q): Query<UploadQuery>,
    body: Bytes,
) -> impl IntoResponse {
    // 1. Token check.
    let tok = match BlobToken::verify(&q.token, &state.service.keypair, now_unix()) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(?e, "blob upload: token verify failed");
            return (StatusCode::UNAUTHORIZED, "bad token").into_response();
        }
    };
    if !matches!(tok.purpose, BlobTokenPurpose::Upload) || tok.upload_id() != Some(q.upload_id) {
        return (StatusCode::FORBIDDEN, "wrong-purpose token").into_response();
    }

    // 2. Pending session?
    let Some(pending) = state.service.peek_pending(&q.upload_id) else {
        return (StatusCode::NOT_FOUND, "no pending upload for that id").into_response();
    };
    // Optional sanity: caller said size_bytes; reject grossly
    // mismatched bodies (we don't enforce exactness — the schema
    // may have lied; just block runaway uploads).
    let body_len = body.len() as u64;
    if pending.size_bytes != 0 && body_len > pending.size_bytes.saturating_mul(2) {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("body {body_len} exceeds 2x declared {}", pending.size_bytes),
        )
            .into_response();
    }

    // 3. Hash + persist.
    let mut hasher = Sha256::new();
    hasher.update(&body);
    let hash = hex_encode(&hasher.finalize());
    if let Err(e) = state.service.store.put_blob(&hash, &body).await {
        tracing::error!(?e, "blob upload: store write failed");
        return (StatusCode::INTERNAL_SERVER_ERROR, "store error").into_response();
    }

    // 4. Consume the pending session, write the catalog row.
    let pending = state.service.take_pending(&q.upload_id).unwrap_or(pending);
    let meta = AttachmentMeta {
        content_hash: hash.clone(),
        filename: pending.filename,
        mime_type: pending.mime_type,
        size_bytes: body_len,
        doc_id: pending.doc_id,
    };
    state.service.record_commit(meta);

    (StatusCode::OK, hash).into_response()
}

async fn handle_download(
    State(state): State<AttachmentRouteState>,
    Path(content_hash): Path<String>,
    Query(q): Query<DownloadQuery>,
    req_headers: HeaderMap,
) -> impl IntoResponse {
    let tok = match BlobToken::verify(&q.token, &state.service.keypair, now_unix()) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(?e, "blob download: token verify failed");
            return (StatusCode::UNAUTHORIZED, "bad token").into_response();
        }
    };
    if !matches!(tok.purpose, BlobTokenPurpose::Download)
        || tok.content_hash() != Some(content_hash.as_str())
    {
        return (StatusCode::FORBIDDEN, "wrong-purpose token").into_response();
    }

    let total = match state.service.store.blob_len(&content_hash).await {
        Ok(n) => n,
        Err(attachments_proto::AttachmentError::NotFound) => {
            return (StatusCode::NOT_FOUND, "no such blob").into_response();
        }
        Err(e) => {
            tracing::error!(?e, "blob download: store stat failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "store error").into_response();
        }
    };
    let meta = state.service.catalog.get(&content_hash);
    let mime = match meta.as_ref() {
        Some(m) => m.mime_type.clone(),
        // Catalog is in-memory only — sniff restarts' orphaned blobs from
        // their magic bytes so <audio> gets a playable content type.
        None => match state.service.store.get_blob(&content_hash).await {
            Ok(head) => super::sniff_mime(head.get(..16).unwrap_or(&head)).to_string(),
            Err(_) => "application/octet-stream".to_string(),
        },
    };
    let base_headers = |body_len: u64| {
        [
            (header::CONTENT_TYPE, mime.clone()),
            (header::ACCEPT_RANGES, "bytes".into()),
            (header::CONTENT_LENGTH, body_len.to_string()),
        ]
    };

    // `<audio>` seek and progressive playback send `Range`; honor a
    // single `bytes=` range with 206 so the browser never has to
    // buffer a whole stem. Malformed headers are ignored (200 full
    // body, per RFC 9110); syntactically valid but unsatisfiable
    // ranges get 416.
    let range = req_headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map_or(ByteRange::Absent, |s| parse_byte_range(s, total));
    match range {
        ByteRange::Satisfiable(start, end) => {
            let len = end - start + 1;
            match state
                .service
                .store
                .get_blob_range(&content_hash, start, len)
                .await
            {
                Ok(bytes) => (
                    StatusCode::PARTIAL_CONTENT,
                    base_headers(len),
                    [(
                        header::CONTENT_RANGE,
                        format!("bytes {start}-{end}/{total}"),
                    )],
                    bytes,
                )
                    .into_response(),
                Err(e) => {
                    tracing::error!(?e, "blob download: ranged read failed");
                    (StatusCode::INTERNAL_SERVER_ERROR, "store error").into_response()
                }
            }
        }
        ByteRange::Unsatisfiable => (
            StatusCode::RANGE_NOT_SATISFIABLE,
            [(header::CONTENT_RANGE, format!("bytes */{total}"))],
        )
            .into_response(),
        ByteRange::Absent => match state.service.store.get_blob(&content_hash).await {
            Ok(bytes) => (StatusCode::OK, base_headers(total), bytes).into_response(),
            Err(e) => {
                tracing::error!(?e, "blob download: store read failed");
                (StatusCode::INTERNAL_SERVER_ERROR, "store error").into_response()
            }
        },
    }
}

/// Outcome of resolving a request's `Range` header against a blob.
#[derive(Debug, PartialEq, Eq)]
enum ByteRange {
    /// No Range header, or one malformed enough to ignore (RFC 9110
    /// lets a server ignore Range) → serve the full body, 200.
    Absent,
    /// A single satisfiable `bytes=` range, resolved to inclusive
    /// `(start, end)` → 206.
    Satisfiable(u64, u64),
    /// Syntactically valid `bytes=` spec that selects nothing → 416.
    Unsatisfiable,
}

/// Parse a `Range` header value against a resource of `total` bytes.
/// Single range only; multi-range (`a-b,c-d`) is never sent by media
/// elements and isn't worth multipart/byteranges here (ignored →
/// full body).
fn parse_byte_range(raw: &str, total: u64) -> ByteRange {
    let Some(spec) = raw.trim().strip_prefix("bytes=") else {
        return ByteRange::Absent;
    };
    let spec = spec.trim();
    if spec.contains(',') {
        return ByteRange::Absent;
    }
    let Some((from, to)) = spec.split_once('-') else {
        return ByteRange::Absent;
    };
    let (from, to) = (from.trim(), to.trim());
    let (start, end) = match (from.parse::<u64>(), to.parse::<u64>()) {
        // `bytes=a-b`
        (Ok(a), Ok(b)) if !from.is_empty() && !to.is_empty() => (a, b),
        // `bytes=a-` → to end
        (Ok(a), Err(_)) if to.is_empty() => (a, total.saturating_sub(1)),
        // `bytes=-n` → last n bytes
        (Err(_), Ok(n)) if from.is_empty() => {
            if n == 0 || total == 0 {
                return ByteRange::Unsatisfiable;
            }
            (total.saturating_sub(n), total - 1)
        }
        _ => return ByteRange::Absent,
    };
    if start <= end && start < total {
        ByteRange::Satisfiable(start, end.min(total.saturating_sub(1)))
    } else {
        ByteRange::Unsatisfiable
    }
}

#[cfg(test)]
mod range_tests {
    use super::{ByteRange, parse_byte_range};

    #[test]
    fn plain_ranges_resolve() {
        assert_eq!(
            parse_byte_range("bytes=0-499", 1000),
            ByteRange::Satisfiable(0, 499)
        );
        assert_eq!(
            parse_byte_range("bytes=500-", 1000),
            ByteRange::Satisfiable(500, 999)
        );
        assert_eq!(
            parse_byte_range("bytes=-200", 1000),
            ByteRange::Satisfiable(800, 999)
        );
        // end past EOF clamps
        assert_eq!(
            parse_byte_range("bytes=900-2000", 1000),
            ByteRange::Satisfiable(900, 999)
        );
        // suffix longer than the blob → whole blob
        assert_eq!(
            parse_byte_range("bytes=-5000", 1000),
            ByteRange::Satisfiable(0, 999)
        );
    }

    #[test]
    fn unsatisfiable_ranges_416() {
        assert_eq!(
            parse_byte_range("bytes=1000-", 1000),
            ByteRange::Unsatisfiable
        );
        assert_eq!(
            parse_byte_range("bytes=5-2", 1000),
            ByteRange::Unsatisfiable
        );
        assert_eq!(parse_byte_range("bytes=-0", 1000), ByteRange::Unsatisfiable);
        assert_eq!(parse_byte_range("bytes=0-", 0), ByteRange::Unsatisfiable);
    }

    #[test]
    fn malformed_ranges_ignored() {
        assert_eq!(parse_byte_range("chunks=0-1", 1000), ByteRange::Absent);
        assert_eq!(parse_byte_range("bytes=a-b", 1000), ByteRange::Absent);
        assert_eq!(parse_byte_range("bytes=0-1,5-9", 1000), ByteRange::Absent);
        assert_eq!(parse_byte_range("bytes=-", 1000), ByteRange::Absent);
    }
}

#[derive(Debug, Deserialize)]
pub struct DownloadQuery {
    token: String,
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}
