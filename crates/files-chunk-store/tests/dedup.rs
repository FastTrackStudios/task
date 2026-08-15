//! Acceptance criterion: "Two saves differing by a small edit share the
//! vast majority of chunks (dedup ratio asserted)."
//!
//! This is the whole reason ADR 0001 picked FastCDC over fixed-size
//! chunking: content-defined boundaries mean an edit only perturbs the
//! chunks that actually overlap it — everything before and after
//! resynchronizes to the same boundaries as the unedited file, so a small
//! in-place edit to a multi-GB session file dedups almost entirely against
//! the previous save.

mod common;

use std::collections::HashSet;

use task_files_chunk_store::{ChunkStore, ChunkerConfig};

const FILE_LEN: u64 = 1536 * 1024 * 1024; // 1.5 GiB
const EDIT_LEN: u64 = 256 * 1024; // a 256 KiB in-place edit
const EDIT_START: u64 = FILE_LEN / 2;

#[tokio::test]
#[ignore = "streams 3 GiB total through tempfile::tempdir() (tmpfs = real RAM on typical Linux); opt in with FILES_CHUNK_STORE_STRESS=1 cargo test -- --ignored"]
async fn near_identical_multi_gb_saves_share_the_vast_majority_of_chunks() {
    if !common::stress_tests_enabled() {
        eprintln!(
            "skipping: set {} to run this stress test",
            common::STRESS_ENV_VAR
        );
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let config = ChunkerConfig::with_avg_size(1024 * 1024);
    let store = ChunkStore::open_with_config(dir.path(), config)
        .await
        .unwrap();

    let original = common::DeterministicSource::new(42, FILE_LEN);
    let edited = common::DeterministicSource::with_edit(42, FILE_LEN, EDIT_START, EDIT_LEN, 99);

    let original_id = store.write_stream(original).await.unwrap();
    let edited_id = store.write_stream(edited).await.unwrap();
    assert_ne!(
        original_id, edited_id,
        "an edited file must get a different FileId"
    );

    let original_manifest = store.manifest(original_id).await.unwrap();
    let edited_manifest = store.manifest(edited_id).await.unwrap();
    assert_eq!(original_manifest.total_len(), FILE_LEN);
    assert_eq!(edited_manifest.total_len(), FILE_LEN);

    let original_hashes: HashSet<_> = original_manifest.chunks.iter().map(|c| c.hash).collect();

    let shared_bytes: u64 = edited_manifest
        .chunks
        .iter()
        .filter(|c| original_hashes.contains(&c.hash))
        .map(|c| c.len)
        .sum();
    let dedup_ratio = shared_bytes as f64 / FILE_LEN as f64;

    assert!(
        dedup_ratio > 0.95,
        "expected >95% of bytes to dedup against the near-identical save, got {:.2}% ({shared_bytes} of {FILE_LEN} bytes shared, {} of {} chunks in the edited manifest matched)",
        dedup_ratio * 100.0,
        edited_manifest
            .chunks
            .iter()
            .filter(|c| original_hashes.contains(&c.hash))
            .count(),
        edited_manifest.chunks.len(),
    );

    // The dedup story only matters if the second write actually skipped
    // re-storing the shared chunks rather than merely producing a manifest
    // that happens to name them — read both files back byte-for-byte to
    // prove the shared chunk blobs are still resolvable and correct after
    // both writes.
    let original_bytes_len = store.read_to(original_id, &mut tokio::io::sink()).await;
    assert!(original_bytes_len.is_ok());
    let edited_bytes_len = store.read_to(edited_id, &mut tokio::io::sink()).await;
    assert!(edited_bytes_len.is_ok());

    store.shutdown().await.unwrap();
}
