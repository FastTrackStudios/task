//! The blob-store thread count does not grow with the number of stores.
//!
//! iroh-blobs' own `FsStore::load_with_opts` builds a runtime of `cores`
//! threads (`iroh-blob-store-N`) per store, so a server with one chunk
//! store per root ran `roots × cores` threads. Every store is now opened
//! on one shared runtime (`files_store::chunk::runtime`), bounded by
//! `TASK_BLOB_STORE_THREADS`. This opens ten stores — with and without
//! GC, each written to and read back — and counts the process's threads
//! by name.
//!
//! One test in its own file, so its process holds no other test's
//! threads.

#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::time::Duration;

use files_store::chunk::runtime::{THREAD_NAME, max_threads};
use files_store::chunk::{ChunkStore, ChunkerConfig, GcConfig};

/// Thread names of this process, from `/proc/self/task/*/comm`.
fn thread_names() -> Vec<String> {
    std::fs::read_dir("/proc/self/task")
        .unwrap()
        .filter_map(|entry| std::fs::read_to_string(entry.ok()?.path().join("comm")).ok())
        .map(|name| name.trim_end().to_owned())
        .collect()
}

fn census() -> BTreeMap<String, usize> {
    let mut by_name = BTreeMap::new();
    for name in thread_names() {
        *by_name.entry(name).or_default() += 1;
    }
    by_name
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ten_stores_share_one_bounded_runtime() {
    const STORES: usize = 10;
    let before = census();

    let dirs: Vec<_> = (0..STORES).map(|_| tempfile::tempdir().unwrap()).collect();
    let mut stores = Vec::new();
    for (i, dir) in dirs.iter().enumerate() {
        let store = if i % 2 == 0 {
            ChunkStore::open(dir.path()).await.unwrap()
        } else {
            ChunkStore::open_with_gc(
                dir.path(),
                ChunkerConfig::default(),
                GcConfig {
                    interval: Duration::from_millis(50),
                },
            )
            .await
            .unwrap()
        };
        // Exercise the store so its actors, meta db and any blocking
        // work have actually run on the runtime.
        let body = format!("store {i} says hello").into_bytes();
        let id = store.write_stream(&body[..]).await.unwrap();
        assert_eq!(store.read_to_vec(id).await.unwrap(), body);
        stores.push(store);
    }
    // Let the GC loops run a pass or two.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let after = census();
    let blob_threads = after.get(THREAD_NAME).copied().unwrap_or(0);
    let per_store_threads: usize = after
        .iter()
        .filter(|(name, _)| name.starts_with("iroh-blob-stor"))
        .map(|(_, n)| n)
        .sum();
    let detail = format!("{STORES} stores; threads by name {before:?} -> {after:?}");

    assert!(blob_threads >= 1, "no shared blob-store thread: {detail}");
    assert!(
        blob_threads <= max_threads(),
        "{blob_threads} {THREAD_NAME} threads > bound {}: {detail}",
        max_threads()
    );
    assert_eq!(
        per_store_threads, 0,
        "a store built its own runtime: {detail}"
    );
    // Nothing else grew either, apart from this test's own runtime
    // (`tokio-rt-worker`, whose blocking pool serves the stores' callers).
    for (name, n) in &after {
        if name == THREAD_NAME || name == "tokio-rt-worker" {
            continue;
        }
        let was = before.get(name).copied().unwrap_or(0);
        assert!(*n <= was, "{n} `{name}` threads, {was} before: {detail}");
    }
    eprintln!("{detail}");

    for store in &stores {
        store.shutdown().await.unwrap();
    }
}
