//! Verified, resumable range transfer of a whole-tier blob.
//!
//! The whole-file tier links a file into the store rather than chunking
//! it, so its manifest names one chunk of the file's whole length. That
//! is right for storage — a link costs nothing at any size — and it used
//! to mean a transfer had exactly one unit: send 800 GB or send nothing.
//!
//! What makes it divisible again is that BLAKE3 is a Merkle tree. Given
//! an outboard — the tree over the content, a few hundred KB whatever the
//! file's size — any range can be sent with the hashes on its path to the
//! root, so a receiver verifies each window as it lands and rejects a bad
//! one immediately rather than after the last byte.
//!
//! These tests are about that property, on the tier where it was missing.

use std::path::Path;

use bao_tree::{ChunkNum, ChunkRanges};
use files_store::chunk::{ChunkStore, ChunkerConfig};

/// Small enough that the fixtures below take the whole-file path.
const SMALL_THRESHOLD: u64 = 1024;

async fn store_at(root: &Path) -> ChunkStore {
    ChunkStore::open_with_config(
        root,
        ChunkerConfig::default().with_whole_file_threshold(SMALL_THRESHOLD),
    )
    .await
    .unwrap()
}

fn incompressible(len: usize) -> Vec<u8> {
    use std::io::Read as _;
    let mut buf = vec![0u8; len];
    std::fs::File::open("/dev/urandom")
        .expect("/dev/urandom")
        .read_exact(&mut buf)
        .expect("reading random bytes");
    buf
}

/// The chunk hash behind a whole-stored file — not its `FileId`, which
/// hashes the manifest.
async fn chunk_of(store: &ChunkStore, file_id: files_store::chunk::FileId) -> blake3::Hash {
    let manifest = store.manifest(file_id).await.expect("manifest");
    assert_eq!(manifest.chunks.len(), 1, "fixture: not the whole-file tier");
    manifest.chunks[0].hash
}

/// A whole-stored file has an outboard, so its ranges can be exported.
#[tokio::test]
async fn a_linked_file_can_export_a_verified_range() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_at(dir.path()).await;
    let bytes = incompressible(256 * 1024);
    let path = dir.path().join("take.wav");
    tokio::fs::write(&path, &bytes).await.unwrap();

    let file_id = store.write_path(&path).await.unwrap();
    let hash = chunk_of(&store, file_id).await;

    // The first 16 KiB, as bao: the data plus the proof it belongs here.
    let ranges = ChunkRanges::from(..ChunkNum(16));
    let bao = store
        .export_ranges(hash, ranges)
        .await
        .expect("a linked file must be able to prove its own ranges");
    assert!(
        !bao.is_empty(),
        "no outboard — the blob was never registered by reference"
    );
    store.shutdown().await.unwrap();
}

/// A range verifies into a *different* store, which is the point: the
/// receiver holds nothing but the hash and still knows the bytes are the
/// right ones.
#[tokio::test]
async fn a_range_verifies_against_the_hash_alone() {
    let origin_dir = tempfile::tempdir().unwrap();
    let origin = store_at(origin_dir.path()).await;
    let bytes = incompressible(256 * 1024);
    let path = origin_dir.path().join("take.wav");
    tokio::fs::write(&path, &bytes).await.unwrap();
    let file_id = origin.write_path(&path).await.unwrap();
    let hash = chunk_of(&origin, file_id).await;

    let receiver_dir = tempfile::tempdir().unwrap();
    let receiver = store_at(receiver_dir.path()).await;

    // Nothing of it here yet.
    assert!(
        receiver.have_ranges(hash).await.unwrap().is_empty(),
        "an empty store claimed to hold ranges"
    );

    let all = ChunkRanges::all();
    let bao = origin.export_ranges(hash, all.clone()).await.unwrap();
    receiver
        .import_ranges(hash, all, bao)
        .await
        .expect("a verified range must import");

    // And now it does — reported by iroh-blobs' own bitfield rather than
    // by anything this crate keeps.
    assert!(
        !receiver.have_ranges(hash).await.unwrap().is_empty(),
        "the imported ranges are not visible to the resume cursor"
    );
    origin.shutdown().await.unwrap();
    receiver.shutdown().await.unwrap();
}

/// Half now, half later — and the cursor moves in between.
///
/// This is resumability at the level that matters: the receiver is asked
/// what it has, asks only for the rest, and ends up with the file. An
/// interrupted 800 GB transfer is this, with a bigger number.
#[tokio::test]
async fn a_transfer_can_stop_halfway_and_finish_later() {
    let origin_dir = tempfile::tempdir().unwrap();
    let origin = store_at(origin_dir.path()).await;
    let bytes = incompressible(256 * 1024);
    let path = origin_dir.path().join("take.wav");
    tokio::fs::write(&path, &bytes).await.unwrap();
    let file_id = origin.write_path(&path).await.unwrap();
    let hash = chunk_of(&origin, file_id).await;

    let receiver_dir = tempfile::tempdir().unwrap();
    let receiver = store_at(receiver_dir.path()).await;

    // 256 KiB is 256 BLAKE3 chunks. Take the first half.
    let first = ChunkRanges::from(..ChunkNum(128));
    let bao = origin.export_ranges(hash, first.clone()).await.unwrap();
    receiver.import_ranges(hash, first, bao).await.unwrap();

    let have = receiver.have_ranges(hash).await.unwrap();
    assert!(!have.is_empty(), "the first half left no trace");

    // The rest, asked for as the complement of what is held — which is
    // how a resumed pull decides what to request.
    let rest = ChunkRanges::all().difference(&have);
    let bao = origin.export_ranges(hash, rest.clone()).await.unwrap();
    receiver
        .import_ranges(hash, rest, bao)
        .await
        .expect("the remainder must import onto the half already there");

    // Whole, and byte-for-byte: a seam between the halves would show up
    // here and nowhere else.
    let back = receiver.read_chunk(hash).await.expect("read it back");
    assert_eq!(back, bytes, "the two halves did not join cleanly");
    origin.shutdown().await.unwrap();
    receiver.shutdown().await.unwrap();
}

/// A tampered window is refused, and refused *before* it is written.
///
/// The failure the outboard exists to catch. Without it the only check
/// available is over the whole file, so a corrupt window is discovered
/// after every byte has been transferred — and on an 800 GB take that is
/// a day's transfer to learn something a proof would have said at once.
#[tokio::test]
async fn a_tampered_range_is_refused() {
    let origin_dir = tempfile::tempdir().unwrap();
    let origin = store_at(origin_dir.path()).await;
    let bytes = incompressible(256 * 1024);
    let path = origin_dir.path().join("take.wav");
    tokio::fs::write(&path, &bytes).await.unwrap();
    let file_id = origin.write_path(&path).await.unwrap();
    let hash = chunk_of(&origin, file_id).await;

    let receiver_dir = tempfile::tempdir().unwrap();
    let receiver = store_at(receiver_dir.path()).await;

    let all = ChunkRanges::all();
    let mut bao = origin.export_ranges(hash, all.clone()).await.unwrap();
    // Flip a byte deep in the payload, past the header and the first
    // parent hashes.
    let at = bao.len() / 2;
    bao[at] ^= 0xFF;

    let refused = receiver.import_ranges(hash, all, bao).await;
    assert!(refused.is_err(), "a tampered range was accepted");
    origin.shutdown().await.unwrap();
    receiver.shutdown().await.unwrap();
}
