#![allow(clippy::large_futures)]
//! Byte ranges on the two plain-HTTP byte routes that had no wire test:
//! `/org/{slug}/media/{path}` (a file on disk) and
//! `/blobs/download/{hash}` (the attachment store). Both now answer
//! through `architect::http::ranged` (STA-396): `200` whole with
//! `Accept-Ranges`, `206` + `Content-Range` for one range (the `<audio>`
//! seek path), `416` + `bytes */total` for a range that selects nothing.
//! The rendition and share routes are covered by `rendition_route.rs`
//! and `share_files_e2e.rs`.

use attachments_proto::{AttachmentService as _, ContentHashArg};
use task_server::{AppState, AuthState, capability::ServerKeypair, router};

/// The bytes both routes serve: 1000 distinct-ish bytes so every window
/// is checkable.
fn payload() -> Vec<u8> {
    (0..1000_u32).map(|i| (i % 251) as u8).collect()
}

async fn boot() -> eyre::Result<(String, AppState, tempfile::TempDir)> {
    let auth = AuthState::open("sqlite::memory:", "test-secret-at-least-32-bytes!!!").await?;
    let tmp = tempfile::tempdir()?;
    let guard = crate::support::env_lock().await;
    // SAFETY: held under `ENV_LOCK` while `AppState` reads the env.
    unsafe {
        std::env::set_var("TASK_DATA_ROOT", tmp.path());
    }
    let data_root = org_proto::DataRoot::from_env().map_err(|e| eyre::eyre!("data root: {e}"))?;
    data_root
        .init_org("range-test", "Range Test", true)
        .map_err(|e| eyre::eyre!("scaffold org: {e}"))?;
    let stems = data_root
        .org("range-test")
        .path()
        .join("resources")
        .join("songs")
        .join("hymn")
        .join("stems");
    std::fs::create_dir_all(&stems)?;
    std::fs::write(stems.join("bass.ogg"), payload())?;
    let state = AppState::new_with_auth(auth, ServerKeypair::generate_ephemeral()).await?;
    drop(guard);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let app = router(state.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok((format!("http://127.0.0.1:{port}"), state, tmp))
}

/// A media grant minted through the service, so the test passes whether
/// or not another test in this process left `TASK_ENFORCE_MEDIA_TOKEN` on.
async fn grant(state: &AppState, prefix: &str) -> String {
    use media_proto::AttachmentMediaService as _;
    let org = state.org("range-test").expect("org hosted");
    let svc = task_server::media::AttachmentMediaServiceImpl::new(
        org.attachments.clone(),
        org.slug.clone(),
        org.attachments.keypair.clone(),
    );
    svc.media_grant(prefix.to_string())
        .await
        .expect("mint grant")
        .token
}

fn header(r: &reqwest::Response, name: &str) -> Option<String> {
    r.headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

/// Drive one URL through the whole range contract.
async fn assert_ranged(url: &str, mime: &str, what: &str) -> eyre::Result<()> {
    let data = payload();
    let client = reqwest::Client::new();

    // No Range: the whole body, advertised as seekable.
    let r = client.get(url).send().await?;
    assert_eq!(r.status().as_u16(), 200, "{what}: whole body");
    assert_eq!(
        header(&r, "accept-ranges").as_deref(),
        Some("bytes"),
        "{what}"
    );
    assert_eq!(header(&r, "content-type").as_deref(), Some(mime), "{what}");
    assert_eq!(
        header(&r, "content-length").as_deref(),
        Some("1000"),
        "{what}"
    );
    assert_eq!(&r.bytes().await?[..], &data[..], "{what}: whole bytes");

    // One range: 206 with exactly that window (the `<audio>` seek).
    let r = client
        .get(url)
        .header("Range", "bytes=100-199")
        .send()
        .await?;
    assert_eq!(r.status().as_u16(), 206, "{what}: seek");
    assert_eq!(
        header(&r, "content-range").as_deref(),
        Some("bytes 100-199/1000"),
        "{what}"
    );
    assert_eq!(
        header(&r, "content-length").as_deref(),
        Some("100"),
        "{what}"
    );
    assert_eq!(&r.bytes().await?[..], &data[100..200], "{what}: window");

    // Chrome's probe (`bytes=0-`) and a suffix (Ogg reads its tail for
    // the duration).
    let r = client.get(url).header("Range", "bytes=0-").send().await?;
    assert_eq!(r.status().as_u16(), 206, "{what}: probe");
    assert_eq!(
        header(&r, "content-range").as_deref(),
        Some("bytes 0-999/1000"),
        "{what}"
    );
    let r = client.get(url).header("Range", "bytes=-10").send().await?;
    assert_eq!(r.status().as_u16(), 206, "{what}: suffix");
    assert_eq!(&r.bytes().await?[..], &data[990..], "{what}: tail");

    // A range past the end selects nothing: 416, no body.
    let r = client
        .get(url)
        .header("Range", "bytes=5000-")
        .send()
        .await?;
    assert_eq!(r.status().as_u16(), 416, "{what}: unsatisfiable");
    assert_eq!(
        header(&r, "content-range").as_deref(),
        Some("bytes */1000"),
        "{what}"
    );
    assert!(r.bytes().await?.is_empty(), "{what}: 416 has no body");

    // Several ranges are not understood: ignored, whole body.
    let r = client
        .get(url)
        .header("Range", "bytes=0-1,5-6")
        .send()
        .await?;
    assert_eq!(r.status().as_u16(), 200, "{what}: multi-range ignored");
    assert_eq!(r.bytes().await?.len(), 1000, "{what}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn media_and_blob_routes_serve_byte_ranges() -> eyre::Result<()> {
    let (base, state, _tmp) = boot().await?;

    // `/media`: a stem on disk, streamed by `serve_file`.
    let token = grant(&state, "songs/hymn").await;
    let media = format!("{base}/org/range-test/media/songs/hymn/stems/bass.ogg?token={token}");
    assert_ranged(&media, "audio/ogg", "media").await?;
    let r = reqwest::get(format!(
        "{base}/org/range-test/media/songs/hymn/stems/absent.ogg?token={token}"
    ))
    .await?;
    assert_eq!(r.status().as_u16(), 404, "a missing media file");

    // `/blobs/download`: a blob in the attachment store, read through
    // `serve_body` (only the window is fetched from the store).
    let org = state.org("range-test").expect("org hosted");
    let hash = "ab".repeat(32);
    org.attachments
        .store
        .put_blob(&hash, &payload())
        .await
        .map_err(|e| eyre::eyre!("put blob: {e:?}"))?;
    let signed = org
        .attachments
        .get_download_url(ContentHashArg {
            content_hash: hash.clone(),
        })
        .await
        .map_err(|e| eyre::eyre!("download url: {e:?}"))?;
    let blob = if signed.url.starts_with("http") {
        signed.url
    } else {
        format!("{base}{}", signed.url)
    };
    // Not in the in-memory catalog, so the type is sniffed; 1000 bytes of
    // a counting pattern sniff as octet-stream.
    assert_ranged(&blob, "application/octet-stream", "blob").await?;
    Ok(())
}
