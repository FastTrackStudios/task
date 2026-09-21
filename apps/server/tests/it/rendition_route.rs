#![allow(clippy::large_futures)]
//! `/org/{slug}/files/renditions/{root_id}/{kind}/{file_id}` — the
//! Review page's rendition streaming route (issue #270).
//!
//! Renditions live in a root's *private* CAS (issue #269); this route is
//! the only HTTP surface over it. It must honour byte ranges (a `<video>`
//! proxy seek never downloads the whole file) and share `/media`'s
//! authorization exactly: signed `?token=` grant (minted over vox, prefix
//! `files/renditions/{root_id}`) or a session bearer, gated by
//! `TASK_ENFORCE_MEDIA_TOKEN`.
//!
//! Driven end-to-end against a real server with the deterministic
//! `FakeTranscoder` — no ffmpeg, no real media.

use std::sync::Arc;

use files::FilesService as _;
use files_proto::RenditionKind;
use files_transcode::transcoder::FakeTranscoder;
use task_server::{AppState, AuthState, capability::ServerKeypair, router};

static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Boot a server with one org, a Media File Root holding one video file
/// (checkpointed), and the fake transcoder wired in. Returns the base
/// URL, the state, the root id, and the tempdir keeping it all alive.
async fn boot() -> eyre::Result<(String, AppState, uuid::Uuid, tempfile::TempDir)> {
    let auth = AuthState::open("sqlite::memory:", "test-secret-at-least-32-bytes!!!").await?;
    let tmp = tempfile::tempdir()?;
    let guard = ENV_LOCK.lock().await;
    // SAFETY: held under `ENV_LOCK` while `AppState` reads the env.
    unsafe {
        std::env::set_var("TASK_DATA_ROOT", tmp.path());
        std::env::set_var("TASK_ENFORCE_MEDIA_TOKEN", "1");
    }
    let data_root = org_proto::DataRoot::from_env().map_err(|e| eyre::eyre!("data root: {e}"))?;
    data_root
        .init_org("media-test", "Media Test", true)
        .map_err(|e| eyre::eyre!("scaffold org: {e}"))?;
    let state = AppState::new_with_auth(auth, ServerKeypair::generate_ephemeral()).await?;
    drop(guard);

    // A Media root with one video file, checkpointed so the file has a
    // source id at the head — the state a review starts from.
    let org = state.org("media-test").expect("org hosted");
    org.files.set_transcoder(Arc::new(FakeTranscoder));
    // Roots are confined to the org's files area.
    let root_dir = tmp
        .path()
        .join("orgs")
        .join("media-test")
        .join("files")
        .join("session");
    std::fs::create_dir_all(&root_dir)?;
    // The FakeTranscoder classifies by a magic prefix.
    let mut video = b"VIDEO".to_vec();
    video.extend_from_slice(&vec![0x11u8; 4096]);
    std::fs::write(root_dir.join("cut.mov"), &video)?;
    let root = org
        .files
        .create_root(
            root_dir.to_string_lossy().into_owned(),
            "session".into(),
            files_proto::RootFlavor::Media,
        )
        .await
        .map_err(|e| eyre::eyre!("create root: {e}"))?;
    org.files
        .checkpoint_now(root.id, None)
        .await
        .map_err(|e| eyre::eyre!("checkpoint: {e}"))?;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let app = router(state.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok((format!("http://127.0.0.1:{port}"), state, root.id, tmp))
}

/// Mint a grant the way a client does — through the service, so the test
/// exercises the real signing path.
async fn grant(state: &AppState, prefix: &str) -> String {
    use media_proto::AttachmentMediaService as _;
    let org = state.org("media-test").expect("org hosted");
    let keypair = org.attachments.keypair.clone();
    let svc = task_server::media::AttachmentMediaServiceImpl::new(
        org.attachments.clone(),
        org.slug.clone(),
        keypair,
    );
    svc.media_grant(prefix.to_string())
        .await
        .expect("mint grant")
        .token
}

/// One test, not several: `TASK_ENFORCE_MEDIA_TOKEN` is process-wide, so
/// parallel tests in one binary would race the flag (same shape as
/// `media_auth.rs`).
#[tokio::test(flavor = "multi_thread")]
async fn rendition_route_streams_ranges_behind_the_media_grant() -> eyre::Result<()> {
    let (base, state, root_id, _tmp) = boot().await?;
    let org = state.org("media-test").expect("org hosted");

    // Resolve the proxy over the RPC surface — what the Review player
    // does before building its streaming URL.
    let proxy = org
        .files
        .rendition(root_id, "cut.mov".into(), RenditionKind::Proxy720)
        .await
        .map_err(|e| eyre::eyre!("rendition: {e}"))?;
    assert_eq!(proxy.mime, "video/mp4");
    let url = format!(
        "{base}/org/media-test/files/renditions/{root_id}/proxy-720/{}",
        proxy.file_id
    );

    // ── No token: refused (enforcing).
    let r = reqwest::get(&url).await?;
    assert_eq!(r.status().as_u16(), 401, "anonymous rendition read");

    // ── A grant for a DIFFERENT root's renditions must not unlock this
    //    one — the prefix is per (root, review), not per org.
    let stranger = grant(
        &state,
        &format!("files/renditions/{}", uuid::Uuid::new_v4()),
    )
    .await;
    let r = reqwest::get(format!("{url}?token={stranger}")).await?;
    assert_eq!(r.status().as_u16(), 401, "cross-root grant must not leak");

    // ── The real grant: full body, correct type + length, seekable.
    let token = grant(&state, &format!("files/renditions/{root_id}")).await;
    let authed = format!("{url}?token={token}");
    let r = reqwest::get(&authed).await?;
    assert_eq!(r.status().as_u16(), 200);
    assert_eq!(
        r.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("video/mp4")
    );
    assert_eq!(
        r.headers()
            .get("accept-ranges")
            .and_then(|v| v.to_str().ok()),
        Some("bytes"),
        "media elements need Accept-Ranges to consider the source seekable"
    );
    let full = r.bytes().await?;
    assert_eq!(full.len() as u64, proxy.len, "full body is the rendition");

    // ── A byte range: 206 with exactly the window, plus Content-Range —
    //    the `<video>` seek path.
    let client = reqwest::Client::new();
    let r = client
        .get(&authed)
        .header("Range", "bytes=5-9")
        .send()
        .await?;
    assert_eq!(r.status().as_u16(), 206);
    assert_eq!(
        r.headers()
            .get("content-range")
            .and_then(|v| v.to_str().ok()),
        Some(format!("bytes 5-9/{}", proxy.len).as_str())
    );
    let window = r.bytes().await?;
    assert_eq!(&window[..], &full[5..=9], "the window is the file's bytes");

    // ── An open-ended range (`bytes=N-`) resolves to the tail.
    let start = proxy.len - 16;
    let r = client
        .get(&authed)
        .header("Range", format!("bytes={start}-"))
        .send()
        .await?;
    assert_eq!(r.status().as_u16(), 206);
    let tail = r.bytes().await?;
    assert_eq!(&tail[..], &full[usize::try_from(start)?..], "tail window");

    // ── An unknown kind tag is a 404, not a panic or a sniff.
    let r = reqwest::get(format!(
        "{base}/org/media-test/files/renditions/{root_id}/proxy-4k/{}?token={token}",
        proxy.file_id
    ))
    .await?;
    assert_eq!(r.status().as_u16(), 404, "unknown rendition kind");

    // ── A well-formed id the store doesn't hold is a 404 — and in
    //    particular, a SOURCE file's content id must not be servable
    //    here (originals never stream from the rendition route).
    let mut absent = proxy.file_id.clone();
    let flipped = if absent.ends_with('0') { '1' } else { '0' };
    absent.pop();
    absent.push(flipped);
    let r = reqwest::get(format!(
        "{base}/org/media-test/files/renditions/{root_id}/proxy-720/{absent}?token={token}"
    ))
    .await?;
    assert_eq!(r.status().as_u16(), 404, "unknown rendition id");

    Ok(())
}
