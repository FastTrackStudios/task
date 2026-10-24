//! Phase 7 — HTTP routes for blob upload + download.
//!
//! Every endpoint verifies a `?token=…` signed by the server's keypair
//! before doing any work, and serves the request from the attachment
//! service of the org the token is **bound to**. All hosted orgs share
//! one router, so the org comes from the token, never from whichever
//! org happened to be hosted first:
//!
//! - `PUT /org/{slug}/blobs/upload?upload_id=<uuid>&token=<sig>` —
//!   reads the request body, hashes (sha256), persists to that org's
//!   `ObjectStore` under the hash, consumes the org's pending session
//!   for `upload_id`, writes its catalog row, and returns the hash.
//! - `GET /org/{slug}/blobs/download/{content_hash}?token=<sig>` —
//!   the token must be a download grant for this org, this hash, AND
//!   the caller presenting it (`Authorization: Bearer …`, resolved by
//!   the org's own identity resolver). Then streams the blob.
//!
//! The unprefixed `/blobs/upload` and `/blobs/download/{hash}` remain
//! for clients holding an old URL shape; they resolve the org from the
//! token alone. The `/org/{slug}` forms additionally refuse a token
//! bound to a different org.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use architect_permissions::IdentityResolver;
use attachments_proto::AttachmentMeta;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::attachments::service::AttachmentServiceImpl;
use crate::attachments::signed_url::BlobToken;
use crate::capability::ServerKeypair;

/// Router carrying the blob routes. Mounted by `task-server`'s
/// top-level router alongside `/health` + `/vox`.
pub fn attachment_router() -> Router<AttachmentRouteState> {
    Router::new()
        .route("/blobs/upload", put(handle_upload_legacy))
        .route(
            "/blobs/download/{content_hash}",
            get(handle_download_legacy),
        )
        .route("/org/{slug}/blobs/upload", put(handle_upload_org))
        .route(
            "/org/{slug}/blobs/download/{content_hash}",
            get(handle_download_org),
        )
        // Stems and other media blow straight past axum's 2 MB default
        // body cap; uploads are pre-authorized by signed ticket, so a
        // generous cap is safe (multitrack WAVs run hundreds of MB).
        .layer(axum::extract::DefaultBodyLimit::max(1024 * 1024 * 1024))
}

/// How the blob routes reach a hosted org: its attachment service, and
/// the identity resolver its permission gate uses (so a download is
/// redeemed against the same notion of "who" that minted it).
pub trait BlobOrgs: Send + Sync {
    fn attachments(&self, slug: &str) -> Option<Arc<AttachmentServiceImpl>>;
    fn identity(&self, slug: &str) -> Option<Arc<dyn IdentityResolver>>;
}

