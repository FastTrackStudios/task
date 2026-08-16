//! Content on the same filesystem is stored WHOLE and *linked* in
//! rather than copied — reflink where extents can be cloned, hardlink
//! otherwise. See `ChunkStore::write_path`.
//!
//! Which rung of that ladder a given filesystem offers is not
//! something a test can assume, so these assert the properties that
//! hold either way:
//!
//! - **the content cannot be lost by deleting the original** — the
//!   reason a link is acceptable instead of a copy at all;
//! - **linking consumes no space**, at any size — what makes importing
//!   a multi-terabyte archive possible;
//! - probe and write agree on whole-vs-chunked, the invariant that
//!   silently ruins everything if it breaks (every capture would
//!   re-import every file, forever);
//! - honest round-trips, including range reads over a blob that is an
//!   entire file, and GC reclaiming the tier.
//!
//! These run with a small forced threshold so both paths are exercised;
//! in production it is 0, meaning everything links.

use std::path::Path;

use files_store::chunk::{ChunkStore, ChunkerConfig};

const SMALL_THRESHOLD: u64 = 1024 * 1024;

async fn store_at(root: &Path) -> ChunkStore {
    ChunkStore::open_with_config(
        root,
        ChunkerConfig::default().with_whole_file_threshold(SMALL_THRESHOLD),
    )
    .await
    .unwrap()
}

/// Varied enough to produce many chunk boundaries when chunked, so the
/// whole-file path is genuinely doing something different.
fn content(len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| ((i as u32).wrapping_mul(2_654_435_761) >> 13) as u8)
        .collect()
}

/// Genuinely incompressible bytes. The free-space measurement below is
/// meaningless on a transparently-compressing filesystem — this repo's
/// own `/tmp` is btrfs with `compress=zstd` — unless the content resists
/// compression the way real media does.
fn incompressible(len: usize) -> Vec<u8> {
    use std::io::Read as _;
    let mut buf = vec![0u8; len];
    std::fs::File::open("/dev/urandom")
        .expect("/dev/urandom")
        .read_exact(&mut buf)
        .expect("reading random bytes");
    buf
}

#[tokio::test]
async fn a_large_file_is_stored_as_one_blob_and_reads_back_whole() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_at(dir.path()).await;
    let bytes = content(4 * 1024 * 1024);
    let path = dir.path().join("big.wav");
    tokio::fs::write(&path, &bytes).await.unwrap();

    let file_id = store.write_path(&path).await.unwrap();

    let manifest = store.manifest(file_id).await.unwrap();
    assert_eq!(
        manifest.chunks.len(),
        1,
        "a file above the threshold must be one whole blob, not chunked"
    );
    assert_eq!(manifest.chunks[0].len, bytes.len() as u64);
    assert_eq!(
        manifest.chunks[0].hash,
        blake3::hash(&bytes),
        "the single entry's hash must be the blake3 of the whole file"
    );
    assert_eq!(store.read_to_vec(file_id).await.unwrap(), bytes);
    assert_eq!(
        store.content_len(file_id).await.unwrap(),
        bytes.len() as u64
    );
}

#[tokio::test]
async fn a_small_file_still_chunks_and_matches_the_streaming_write() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_at(dir.path()).await;
    let bytes = content(200 * 1024); // under SMALL_THRESHOLD
    let path = dir.path().join("small.txt");
    tokio::fs::write(&path, &bytes).await.unwrap();

    let by_path = store.write_path(&path).await.unwrap();
    let by_stream = store.write_stream(&bytes[..]).await.unwrap();
    assert_eq!(
        by_path, by_stream,
        "below the threshold, write_path is write_stream — same id"
    );
    assert_eq!(store.read_to_vec(by_path).await.unwrap(), bytes);
}

