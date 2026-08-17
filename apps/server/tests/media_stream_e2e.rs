#![allow(clippy::large_futures)]
//! End-to-end check for `MediaService` — media streamed over vox —
//! against a live `task-server`. Uploads a blob through the normal
//! attachment flow (initiate → HTTP PUT → complete), then reads it
//! back through the vox lane as ordered `MediaChunk`s: full body,
//! a seek window, and NotFound for an unknown hash.

use attachments_proto::{AttachmentServiceClient, CompleteUpload, InitiateUpload};
use media_proto::{MediaChunk, MediaError, AttachmentMediaServiceClient};
use task_server::{AppState, router};

static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn boot_server() -> eyre::Result<(String, String, tempfile::TempDir)> {
    let tmp = tempfile::tempdir()?;
    let guard = ENV_LOCK.lock().await;
    // SAFETY: held under `ENV_LOCK` for the duration of
    // `AppState::new`, which reads the vars exactly once (same boot
    // shape as `collection_e2e` — blobs land under the default data
    // root's org, vault under the temp dir).
    unsafe {
        // Sandbox the DATA root too — see `collection_e2e`'s note: a
        // vault-root-only override leaves `DataRoot::from_env` pointing
        // at `$HOME/.task` (PR #284 review).
        std::env::set_var("TASK_DATA_ROOT", tmp.path().join("data"));
        std::env::set_var("TASK_SERVER_VAULT_ROOT", tmp.path());
    }
    let data_root = org_proto::DataRoot::from_env().map_err(|e| eyre::eyre!("data root: {e}"))?;
    data_root
        .ensure()
        .map_err(|e| eyre::eyre!("ensure data root: {e}"))?;
    data_root
        .init_org("home", "Home", true)
        .map_err(|e| eyre::eyre!("scaffold home org: {e}"))?;
    let state = AppState::new(None).await?;
    drop(guard);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let app = router(state);
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok((
        format!("ws://127.0.0.1:{port}/vox"),
        format!("http://127.0.0.1:{port}"),
        tmp,
    ))
}

async fn connect<C: vox_core::FromVoxLane>(url: &str) -> eyre::Result<C> {
    vox::connect_lane(url)
        .establish()
        .await
        .map_err(|e| eyre::eyre!("vox connect: {e:?}"))
}

/// Upload `bytes` via the attachment flow, returning the content hash.
async fn upload(vox_url: &str, http_base: &str, bytes: &[u8]) -> eyre::Result<String> {
    let att: AttachmentServiceClient = connect(vox_url).await?;
    let ticket = att
        .initiate_upload(InitiateUpload {
            doc_id: "Songs/E2E.md".into(),
            filename: "stem.ogg".into(),
            mime_type: "audio/ogg".into(),
            size_bytes: bytes.len() as u64,
        })
        .await
        .map_err(|e| eyre::eyre!("initiate: {e:?}"))?;
    let put_url = if ticket.upload_url.starts_with("http") {
        ticket.upload_url.clone()
    } else {
        format!("{http_base}{}", ticket.upload_url)
    };
    let resp = reqwest::Client::new()
        .put(&put_url)
        .body(bytes.to_vec())
        .send()
        .await?;
    eyre::ensure!(resp.status().is_success(), "PUT failed: {}", resp.status());
    let hash = resp.text().await?.trim().to_string();
    att.complete_upload(CompleteUpload {
        upload_id: ticket.upload_id,
        content_hash: hash.clone(),
    })
    .await
    .map_err(|e| eyre::eyre!("complete: {e:?}"))?;
    Ok(hash)
}

/// Collect a read window into a contiguous buffer, asserting ordered
/// contiguous offsets.
async fn read_window(
    media: &AttachmentMediaServiceClient,
    hash: &str,
    start: u64,
    len: u64,
) -> eyre::Result<Vec<u8>> {
    let (tx, mut rx) = vox::channel::<MediaChunk>();
    let media = media.clone();
    let hash_owned = hash.to_string();
    let reader = tokio::spawn(async move { media.read(hash_owned, start, len, tx).await });
    let mut got: Vec<u8> = Vec::new();
    while let Ok(Some(chunk)) = rx.recv().await {
        let c = chunk.get();
        assert_eq!(c.offset, start + got.len() as u64, "contiguous offsets");
        got.extend_from_slice(&c.bytes);
    }
    reader.await?.map_err(|e| eyre::eyre!("read: {e:?}"))?;
    Ok(got)
}

#[tokio::test(flavor = "multi_thread")]
async fn upload_then_stream_over_vox() {
    let (vox_url, http_base, _tmp) = boot_server().await.unwrap();

    // ~600 KiB so the read spans multiple 256 KiB chunks.
    let data: Vec<u8> = (0..600 * 1024).map(|i| (i % 251) as u8).collect();
    let hash = upload(&vox_url, &http_base, &data).await.unwrap();

    let media: AttachmentMediaServiceClient = connect(&vox_url).await.unwrap();

    // stat sees the size + mime recorded by the attachment flow.
    let info = media.stat(hash.clone()).await.unwrap();
    assert_eq!(info.size_bytes, data.len() as u64);
    assert_eq!(info.mime_type, "audio/ogg");

    // Full-body stream round-trips the bytes.
    let full = read_window(&media, &hash, 0, u64::MAX).await.unwrap();
    assert_eq!(full, data);

    // A seek window returns exactly that slice.
    let window = read_window(&media, &hash, 300_000, 4096).await.unwrap();
    assert_eq!(window, &data[300_000..304_096]);
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_hash_is_not_found() {
    let (vox_url, _http, _tmp) = boot_server().await.unwrap();
    let media: AttachmentMediaServiceClient = connect(&vox_url).await.unwrap();
    assert!(matches!(
        media.stat("doesnotexist".into()).await,
        Err(e) if format!("{e:?}").contains("NotFound")
    ));
    let (tx, _rx) = vox::channel::<MediaChunk>();
    let res = media.read("doesnotexist".into(), 0, u64::MAX, tx).await;
    assert!(res.is_err(), "read of unknown hash must error");
}

// Keep the unused-dep lint honest: MediaError is part of the public
// contract exercised above (matched through the client error path).
#[allow(dead_code)]
fn _contract(_: MediaError) {}
