//! Derived media (issue #269) at the RPC seam: the `rendition` call and
//! the checkpoint-warm-up / GC triggers over an in-process
//! `LocalServer`, driving a deterministic `FakeTranscoder` (no ffmpeg,
//! no real media). One test per acceptance criterion.

use std::sync::Arc;
use std::time::Duration;

use architect::{LayerRouter, LocalServer, Scope};
use files::{FilesBackend, RootFlavor};
use files_proto::RenditionKind;
use files_transcode::transcoder::FakeTranscoder;

fn router(backend: FilesBackend) -> LayerRouter {
    LayerRouter::new().merge(files::files_service_layer(backend))
}

/// A media root with a "VIDEO" file and an "AUDIO" file, checkpointed,
/// with the fake transcoder wired in.
async fn rig() -> (
    tempfile::TempDir,
    FilesBackend,
    files::FilesServiceClient,
    uuid::Uuid,
    LocalServer,
) {
    let dir = tempfile::tempdir().unwrap();
    let backend = FilesBackend::new(dir.path(), dir.path().join("vault")).unwrap();
    backend.set_transcoder(Arc::new(FakeTranscoder));

    let root_dir = dir.path().join("session");
    std::fs::create_dir(&root_dir).unwrap();
    // The FakeTranscoder classifies by a magic prefix.
    let mut video = b"VIDEO".to_vec();
    video.extend_from_slice(&vec![0x11u8; 4096]);
    std::fs::write(root_dir.join("cut.mov"), &video).unwrap();
    let mut audio = b"AUDIO".to_vec();
    audio.extend_from_slice(&vec![0x22u8; 4096]);
    std::fs::write(root_dir.join("mix.wav"), &audio).unwrap();

    let scope = Scope::new();
    let local = LocalServer::serve(router(backend.clone()), scope.clone());
    let client: files::FilesServiceClient = local.establish().await.unwrap();
    let root = client
        .create_root(
            root_dir.to_string_lossy().into_owned(),
            "session".into(),
            RootFlavor::Media,
        )
        .await
        .unwrap();
    (dir, backend, client, root.id, local)
}

/// AC 1 (via the checkpoint warm-up) + AC 2 (lazy generate then cache):
/// after a checkpoint warms the ladder, a `rendition` request returns a
/// cached handle; a proxy of the video and peaks of the audio both
/// resolve, and an inapplicable kind is refused.
#[tokio::test(flavor = "multi_thread")]
async fn rendition_generates_and_caches_per_media_class() {
    let (_dir, backend, client, root_id, _local) = rig().await;
    client.checkpoint_now(root_id, None).await.unwrap();
    // The warm-up is spawned detached; give it a moment (deterministic
    // fake, so this is fast), then request.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Video → a 720p proxy, with the right MIME.
    let proxy = client
        .rendition(root_id, "cut.mov".into(), RenditionKind::Proxy720)
        .await
        .expect("video proxy");
    assert_eq!(proxy.mime, "video/mp4");
    assert!(!proxy.file_id.is_empty());

    // Requesting again returns the SAME cached content id (generated
    // once — AC 2).
    let again = client
        .rendition(root_id, "cut.mov".into(), RenditionKind::Proxy720)
        .await
        .unwrap();
    assert_eq!(proxy.file_id, again.file_id);

    // Audio → peaks; a filmstrip of audio is refused.
    let peaks = client
        .rendition(root_id, "mix.wav".into(), RenditionKind::Peaks)
        .await
        .expect("audio peaks");
    assert_eq!(peaks.mime, "application/octet-stream");
    let err = client
        .rendition(root_id, "mix.wav".into(), RenditionKind::Filmstrip)
        .await
        .expect_err("filmstrip of audio must fail");
    assert!(err.to_string().contains("does not apply"), "{err}");

    backend.shutdown().await;
}

