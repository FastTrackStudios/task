//! Acceptance criterion: "Write/read a multi-GB stream without whole-file
//! buffering (bounded memory)."
//!
//! We drive a 2 GiB source through `write_stream` and read it back through
//! `read_to`, and check process RSS growth stays a small fraction of the
//! file size — proof that the store is never materializing the whole file
//! in memory, only chunk-sized windows of it (bounded by
//! `ChunkerConfig::max_size`). Linux-only (reads `/proc/self/statm`); on
//! other OSes this still exercises the streaming code paths, just without
//! the memory assertion.

mod common;

use files_store::chunk::{ChunkStore, ChunkerConfig};

const TWO_GIB: u64 = 2 * 1024 * 1024 * 1024;

/// Current resident set size, in bytes, via `/proc/self/statm` (field 2,
/// in pages). Linux-only.
#[cfg(target_os = "linux")]
fn rss_bytes() -> u64 {
    let statm = std::fs::read_to_string("/proc/self/statm").expect("read /proc/self/statm");
    let pages: u64 = statm
        .split_whitespace()
        .nth(1)
        .expect("statm has a resident-pages field")
        .parse()
        .expect("resident-pages field is a number");
    pages * 4096 // page size on every Linux arch we run on
}

#[tokio::test]
#[ignore = "streams 2 GiB through tempfile::tempdir() (tmpfs = real RAM on typical Linux); opt in with FILES_CHUNK_STORE_STRESS=1 cargo test -- --ignored"]
async fn writes_and_reads_a_multi_gb_stream_with_bounded_memory() {
    if !common::stress_tests_enabled() {
        eprintln!(
            "skipping: set {} to run this stress test",
            common::STRESS_ENV_VAR
        );
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    // Larger-than-default chunks (4 MiB avg / 16 MiB max) keep the chunk
    // count — and so the number of blob-store round trips — reasonable
    // for a 2 GiB source in a test run, while keeping the per-chunk bound
    // far below the file size.
    let config = ChunkerConfig::with_avg_size(4 * 1024 * 1024);
    let store = ChunkStore::open_with_config(dir.path(), config)
        .await
        .unwrap();

    let source = common::DeterministicSource::new(1, TWO_GIB);

    #[cfg(target_os = "linux")]
    let rss_before_write = rss_bytes();

    let file_id = store.write_stream(source).await.unwrap();

    #[cfg(target_os = "linux")]
    {
        let grew_by = rss_bytes().saturating_sub(rss_before_write);
        assert!(
            grew_by < 512 * 1024 * 1024,
            "write_stream grew RSS by {grew_by} bytes writing a {TWO_GIB}-byte source — looks like whole-file buffering"
        );
    }

    let manifest = store.manifest(file_id).await.unwrap();
    assert_eq!(manifest.total_len(), TWO_GIB);
    assert!(
        manifest.chunks.len() > 10,
        "expected a 2 GiB source to produce many chunks, got {}",
        manifest.chunks.len()
    );

    #[cfg(target_os = "linux")]
    let rss_before_read = rss_bytes();

    let mut sink = tokio::io::sink();
    store.read_to(file_id, &mut sink).await.unwrap();

    #[cfg(target_os = "linux")]
    {
        let grew_by = rss_bytes().saturating_sub(rss_before_read);
        assert!(
            grew_by < 512 * 1024 * 1024,
            "read_to grew RSS by {grew_by} bytes reading a {TWO_GIB}-byte file — looks like whole-file buffering"
        );
    }

    store.shutdown().await.unwrap();
}