/// Sliver of `AppState` the routes need. Avoids depending on the
/// full state shape so the attachments module stays decoupled.
#[derive(Clone)]
pub struct AttachmentRouteState {
    /// Verifies every token. Shared by all orgs on this server.
    pub keypair: ServerKeypair,
    pub orgs: Arc<dyn BlobOrgs>,
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

async fn handle_upload_legacy(
    State(state): State<AttachmentRouteState>,
    Query(q): Query<UploadQuery>,
    body: Bytes,
) -> Response {
    handle_upload(state, None, q, body).await
}

async fn handle_upload_org(
    State(state): State<AttachmentRouteState>,
    Path(slug): Path<String>,
    Query(q): Query<UploadQuery>,
    body: Bytes,
) -> Response {
    handle_upload(state, Some(slug), q, body).await
}

/// Verify `token` and resolve the org it is bound to. `path_slug` is
/// the `/org/{slug}` the request named, if any: a token bound to a
/// different org is refused there rather than quietly served from the
/// org it names.
fn bound_org(
    state: &AttachmentRouteState,
    token: &str,
    path_slug: Option<&str>,
    what: &'static str,
) -> Result<(BlobToken, String, Arc<AttachmentServiceImpl>), Response> {
    let tok = BlobToken::verify(token, &state.keypair, now_unix()).map_err(|e| {
        tracing::warn!(?e, "blob {what}: token verify failed");
        (StatusCode::UNAUTHORIZED, "bad token").into_response()
    })?;
    let Some(org) = tok.org.clone() else {
        // Legacy unbound token: names no org, so it can name no store.
        return Err((StatusCode::FORBIDDEN, "token not bound to an org").into_response());
    };
    if path_slug.is_some_and(|p| p != org) {
        tracing::warn!(
            token.org = %org,
            path.org = ?path_slug,
            "blob {what}: token minted for another org"
        );
        return Err((StatusCode::FORBIDDEN, "token minted for another org").into_response());
    }
    let Some(service) = state.orgs.attachments(&org) else {
        return Err((StatusCode::NOT_FOUND, "org not hosted here").into_response());
    };
    Ok((tok, org, service))
}

async fn handle_upload(
    state: AttachmentRouteState,
    path_slug: Option<String>,
    q: UploadQuery,
    body: Bytes,
) -> Response {
    // 1. Token check, and the org it was minted in.
    let (tok, org, service) = match bound_org(&state, &q.token, path_slug.as_deref(), "upload") {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    if !tok.allows_upload(&org, q.upload_id) {
        return (StatusCode::FORBIDDEN, "wrong-purpose token").into_response();
    }

    // 2. Pending session — in THAT org's service.
    let Some(pending) = service.peek_pending(&q.upload_id) else {
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
    if let Err(e) = service.store.put_blob(&hash, &body).await {
        tracing::error!(?e, "blob upload: store write failed");
        return (StatusCode::INTERNAL_SERVER_ERROR, "store error").into_response();
    }

    // 4. Consume the pending session, write the catalog row.
    let pending = service.take_pending(&q.upload_id).unwrap_or(pending);
    let meta = AttachmentMeta {
        content_hash: hash.clone(),
        filename: pending.filename,
        mime_type: pending.mime_type,
        size_bytes: body_len,
        doc_id: pending.doc_id,
    };
    service.record_commit(meta);

    (StatusCode::OK, hash).into_response()
}

async fn handle_download_legacy(
    State(state): State<AttachmentRouteState>,
    Path(content_hash): Path<String>,
    Query(q): Query<DownloadQuery>,
    req_headers: HeaderMap,
) -> Response {
    handle_download(state, None, content_hash, q, req_headers).await
}

async fn handle_download_org(
    State(state): State<AttachmentRouteState>,
    Path((slug, content_hash)): Path<(String, String)>,
    Query(q): Query<DownloadQuery>,
    req_headers: HeaderMap,
) -> Response {
    handle_download(state, Some(slug), content_hash, q, req_headers).await
}

/// The bearer on a blob request, if any.
fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .filter(|t| !t.is_empty())
}

async fn handle_download(
    state: AttachmentRouteState,
    path_slug: Option<String>,
    content_hash: String,
    q: DownloadQuery,
    req_headers: HeaderMap,
) -> Response {
    let (tok, org, service) = match bound_org(&state, &q.token, path_slug.as_deref(), "download") {
        Ok(v) => v,
        Err(resp) => return resp,
    };

    // The grant names the principal that minted it. The request must
    // prove it is that principal, resolved by the same org's identity
    // resolver the permission gate used when it minted. A leaked URL is
    // then useless to anyone else.
    let Some(identity) = state.orgs.identity(&org) else {
        return (StatusCode::NOT_FOUND, "org not hosted here").into_response();
    };
    let Some(presented) = bearer(&req_headers) else {
        return (
            StatusCode::UNAUTHORIZED,
            "blob download needs the minting session",
        )
            .into_response();
    };
    let who = identity.resolve(Some(presented)).await;
    if !tok.allows_download(&org, &who.describe(), &content_hash) {
        tracing::warn!(
            org.slug = %org,
            principal = who.describe(),
            "blob download: grant does not cover this caller or hash"
        );
        return (StatusCode::FORBIDDEN, "grant does not cover this caller").into_response();
    }

    let total = match service.store.blob_len(&content_hash).await {
        Ok(n) => n,
        Err(attachments_proto::AttachmentError::NotFound) => {
            return (StatusCode::NOT_FOUND, "no such blob").into_response();
        }
        Err(e) => {
            tracing::error!(?e, "blob download: store stat failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "store error").into_response();
        }
    };
    let meta = service.catalog.get(&content_hash);
    let mime = match meta.as_ref() {
        Some(m) => m.mime_type.clone(),
        // Catalog is in-memory only — sniff restarts' orphaned blobs from
        // their magic bytes so <audio> gets a playable content type.
        None => match service.store.get_blob(&content_hash).await {
            Ok(head) => super::sniff_mime(head.get(..16).unwrap_or(&head)).to_string(),
            Err(_) => "application/octet-stream".to_string(),
        },
    };
    // `<audio>` seek and progressive playback send `Range`. architect's
    // ranged helper answers 200 / 206 / 416 (malformed headers are
    // ignored per RFC 9110) and reads only the window it selects, so the
    // browser never has to buffer a whole stem.
    let store = &service.store;
    let hash = content_hash.as_str();
    architect::http::ranged::serve_body(&req_headers, total, &mime, |offset, len| async move {
        store
            .get_blob_range(hash, offset, len)
            .await
            .map(axum::body::Body::from)
    })
    .await
    .unwrap_or_else(|e| {
        tracing::error!(?e, "blob download: store read failed");
        (StatusCode::INTERNAL_SERVER_ERROR, "store error").into_response()
    })
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

/// The parser moved to `architect::http::ranged` (v0.9.12); these pin
/// the cases this route's `<audio>` seek depends on against the pin.
#[cfg(test)]
mod range_tests {
    use architect::http::ranged::{ByteRange, parse_range as parse_byte_range};

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
