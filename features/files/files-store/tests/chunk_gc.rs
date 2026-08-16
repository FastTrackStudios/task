//! Chunk-level GC (issue #258): `ChunkStore::gc`'s manifest mark/sweep is
//! synchronous, but actual chunk-blob reclamation happens on iroh-blobs'
//! own background schedule (see `src/gc.rs`'s module doc for why that's the
//! only deletion path 0.103 exposes publicly) — so tests that need to
//! observe reclamation poll with a bounded timeout against a short GC
//! interval, rather than asserting immediately after `gc` returns.

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime};

use files_store::chunk::{ChunkStore, ChunkerConfig, Error, GcConfig};

const POLL_INTERVAL: Duration = Duration::from_millis(20);
const POLL_TIMEOUT: Duration = Duration::from_secs(5);

async fn open_gc_store(dir: &std::path::Path) -> ChunkStore {
    ChunkStore::open_with_gc(
        dir,
        ChunkerConfig::default(),
        GcConfig {
            interval: Duration::from_millis(30),
        },
    )
    .await
    .unwrap()
}

/// Poll `chunk_count` until it drops below `before`, or time out. Returns
/// the final count either way so callers can assert on it.
async fn wait_for_chunk_count_below(store: &ChunkStore, before: usize) -> usize {
    let deadline = tokio::time::Instant::now() + POLL_TIMEOUT;
    loop {
        let count = store.chunk_count().await.unwrap();
        if count < before || tokio::time::Instant::now() >= deadline {
            return count;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// "Unreferenced expired snapshots are swept; their unshared chunks are
/// reclaimed."
#[tokio::test]
async fn unreferenced_expired_manifest_is_swept_and_its_chunk_reclaimed() {
    let dir = tempfile::tempdir().unwrap();
    let store = open_gc_store(dir.path()).await;

    let file_id = store
        .write_stream(&b"a file nobody keeps"[..])
        .await
        .unwrap();
    let before = store.chunk_count().await.unwrap();
    assert!(
        before > 0,
        "the write should have stored at least one chunk"
    );

    // Let the manifest age past `keep_newer`.
    tokio::time::sleep(Duration::from_millis(10)).await;
    let keep_newer = SystemTime::now();

    let stats = store.gc(&BTreeSet::new(), keep_newer).await.unwrap();
    assert_eq!(stats.manifests_swept, 1);

    // The manifest is gone immediately (gc's mark phase is synchronous).
    assert!(!store.has(file_id).await);

    // The chunk itself is reclaimed on iroh-blobs' own schedule.
    let after = wait_for_chunk_count_below(&store, before).await;
    assert!(
        after < before,
        "expected the unshared chunk to be reclaimed: before={before} after={after}"
    );
}

/// "Protect-callback-listed versions survive GC regardless of age."
#[tokio::test]
async fn protected_manifest_survives_gc_regardless_of_age() {
    let dir = tempfile::tempdir().unwrap();
    let store = open_gc_store(dir.path()).await;

    let file_id = store
        .write_stream(&b"a version the Vault still points at"[..])
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(10)).await;
    let keep_newer = SystemTime::now();

    let mut protected = BTreeSet::new();
    protected.insert(file_id);
    let stats = store.gc(&protected, keep_newer).await.unwrap();
    assert_eq!(stats.manifests_swept, 0);

    assert!(store.has(file_id).await);
    assert_eq!(
        store.read_to_vec(file_id).await.unwrap(),
        b"a version the Vault still points at"
    );
}

/// "keep_newer guards concurrent writers (nothing newer is swept)."
#[tokio::test]
async fn keep_newer_protects_a_manifest_written_after_the_cutoff() {
    let dir = tempfile::tempdir().unwrap();
    let store = open_gc_store(dir.path()).await;

    let keep_newer = SystemTime::now();
    tokio::time::sleep(Duration::from_millis(10)).await;
    // Written *after* keep_newer, and never protected: only its mtime saves it.
    let file_id = store
        .write_stream(&b"written after the gc cutoff"[..])
        .await
        .unwrap();

    let stats = store.gc(&BTreeSet::new(), keep_newer).await.unwrap();
    assert_eq!(
        stats.manifests_swept, 0,
        "a manifest newer than keep_newer must never be swept"
    );
    assert!(store.has(file_id).await);
}

/// Regression test (data-loss finding): the protect callback used to read a
/// snapshot published once per `ChunkStore::gc` call, while iroh-blobs'
/// background sweep runs on its own independent interval — so a manifest
/// written *after* the last `gc()` call (and therefore absent from that
/// stale snapshot) would be reclaimed by the very next background tick,
/// before anything ever called `gc()` again. The callback now derives
/// liveness live from the manifests directory on every sweep, so a file
/// written after the *only* `gc()` call this test ever makes must still
/// survive many background sweep intervals with no further `gc()` call.
#[tokio::test]
async fn a_file_written_after_the_only_gc_call_survives_later_background_sweeps() {
    let dir = tempfile::tempdir().unwrap();
    let store = open_gc_store(dir.path()).await;

    // One gc() call, before the file in question even exists — the stale
    // snapshot this regression guards against would have no idea it will
    // ever exist.
    store.gc(&BTreeSet::new(), SystemTime::now()).await.unwrap();

    let file_id = store
        .write_stream(&b"written after the only gc() call"[..])
        .await
        .unwrap();
    assert!(store.has(file_id).await);

    // Give iroh-blobs' background task (30ms interval) several chances to
    // run without ever calling gc() again.
    tokio::time::sleep(POLL_INTERVAL * 10).await;

    assert!(
        store.has(file_id).await,
        "a file written after the only gc() call must survive later background sweeps \
         even though gc() was never called again"
    );
    assert_eq!(
        store.read_to_vec(file_id).await.unwrap(),
        b"written after the only gc() call"
    );
}

/// Regression test: a corrupt *kept* manifest (protected or fresh) used to
/// abort `gc()` via `?` on a failed decode read, wedging every other
/// manifest's removal decision along with it — and the same corrupt file
/// would re-trigger the error on every later pass, since its mtime never
/// changes. `gc()` no longer decodes kept manifests at all (only their
/// `FileId`/mtime), so it can no longer be wedged this way.
#[tokio::test]
async fn a_corrupt_kept_manifest_does_not_wedge_gc() {
    let dir = tempfile::tempdir().unwrap();
    let store = open_gc_store(dir.path()).await;

    let protected_id = store
        .write_stream(&b"protected but corrupt on disk"[..])
        .await
        .unwrap();
    let expired_id = store
        .write_stream(&b"an ordinary expired file"[..])
        .await
        .unwrap();

    // Corrupt the protected manifest's bytes directly, as if it survived a
    // torn write (the exact scenario `write_manifest`'s own doc describes).
    // `manifests/<file id hex>.manifest` is this crate's documented,
    // stable on-disk layout.
    let manifest_path = dir
        .path()
        .join("manifests")
        .join(format!("{}.manifest", protected_id.to_hex()));
    tokio::fs::write(&manifest_path, b"not a valid manifest")
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(10)).await;
    let keep_newer = SystemTime::now();

    let mut protected = BTreeSet::new();
    protected.insert(protected_id);

    let stats = store.gc(&protected, keep_newer).await.unwrap();
    assert_eq!(
        stats.manifests_swept, 1,
        "the ordinary expired manifest must still be swept despite the corrupt protected one"
    );
    assert!(!store.has(expired_id).await);
}

/// `ChunkStore::gc` on a store opened without GC enabled has nothing that
/// will ever reclaim the chunks a manifest removal would orphan, so it
/// refuses rather than silently leaking or lying about what it did.
#[tokio::test]
async fn gc_is_disabled_without_open_with_gc() {
    let dir = tempfile::tempdir().unwrap();
    let store = ChunkStore::open(dir.path()).await.unwrap();

    let err = store
        .gc(&BTreeSet::new(), SystemTime::now())
        .await
        .unwrap_err();
    assert!(
        matches!(err, Error::GcDisabled),
        "expected GcDisabled, got {err:?}"
    );
}