/// The version switcher (issue #270 AC 4): `rendition_at` renders the
/// file as it was at a PAST commit, distinct from the head's rendition
/// (each version's proxy is keyed by that version's own content), and
/// `rendition_at(head)` hits the same cache entry as `rendition`.
#[tokio::test(flavor = "multi_thread")]
async fn rendition_at_serves_past_versions() {
    let (dir, backend, client, root_id, _local) = rig().await;
    client.checkpoint_now(root_id, None).await.unwrap();
    let v1_proxy = client
        .rendition(root_id, "cut.mov".into(), RenditionKind::Proxy720)
        .await
        .expect("v1 proxy");

    // A new cut of the video lands as a second version.
    let mut video = b"VIDEO".to_vec();
    video.extend_from_slice(&vec![0x33u8; 8192]);
    std::fs::write(dir.path().join("session/cut.mov"), &video).unwrap();
    client.checkpoint_now(root_id, None).await.unwrap();
    let chain = client.chain(root_id, "cut.mov".into()).await.unwrap();
    assert!(chain.len() >= 2, "two versions: {}", chain.len());
    let (v2, v1) = (&chain[0].commit_id, &chain[1].commit_id);

    let old = client
        .rendition_at(
            root_id,
            "cut.mov".into(),
            v1.clone(),
            RenditionKind::Proxy720,
        )
        .await
        .expect("past version proxy");
    assert_eq!(
        old.file_id, v1_proxy.file_id,
        "the old version's proxy is exactly what v1 rendered"
    );
    let new = client
        .rendition_at(
            root_id,
            "cut.mov".into(),
            v2.clone(),
            RenditionKind::Proxy720,
        )
        .await
        .expect("head version proxy");
    assert_ne!(
        old.file_id, new.file_id,
        "different content, different proxy"
    );
    let head = client
        .rendition(root_id, "cut.mov".into(), RenditionKind::Proxy720)
        .await
        .unwrap();
    assert_eq!(new.file_id, head.file_id, "rendition_at(head) == rendition");

    // A commit that doesn't exist is a clean error.
    client
        .rendition_at(
            root_id,
            "cut.mov".into(),
            "0000000000000000".into(),
            RenditionKind::Proxy720,
        )
        .await
        .expect_err("unknown commit");

    backend.shutdown().await;
}

/// A rendition request without a transcoder configured is a clean
/// NotFound, not a panic.
#[tokio::test(flavor = "multi_thread")]
async fn rendition_without_a_transcoder_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let backend = FilesBackend::new(dir.path(), dir.path().join("vault")).unwrap();
    let root_dir = dir.path().join("session");
    std::fs::create_dir(&root_dir).unwrap();
    let mut video = b"VIDEO".to_vec();
    video.extend_from_slice(&vec![1u8; 1024]);
    std::fs::write(root_dir.join("cut.mov"), &video).unwrap();
    let scope = Scope::new();
    let local = LocalServer::serve(router(backend.clone()), scope);
    let client: files::FilesServiceClient = local.establish().await.unwrap();
    let root = client
        .create_root(
            root_dir.to_string_lossy().into_owned(),
            "session".into(),
            RootFlavor::Media,
        )
        .await
        .unwrap();
    client.checkpoint_now(root.id, None).await.unwrap();
    let err = client
        .rendition(root.id, "cut.mov".into(), RenditionKind::Proxy720)
        .await
        .expect_err("no transcoder");
    assert!(
        err.to_string().to_lowercase().contains("transcoder"),
        "{err}"
    );
}

/// AC 3, at the integration level: `gc_root` runs the source-tied
/// rendition GC and a *live* source's renditions survive it (a
/// still-referenced source is never over-swept). The dead-source sweep
/// itself is proven in `files-transcode`'s unit test — deleting a file
/// and re-checkpointing keeps its content alive (the previous
/// checkpoint still references it), so a source dying is not something
/// an ordinary gc_root produces.
#[tokio::test(flavor = "multi_thread")]
async fn gc_keeps_a_live_sources_renditions() {
    let (_dir, backend, client, root_id, _local) = rig().await;
    client.checkpoint_now(root_id, None).await.unwrap();

    let proxy = client
        .rendition(root_id, "cut.mov".into(), RenditionKind::Proxy720)
        .await
        .unwrap();
    assert!(
        backend
            .rendition_content_present(root_id, &proxy.file_id)
            .await
            .unwrap(),
        "rendition cached in the CAS"
    );

    // The bytes are streamable off the private rendition CAS — the read
    // primitive the Review page (issue #270) serves from.
    let mut bytes = Vec::new();
    backend
        .read_rendition(root_id, &proxy.file_id, &mut bytes)
        .await
        .unwrap();
    assert_eq!(bytes.len() as u64, proxy.len, "streamed the full rendition");
    assert!(!bytes.is_empty());

    // gc_root runs the version-store sweep + the source-tied rendition
    // GC. The source is still referenced (live), so its rendition is
    // kept — not over-swept.
    client.gc_root(root_id, Some(0)).await.unwrap();
    assert!(
        backend
            .rendition_content_present(root_id, &proxy.file_id)
            .await
            .unwrap(),
        "a live source's rendition survives GC"
    );
    // And it's still served from cache (same content id).
    let again = client
        .rendition(root_id, "cut.mov".into(), RenditionKind::Proxy720)
        .await
        .unwrap();
    assert_eq!(proxy.file_id, again.file_id);

    backend.shutdown().await;
}