/// The invariant that quietly destroys everything if it breaks: a
/// whole-stored file and a chunked one have *different* ids for the same
/// bytes, so if `probe_path` made a different size decision than
/// `write_path`, every capture would see every large file as changed and
/// re-import the whole tree — forever, on a tree that never changed.
#[tokio::test]
async fn probe_agrees_with_write_on_both_sides_of_the_threshold() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_at(dir.path()).await;

    for (name, len) in [
        ("under.bin", (SMALL_THRESHOLD - 1) as usize),
        ("exactly.bin", SMALL_THRESHOLD as usize),
        ("over.bin", (SMALL_THRESHOLD * 3) as usize),
    ] {
        let path = dir.path().join(name);
        tokio::fs::write(&path, content(len)).await.unwrap();

        let probed = store.probe_path(&path).await.unwrap();
        let written = store.write_path(&path).await.unwrap();
        assert_eq!(
            probed, written,
            "{name}: probe_path must predict write_path"
        );
    }
}

/// A whole-file blob can be the entire multi-hundred-GB file, so serving
/// a `<video>` seek must read only the window — and, being one "chunk",
/// it exercises a code path the chunked case never reaches.
#[tokio::test]
async fn range_reads_work_over_a_whole_file_blob() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_at(dir.path()).await;
    let bytes = content(3 * 1024 * 1024);
    let path = dir.path().join("seekable.mov");
    tokio::fs::write(&path, &bytes).await.unwrap();
    let file_id = store.write_path(&path).await.unwrap();

    let mut full = Vec::new();
    store
        .read_range(file_id, 0, bytes.len() as u64, &mut full)
        .await
        .unwrap();
    assert_eq!(full, bytes, "full range == whole file");

    for (start, len) in [
        (0u64, 10u64),
        (1_000_000, 65_536),
        (bytes.len() as u64 - 5, 50), // straddles the end: clamped
    ] {
        let mut got = Vec::new();
        store
            .read_range(file_id, start, len, &mut got)
            .await
            .unwrap();
        let end = (start + len).min(bytes.len() as u64) as usize;
        assert_eq!(got, bytes[start as usize..end], "window {start}+{len}");
    }
}

/// Re-storing an unchanged file must be a no-op, not a second clone —
/// this is the property the whole change exists to buy (a nightly
/// snapshot of a tree nobody touched should cost nothing).
#[tokio::test]
async fn rewriting_an_unchanged_large_file_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_at(dir.path()).await;
    let path = dir.path().join("stable.braw");
    tokio::fs::write(&path, content(2 * 1024 * 1024))
        .await
        .unwrap();

    let first = store.write_path(&path).await.unwrap();
    let before = store.chunk_count().await.unwrap();
    let second = store.write_path(&path).await.unwrap();
    let after = store.chunk_count().await.unwrap();

    assert_eq!(first, second);
    assert_eq!(
        before, after,
        "re-storing identical content must not add a blob"
    );
}

/// Free blocks on the filesystem holding `path`, in bytes — taken after
/// syncing it, because a filesystem with delayed allocation (btrfs,
/// ext4) does not charge a write against free space until it commits.
/// Without that, an unmeasured full copy reads back as costing nothing
/// and the assertion below passes for entirely the wrong reason
/// (confirmed: it did, until this existed).
///
/// `--file-system` matters: a bare `sync` flushes EVERY mounted
/// filesystem, which on a busy build machine took long enough to blow
/// nextest's 30 s slow-timeout. Only the one under test needs flushing.
fn free_bytes(path: &Path) -> u64 {
    let _ = std::process::Command::new("sync")
        .arg("--file-system")
        .arg(path)
        .status();
    let out = std::process::Command::new("stat")
        .args(["-f", "-c", "%f %S"])
        .arg(path)
        .output()
        .expect("stat -f");
    let text = String::from_utf8_lossy(&out.stdout);
    let mut parts = text.split_whitespace();
    let blocks: u64 = parts.next().unwrap().parse().unwrap();
    let size: u64 = parts.next().unwrap().parse().unwrap();
    blocks * size
}

