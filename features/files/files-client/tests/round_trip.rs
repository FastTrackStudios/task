//! An app's whole loop through `FilesClient`, over vox: make a store,
//! save, read back whole and by range, lose a race safely, and follow a
//! pinned reference after its path was overwritten.

use architect::{LayerRouter, LocalServer, Scope};
use files::FilesBackend;
use files_client::{FilesClient, Resolved, Save, root_id};
use files_proto::model::RootFlavor;

async fn client() -> (tempfile::TempDir, LocalServer, FilesClient) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let backend = FilesBackend::new(tmp.path(), tmp.path().join("vault")).expect("backend");
    let local = LocalServer::serve(
        LayerRouter::new()
            .merge(files_proto::roots_layer(backend.clone()))
            .merge(files_proto::tree_layer(backend.clone()))
            .merge(files_proto::upload_layer(backend.clone()))
            .merge(files_proto::media_layer(backend.clone()))
            .merge(files_proto::media_stream_layer(backend)),
        Scope::new(),
    );
    let files = FilesClient::new(
        local.establish().await.expect("roots"),
        local.establish().await.expect("tree"),
        local.establish().await.expect("uploads"),
        local.establish().await.expect("media"),
        local.establish().await.expect("bytes"),
    );
    (tmp, local, files)
}

#[tokio::test(flavor = "multi_thread")]
async fn an_app_saves_reads_and_follows_a_pin() {
    let (_tmp, _local, files) = client().await;
    let store = files
        .ensure_root(
            "signal/impulse-responses",
            "Impulse responses",
            RootFlavor::Media,
        )
        .await
        .expect("ensure_root");
    let root = root_id(&store);

    // Larger than one frame, so the save really is chunked.
    let v1: Vec<u8> = (0..700_000u32).map(|i| (i % 251) as u8).collect();
    let saved = files
        .put(root, "Cab/4x12.wav", &v1, Save::create_only())
        .await
        .expect("create-only save");
    let pin = saved.content.clone().expect("the save reports its etag");

    assert_eq!(files.get(root, "Cab/4x12.wav").await.expect("get"), v1);
    assert_eq!(
        files
            .read_range(root, "Cab/4x12.wav", 1_000, 1_009)
            .await
            .expect("range"),
        v1[1_000..=1_009].to_vec(),
        "a range is a seek, byte-exact"
    );

    // Someone else replaces it; a save against the old etag is refused.
    let v2: Vec<u8> = v1.iter().map(|b| b.wrapping_add(1)).collect();
    files
        .put(root, "Cab/4x12.wav", &v2, Save::replacing(pin.clone()))
        .await
        .expect("a save against the current etag lands");
    let stale = files
        .put(root, "Cab/4x12.wav", b"late", Save::replacing(pin.clone()))
        .await
        .expect_err("the old etag no longer holds");
    assert!(stale.is_stale(), "refused as stale: {stale}");

    // The manifest pinned v1. Resolving says the path moved on; fetching
    // the reference still yields the recording it named.
    match files
        .resolve(root, "Cab/4x12.wav", Some(pin.clone()))
        .await
        .expect("resolve")
    {
        Resolved::Moved { pinned, .. } => assert_eq!(pinned, pin),
        other => panic!("expected Moved, got {other:?}"),
    }
    assert_eq!(
        files
            .get_ref(root, "Cab/4x12.wav", Some(pin))
            .await
            .expect("get_ref"),
        v1,
        "a pinned reference means those bytes, not whatever is at the path now"
    );
}
