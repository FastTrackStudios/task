//! Derived media (issue #269) at the RPC seam: the `rendition_info`
//! call and the checkpoint-warm-up / GC triggers over an in-process
//! `LocalServer`, driving a deterministic `FakeTranscoder` (no ffmpeg,
//! no real media). One test per acceptance criterion.

use std::sync::Arc;
use std::time::Duration;

use architect::{LayerRouter, LocalServer, Scope};
use files::{FilesBackend, RootFlavor};
use files_proto::service::roots::AdoptRequest;
use files_proto::{
    FilesFault, MediaServiceClient, RenditionKind, RootId, RootPath, RootsServiceClient, VersionId,
    VersionServiceClient,
};
use files_transcode::transcoder::FakeTranscoder;

fn router(backend: FilesBackend) -> LayerRouter {
    LayerRouter::new()
        .merge(files_proto::roots_layer(backend.clone()))
        .merge(files_proto::version_layer(backend.clone()))
        .merge(files_proto::media_layer(backend))
}

/// The lanes this file drives, over one in-process link.
struct Rig {
    backend: FilesBackend,
    version: VersionServiceClient,
    media: MediaServiceClient,
    root_id: RootId,
    _local: LocalServer,
}

fn p(s: &str) -> RootPath {
    RootPath::parse(s).unwrap()
}

/// The domain fault inside a transport error, as text.
fn fault_text(err: vox::VoxError<FilesFault>) -> String {
    match err {
        vox::VoxError::User(fault) => fault.to_string(),
        other => panic!("expected a domain fault, got {other:?}"),
    }
}

/// Serve `backend` and adopt `root_dir` as a media root, waiting for the
/// walk. Not checkpointed.
async fn serve(backend: FilesBackend, root_dir: &std::path::Path) -> Rig {
    let local = LocalServer::serve(router(backend.clone()), Scope::new());
    let roots: RootsServiceClient = local.establish().await.unwrap();
    let root = roots
        .adopt(AdoptRequest {
            path: root_dir.to_string_lossy().into_owned(),
            name: "session".into(),
            flavor: RootFlavor::Media,
            hash_content: true,
        })
        .await
        .unwrap();
    let root_id = RootId::new(root.id);
    backend.settled(root_id).await;
    Rig {
        backend,
        version: local.establish().await.unwrap(),
        media: local.establish().await.unwrap(),
        root_id,
        _local: local,
    }
}

/// A media root with a "VIDEO" file and an "AUDIO" file, adopted, with
/// the fake transcoder wired in.
async fn rig() -> (tempfile::TempDir, Rig) {
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

    let rig = serve(backend, &root_dir).await;
    (dir, rig)
}

/// AC 1 (via the checkpoint warm-up) + AC 2 (lazy generate then cache):
/// after a checkpoint warms the ladder, a rendition request returns a
/// cached handle; a proxy of the video and peaks of the audio both
/// resolve, and an inapplicable kind is refused.
#[tokio::test(flavor = "multi_thread")]
async fn rendition_generates_and_caches_per_media_class() {
    let (_dir, rig) = rig().await;
    let root_id = rig.root_id;
    rig.version.checkpoint(root_id, None).await.unwrap();
    // The warm-up is spawned detached; give it a moment (deterministic
    // fake, so this is fast), then request.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Video → a 720p proxy, with the right MIME.
    let proxy = rig
        .media
        .rendition_info(root_id, p("cut.mov"), RenditionKind::Proxy720, None)
        .await
        .expect("video proxy");
    assert_eq!(proxy.mime, "video/mp4");
    assert!(!proxy.file_id.is_empty());

    // Requesting again returns the SAME cached content id (generated
    // once — AC 2).
    let again = rig
        .media
        .rendition_info(root_id, p("cut.mov"), RenditionKind::Proxy720, None)
        .await
        .unwrap();
    assert_eq!(proxy.file_id, again.file_id);

    // Audio → peaks; a filmstrip of audio is refused.
    let peaks = rig
        .media
        .rendition_info(root_id, p("mix.wav"), RenditionKind::Peaks, None)
        .await
        .expect("audio peaks");
    assert_eq!(peaks.mime, "application/octet-stream");
    let err = rig
        .media
        .rendition_info(root_id, p("mix.wav"), RenditionKind::Filmstrip, None)
        .await
        .expect_err("filmstrip of audio must fail");
    let text = fault_text(err);
    assert!(text.contains("does not apply"), "{text}");

    rig.backend.shutdown().await;
}