/// Can this filesystem clone extents at all? Asked by trying it, not by
/// naming filesystems — a test that hard-codes "xfs means yes" is wrong
/// the moment someone builds on an XFS made without `reflink=1`.
fn supports_reflink(dir: &Path) -> bool {
    let src = dir.join(".reflink-probe-src");
    let dst = dir.join(".reflink-probe-dst");
    if std::fs::write(&src, vec![7u8; 1024 * 1024]).is_err() {
        return false;
    }
    let ok = std::process::Command::new("cp")
        .arg("--reflink=always")
        .args([&src, &dst])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let _ = std::fs::remove_file(&src);
    let _ = std::fs::remove_file(&dst);
    ok
}

/// The point of the whole exercise, measured rather than asserted from
/// the API surface: storing a large file on a reflink-capable filesystem
/// must not consume another copy of it.
///
/// Skipped where the filesystem cannot clone — there the fallback is a
/// real copy and that is correct, just slow. Deliberately generous
/// (under a quarter of the file) because this reads *filesystem* free
/// space, which other processes also move; the failure it is meant to
/// catch is a full second copy, which is 4x outside that band.
#[tokio::test]
async fn storing_a_large_file_does_not_consume_a_second_copy() {
    let dir = tempfile::tempdir().unwrap();
    if !supports_reflink(dir.path()) {
        eprintln!(
            "skipping: {} is not on a reflink-capable filesystem",
            dir.path().display()
        );
        return;
    }
    let store = store_at(dir.path()).await;

    // Big enough that a full copy is unmistakable against filesystem
    // noise, small enough to stay quick.
    let bytes = incompressible(256 * 1024 * 1024);
    let path = dir.path().join("camera-original.braw");
    tokio::fs::write(&path, &bytes).await.unwrap();
    store.shutdown().await.unwrap();
    let store = store_at(dir.path()).await;

    let before = free_bytes(dir.path());
    let file_id = store.write_path(&path).await.unwrap();
    store.shutdown().await.unwrap();
    let after = free_bytes(dir.path());

    let consumed = before.saturating_sub(after);
    let budget = bytes.len() as u64 / 4;
    assert!(
        consumed < budget,
        "storing a {} MiB file consumed {} MiB — that is a copy, not a clone",
        bytes.len() / 1024 / 1024,
        consumed / 1024 / 1024,
    );

    // And it is a real, readable version — a clone that couldn't be read
    // back would be a very cheap way to lose data.
    let store = store_at(dir.path()).await;
    assert_eq!(store.read_to_vec(file_id).await.unwrap(), bytes);
}

/// The requirement in one test: **the content cannot be lost by
/// deleting the original.** Not "cheap to store" — safe. A hardlinked
/// blob keeps the inode alive, so removing the live file frees nothing
/// and the store still returns the exact bytes.
///
/// This is the property that makes a link acceptable instead of a copy,
/// and it is the one that would silently disappear if the placement
/// ladder ever fell through to something referential.
#[tokio::test]
async fn deleting_the_original_does_not_lose_the_stored_content() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_at(dir.path()).await;
    let bytes = incompressible(64 * 1024 * 1024);
    let path = dir.path().join("only-copy.wav");
    tokio::fs::write(&path, &bytes).await.unwrap();

    let file_id = store.write_path(&path).await.unwrap();

    // The live file goes away entirely — the accident this protects
    // against.
    tokio::fs::remove_file(&path).await.unwrap();
    assert!(!path.exists());

    assert_eq!(
        store.read_to_vec(file_id).await.unwrap(),
        bytes,
        "the store must still hold the content after the original is deleted"
    );

    // And it survives a reopen: the bytes are on disk, not in a handle
    // this process happens to be holding.
    store.shutdown().await.unwrap();
    let store = store_at(dir.path()).await;
    assert_eq!(store.read_to_vec(file_id).await.unwrap(), bytes);
}

