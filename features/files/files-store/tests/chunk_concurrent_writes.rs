//! Regression coverage for two defects a code review caught in the first
//! pass at `ChunkStore`:
//!
//! - `write_manifest`'s temp file was named only from the `FileId`, so two
//!   concurrent `write_stream` calls for identical content raced on one
//!   `.manifest.tmp` path — the loser's `rename` failed with ENOENT
//!   because the winner's rename had already consumed the shared path.
//! - the chunking loop was routed through an `impl AsyncFnMut` combinator
//!   whose captured-reference future the compiler couldn't prove `Send`
//!   for every lifetime, so `write_stream` failed to compile inside
//!   `tokio::spawn` at all.
//!
//! This exercises exactly that shape: many concurrent, spawned writers of
//! byte-identical content sharing one store.

mod common;

use std::sync::Arc;

use files_store::chunk::ChunkStore;

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_writes_of_identical_content_do_not_race() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(ChunkStore::open(dir.path()).await.unwrap());
    // Large enough to span several chunks, so the manifest write isn't
    // trivially fast relative to the race window.
    let content: Arc<Vec<u8>> = Arc::new((0..5 * 1024 * 1024).map(|i| (i % 251) as u8).collect());

    let mut tasks = Vec::new();
    for _ in 0..16 {
        let store = store.clone();
        let content = content.clone();
        tasks.push(tokio::spawn(async move {
            store.write_stream(&content[..]).await
        }));
    }

    let mut ids = Vec::new();
    for task in tasks {
        ids.push(
            task.await
                .expect("task panicked")
                .expect("write_stream failed"),
        );
    }

    assert!(
        ids.iter().all(|id| *id == ids[0]),
        "identical content must always produce the same FileId"
    );

    let bytes = store.read_to_vec(ids[0]).await.unwrap();
    assert_eq!(&bytes, content.as_ref());

    store.shutdown().await.unwrap();
}
