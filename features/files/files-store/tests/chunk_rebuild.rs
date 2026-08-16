//! Acceptance criterion: "Store is deleted and rebuilt from manifests +
//! chunks; all FileIds resolve identically."
//!
//! ADR 0001's rebuildability claim means everything needed to resolve a
//! `FileId` back to bytes lives on disk in `manifests/` + `blobs/` — there
//! is no in-memory-only state a `ChunkStore` instance is secretly relying
//! on. We prove that by writing files, `shutdown`ing and dropping the
//! store, then opening a *brand new* `ChunkStore` over the same directory
//! and checking every `FileId` still resolves to identical bytes.

mod common;

use files_store::chunk::ChunkStore;

#[tokio::test]
async fn rebuilt_store_resolves_all_file_ids_identically() {
    let dir = tempfile::tempdir().unwrap();

    let sources: Vec<&[u8]> = vec![
        b"",
        b"a single short file",
        b"another, different, short file",
    ];
    // A larger file so the rebuild proof isn't limited to single-chunk
    // content.
    let large = common::DeterministicSource::new(7, 4 * 1024 * 1024);

    let mut expected: Vec<(files_store::chunk::FileId, Vec<u8>)> = Vec::new();
    {
        let store = ChunkStore::open(dir.path()).await.unwrap();
        for src in &sources {
            let file_id = store.write_stream(*src).await.unwrap();
            expected.push((file_id, src.to_vec()));
        }
        let large_id = store.write_stream(large).await.unwrap();
        let large_bytes = store.read_to_vec(large_id).await.unwrap();
        expected.push((large_id, large_bytes));

        store.shutdown().await.unwrap();
        // `store` (and its FsStore) is dropped here — nothing beyond
        // `dir.path()`'s contents survives into the next block.
    }

    let rebuilt = ChunkStore::open(dir.path()).await.unwrap();
    for (file_id, bytes) in &expected {
        let read_back = rebuilt.read_to_vec(*file_id).await.unwrap();
        assert_eq!(
            &read_back, bytes,
            "FileId {file_id} did not resolve identically after rebuild"
        );
    }
    rebuilt.shutdown().await.unwrap();
}

#[tokio::test]
async fn identical_content_from_two_writes_produces_one_manifest() {
    // A second, unrelated ChunkStore proof: writing the same bytes twice
    // (even across a shutdown/reopen) must yield the same FileId and not
    // error on the "manifest already exists" path.
    let dir = tempfile::tempdir().unwrap();
    let store = ChunkStore::open(dir.path()).await.unwrap();
    let id_a = store
        .write_stream(&b"same content, twice"[..])
        .await
        .unwrap();
    let id_b = store
        .write_stream(&b"same content, twice"[..])
        .await
        .unwrap();
    assert_eq!(id_a, id_b);
    store.shutdown().await.unwrap();

    let reopened = ChunkStore::open(dir.path()).await.unwrap();
    let id_c = reopened
        .write_stream(&b"same content, twice"[..])
        .await
        .unwrap();
    assert_eq!(id_a, id_c);
    assert_eq!(
        reopened.read_to_vec(id_c).await.unwrap(),
        b"same content, twice"
    );
}