/// Storing content that is already on the same filesystem must consume
/// (essentially) nothing, at ANY size — the property that makes
/// importing an existing multi-terabyte archive possible at all.
///
/// No reflink probe here, unlike the measurement below it: a hardlink
/// needs only one filesystem, which a tempdir always satisfies.
#[tokio::test]
async fn linking_content_in_consumes_no_space() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_at(dir.path()).await;
    let bytes = incompressible(256 * 1024 * 1024);
    let path = dir.path().join("archive-original.wav");
    tokio::fs::write(&path, &bytes).await.unwrap();
    store.shutdown().await.unwrap();
    let store = store_at(dir.path()).await;

    let before = free_bytes(dir.path());
    let file_id = store.write_path(&path).await.unwrap();
    store.shutdown().await.unwrap();
    let after = free_bytes(dir.path());

    let consumed = before.saturating_sub(after);
    // Generous against filesystem noise; a copy would be 256 MiB, which
    // is 16x outside this.
    let budget = 16 * 1024 * 1024;
    assert!(
        consumed < budget,
        "linking a {} MiB file consumed {} MiB — that is a copy, not a link",
        bytes.len() / 1024 / 1024,
        consumed / 1024 / 1024,
    );

    let store = store_at(dir.path()).await;
    assert_eq!(store.read_to_vec(file_id).await.unwrap(), bytes);
}

/// A whole-tier blob nothing references is reclaimed. Without this the
/// linked inode would outlive its history forever — space that only
/// looks free until the original is deleted too.
#[tokio::test]
async fn gc_reclaims_an_unreferenced_whole_blob() {
    use std::collections::BTreeSet;
    use std::time::{Duration, SystemTime};

    let dir = tempfile::tempdir().unwrap();
    let store = ChunkStore::open_with_gc(
        dir.path(),
        ChunkerConfig::default(),
        files_store::chunk::GcConfig {
            interval: Duration::from_secs(3600),
        },
    )
    .await
    .unwrap();

    let path = dir.path().join("transient.wav");
    tokio::fs::write(&path, content(8 * 1024 * 1024))
        .await
        .unwrap();
    let file_id = store.write_path(&path).await.unwrap();
    assert!(store.has(file_id).await);

    tokio::time::sleep(Duration::from_millis(20)).await;
    let stats = store.gc(&BTreeSet::new(), SystemTime::now()).await.unwrap();
    assert_eq!(stats.manifests_swept, 1);

    let whole: Vec<_> = walk(&dir.path().join("whole"));
    assert!(
        whole.is_empty(),
        "the whole tier must be swept with its manifest, found {whole:?}"
    );
}

/// Every regular file under `dir`, recursively.
fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else {
            out.push(p);
        }
    }
    out
}

/// Replacing the live file — the ordinary save: write a new file, or
/// truncate and rewrite — versions correctly, and the previous version
/// stays readable.
///
/// Scoped deliberately to *replacement*, because that is what the
/// placement ladder actually guarantees everywhere. A reflink gives
/// separate inodes sharing extents, so even an in-place rewrite leaves
/// the stored version untouched; a hardlink does not, and asserting the
/// stronger property here would pass on btrfs/XFS and fail on ext4 —
/// green for the wrong reason, which is worse than not testing it.
/// In-place rewriting is out of scope by decision, not by accident.
#[tokio::test]
async fn replacing_the_live_file_versions_it_and_keeps_the_old_one() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_at(dir.path()).await;
    let original = content(2 * 1024 * 1024);
    let path = dir.path().join("edited.wav");
    tokio::fs::write(&path, &original).await.unwrap();
    let v1 = store.write_path(&path).await.unwrap();

    // Replace via a fresh file + rename, the safe-save pattern: the old
    // inode survives, so the store's link to it does too.
    let mut edited = original.clone();
    edited[..4096].fill(0xAB);
    edited.extend_from_slice(&content(1024));
    let staged = dir.path().join("edited.wav.new");
    tokio::fs::write(&staged, &edited).await.unwrap();
    tokio::fs::rename(&staged, &path).await.unwrap();
    let v2 = store.write_path(&path).await.unwrap();

    assert_ne!(v1, v2);
    assert_eq!(
        store.read_to_vec(v1).await.unwrap(),
        original,
        "the first version must still read back byte-for-byte"
    );
    assert_eq!(store.read_to_vec(v2).await.unwrap(), edited);
}