/// The version switcher (issue #270 AC 4): a rendition at a PAST
/// version renders the file as it was then, distinct from the head's
/// rendition (each version's proxy is keyed by that version's own
/// content), and a rendition at the head hits the same cache entry as
/// the current one.
#[tokio::test(flavor = "multi_thread")]
async fn rendition_at_serves_past_versions() {
    let (dir, rig) = rig().await;
    let root_id = rig.root_id;
    rig.version.checkpoint(root_id, None).await.unwrap();
    let v1_proxy = rig
        .media
        .rendition_info(root_id, p("cut.mov"), RenditionKind::Proxy720, None)
        .await
        .expect("v1 proxy");

    // A new cut of the video lands as a second version.
    let mut video = b"VIDEO".to_vec();
    video.extend_from_slice(&vec![0x33u8; 8192]);
    std::fs::write(dir.path().join("session/cut.mov"), &video).unwrap();
    rig.version.checkpoint(root_id, None).await.unwrap();
    let chain = rig.version.chain(root_id, p("cut.mov")).await.unwrap();
    assert!(chain.len() >= 2, "two versions: {}", chain.len());
    let (v2, v1) = (&chain[0].commit_id, &chain[1].commit_id);

    let old = rig
        .media
        .rendition_info(
            root_id,
            p("cut.mov"),
            RenditionKind::Proxy720,
            Some(VersionId::from_commit_hex(v1)),
        )
        .await
        .expect("past version proxy");
    assert_eq!(
        old.file_id, v1_proxy.file_id,
        "the old version's proxy is exactly what v1 rendered"
    );
    let new = rig
        .media
        .rendition_info(
            root_id,
            p("cut.mov"),
            RenditionKind::Proxy720,
            Some(VersionId::from_commit_hex(v2)),
        )
        .await
        .expect("head version proxy");
    assert_ne!(
        old.file_id, new.file_id,
        "different content, different proxy"
    );
    let head = rig
        .media
        .rendition_info(root_id, p("cut.mov"), RenditionKind::Proxy720, None)
        .await
        .unwrap();
    assert_eq!(new.file_id, head.file_id, "rendition at head == rendition");

    // A version that doesn't exist is a clean error.
    rig.media
        .rendition_info(
            root_id,
            p("cut.mov"),
            RenditionKind::Proxy720,
            Some(VersionId::from_commit_hex(
                "00000000000000000000000000000000",
            )),
        )
        .await
        .expect_err("unknown commit");

    rig.backend.shutdown().await;
}

/// A rendition request without a transcoder configured is a clean
/// refusal, not a panic.
#[tokio::test(flavor = "multi_thread")]
async fn rendition_without_a_transcoder_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let backend = FilesBackend::new(dir.path(), dir.path().join("vault")).unwrap();
    let root_dir = dir.path().join("session");
    std::fs::create_dir(&root_dir).unwrap();
    let mut video = b"VIDEO".to_vec();
    video.extend_from_slice(&vec![1u8; 1024]);
    std::fs::write(root_dir.join("cut.mov"), &video).unwrap();
    let rig = serve(backend, &root_dir).await;
    rig.version.checkpoint(rig.root_id, None).await.unwrap();
    let err = rig
        .media
        .rendition_info(rig.root_id, p("cut.mov"), RenditionKind::Proxy720, None)
        .await
        .expect_err("no transcoder");
    let text = fault_text(err);
    assert!(text.to_lowercase().contains("transcoder"), "{text}");
}

/// AC 3, at the integration level: `collect` runs the source-tied
/// rendition GC and a *live* source's renditions survive it (a
/// still-referenced source is never over-swept). The dead-source sweep
/// itself is proven in `files-transcode`'s unit test — deleting a file
/// and re-checkpointing keeps its content alive (the previous
/// checkpoint still references it), so a source dying is not something
/// an ordinary collection produces.
#[tokio::test(flavor = "multi_thread")]
async fn gc_keeps_a_live_sources_renditions() {
    let (_dir, rig) = rig().await;
    let root_id = rig.root_id;
    rig.version.checkpoint(root_id, None).await.unwrap();

    let proxy = rig
        .media
        .rendition_info(root_id, p("cut.mov"), RenditionKind::Proxy720, None)
        .await
        .unwrap();
    assert!(
        rig.backend
            .rendition_content_present(root_id.get(), &proxy.file_id)
            .await
            .unwrap(),
        "rendition cached in the CAS"
    );

    // The bytes are streamable off the private rendition CAS — the read
    // primitive the Review page (issue #270) serves from.
    let mut bytes = Vec::new();
    rig.backend
        .read_rendition(root_id.get(), &proxy.file_id, &mut bytes)
        .await
        .unwrap();
    assert_eq!(bytes.len() as u64, proxy.len, "streamed the full rendition");
    assert!(!bytes.is_empty());

    // `collect` runs the version-store sweep + the source-tied rendition
    // GC. The source is still referenced (live), so its rendition is
    // kept — not over-swept.
    rig.version.collect(root_id, Some(0)).await.unwrap();
    assert!(
        rig.backend
            .rendition_content_present(root_id.get(), &proxy.file_id)
            .await
            .unwrap(),
        "a live source's rendition survives GC"
    );
    // And it's still served from cache (same content id).
    let again = rig
        .media
        .rendition_info(root_id, p("cut.mov"), RenditionKind::Proxy720, None)
        .await
        .unwrap();
    assert_eq!(proxy.file_id, again.file_id);

    rig.backend.shutdown().await;
}
