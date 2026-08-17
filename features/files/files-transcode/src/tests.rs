//! Pipeline + store behaviour (issue #269) against the deterministic
//! [`FakeTranscoder`] — every acceptance criterion without ffmpeg or
//! real media. The RPC seam (request a rendition over the Files
//! surface, warm a checkpoint) is tested in the `files` crate; this
//! proves the engine those calls drive.

use std::sync::Arc;

use files_store::chunk::ChunkStore;

use crate::pipeline::TranscodePipeline;
use crate::recipe::{RECIPE_VERSION, RenditionKind};
use crate::store::{RenditionKey, RenditionStore};
use crate::transcoder::FakeTranscoder;

async fn pipeline(dir: &std::path::Path) -> (TranscodePipeline, Arc<ChunkStore>) {
    let chunks = Arc::new(ChunkStore::open(dir.join("chunks")).await.unwrap());
    let store = Arc::new(RenditionStore::open(dir.join("renditions")).await.unwrap());
    let pipe = TranscodePipeline::new(chunks.clone(), store, Arc::new(FakeTranscoder));
    (pipe, chunks)
}

/// Store a source (with a media-class magic prefix) and return its id.
async fn put_source(chunks: &ChunkStore, prefix: &[u8], body: &[u8]) -> files_store::chunk::FileId {
    let mut bytes = prefix.to_vec();
    bytes.extend_from_slice(body);
    chunks.write_stream(&bytes[..]).await.unwrap()
}

/// AC 1: a video yields the ladder + filmstrip; audio yields peaks.
#[tokio::test]
async fn warm_up_builds_the_class_ladder() {
    let dir = tempfile::tempdir().unwrap();
    let (pipe, chunks) = pipeline(dir.path()).await;

    let video = put_source(&chunks, b"VIDEO", b"frames...").await;
    let mut kinds = pipe.warm_up(&video).await.unwrap();
    kinds.sort_by_key(|k| k.tag());
    assert_eq!(
        kinds,
        {
            let mut l = RenditionKind::ladder_for(crate::MediaClass::Video).to_vec();
            l.sort_by_key(|k| k.tag());
            l
        },
        "video ladder incl. filmstrip"
    );
    assert!(kinds.contains(&RenditionKind::Filmstrip));

    let audio = put_source(&chunks, b"AUDIO", b"samples...").await;
    let kinds = pipe.warm_up(&audio).await.unwrap();
    assert_eq!(kinds, vec![RenditionKind::Audio, RenditionKind::Peaks]);

    let doc = put_source(&chunks, b"TEXT", b"not media").await;
    assert!(
        pipe.warm_up(&doc).await.unwrap().is_empty(),
        "non-media: no ladder"
    );
}

/// AC 2: a missing rendition generates on demand once and caches it.
#[tokio::test]
async fn rendition_generates_once_then_caches() {
    let dir = tempfile::tempdir().unwrap();
    let (pipe, chunks) = pipeline(dir.path()).await;
    let video = put_source(&chunks, b"VIDEO", b"frames").await;

    let first = pipe
        .rendition(&video, RenditionKind::Proxy720)
        .await
        .unwrap();
    // Same bytes on a second request (cache hit): same CAS id.
    let second = pipe
        .rendition(&video, RenditionKind::Proxy720)
        .await
        .unwrap();
    assert_eq!(first.file_id, second.file_id);

    // The cache entry exists on disk now.
    let key = RenditionKey {
        source_file_id: &video.to_hex(),
        recipe_version: RECIPE_VERSION,
        kind: RenditionKind::Proxy720,
    };
    assert!(pipe.store().get(&key).await.unwrap().is_some());

    // A kind that doesn't apply to the media class is refused.
    let audio = put_source(&chunks, b"AUDIO", b"samples").await;
    let err = pipe
        .rendition(&audio, RenditionKind::Filmstrip)
        .await
        .unwrap_err();
    assert!(matches!(err, crate::Error::NotMedia(_)), "{err}");
}

/// AC 3: GC sweeps a dead source's renditions; a protected source keeps
/// its renditions.
#[tokio::test]
async fn gc_is_tied_to_the_source() {
    let dir = tempfile::tempdir().unwrap();
    let (pipe, chunks) = pipeline(dir.path()).await;
    let kept = put_source(&chunks, b"VIDEO", b"kept").await;
    let dead = put_source(&chunks, b"VIDEO", b"dead").await;
    pipe.warm_up(&kept).await.unwrap();
    pipe.warm_up(&dead).await.unwrap();

    // GC: only `kept` is live.
    let kept_hex = kept.to_hex();
    let removed = pipe.store().gc(|src| src == kept_hex).await.unwrap();
    assert_eq!(
        removed,
        RenditionKind::ladder_for(crate::MediaClass::Video).len() as u64
    );

    // kept's renditions survive; dead's are gone.
    for &kind in RenditionKind::ladder_for(crate::MediaClass::Video) {
        let key_kept = RenditionKey {
            source_file_id: &kept.to_hex(),
            recipe_version: RECIPE_VERSION,
            kind,
        };
        let key_dead = RenditionKey {
            source_file_id: &dead.to_hex(),
            recipe_version: RECIPE_VERSION,
            kind,
        };
        assert!(pipe.store().get(&key_kept).await.unwrap().is_some());
        assert!(pipe.store().get(&key_dead).await.unwrap().is_none());
    }
}

/// AC 4: a recipe-version bump regenerates without orphaning old
/// renditions — the GC sweeps superseded recipes of a still-live
/// source.
#[tokio::test]
async fn recipe_bump_does_not_orphan_old_renditions() {
    let dir = tempfile::tempdir().unwrap();
    let (pipe, chunks) = pipeline(dir.path()).await;
    let video = put_source(&chunks, b"VIDEO", b"frames").await;

    // Simulate a SUPERSEDED recipe's rendition by writing under a
    // different recipe version directly (RECIPE_VERSION + 1 stands in
    // for "not the current one" without underflowing at version 1).
    let old_key = RenditionKey {
        source_file_id: &video.to_hex(),
        recipe_version: RECIPE_VERSION + 1,
        kind: RenditionKind::Proxy720,
    };
    pipe.store()
        .put(&old_key, b"old-recipe-bytes")
        .await
        .unwrap();
    // And a current-recipe rendition.
    pipe.rendition(&video, RenditionKind::Proxy720)
        .await
        .unwrap();

    // GC with the source live: the old-recipe entry is swept (not
    // orphaned), the current one kept.
    let hex = video.to_hex();
    let removed = pipe.store().gc(|src| src == hex).await.unwrap();
    assert_eq!(removed, 1, "only the superseded-recipe entry");
    assert!(pipe.store().get(&old_key).await.unwrap().is_none());
    let cur_key = RenditionKey {
        source_file_id: &video.to_hex(),
        recipe_version: RECIPE_VERSION,
        kind: RenditionKind::Proxy720,
    };
    assert!(pipe.store().get(&cur_key).await.unwrap().is_some());
}
