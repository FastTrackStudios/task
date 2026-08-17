//! What an outboard costs.
//!
//! Registering a whole-tier blob by reference stores the BLAKE3 tree over
//! its content, so any range of it can be proved. That tree is real disk,
//! spent on every large file the store takes on — and if it were
//! proportionally expensive, "verified transfers" would quietly mean
//! "pay a percentage of your archive again".
//!
//! It is not: about 0.4% of the content. An 800 GB take costs roughly
//! 3 GB of outboard, which buys resumable transfer verified at 1 KiB
//! granularity. This test is the ceiling on that ratio, so a change to
//! the tree's shape has to come past it.

use std::path::Path;

use files_store::chunk::{ChunkStore, ChunkerConfig};

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

/// Bytes of outboard under `root` — the `.obao*` files iroh-blobs writes
/// beside referenced content.
///
/// Deliberately *only* those. A store also holds a `blobs.db`, which is
/// about a megabyte the moment it exists and does not grow with the file;
/// counting it made a 64 MiB fixture look like 2% overhead when the part
/// that scales is a fifth of that.
fn outboard_bytes(root: &Path) -> u64 {
    fn walk(dir: &Path, total: &mut u64) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.is_dir() {
                walk(&path, total);
            } else if path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.starts_with("obao"))
            {
                *total += meta.len();
            }
        }
    }
    let mut total = 0;
    walk(root, &mut total);
    total
}

#[tokio::test]
async fn an_outboard_costs_a_small_fraction_of_the_content() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_at(dir.path()).await;
    let len: usize = 64 * 1024 * 1024;
    let bytes = incompressible(len);
    let path = dir.path().join("take.wav");
    tokio::fs::write(&path, &bytes).await.unwrap();

    store.write_path(&path).await.unwrap();
    store.shutdown().await.unwrap();

    let outboard = outboard_bytes(dir.path());
    assert!(
        outboard > 0,
        "no outboard was written — a referenced blob cannot prove its ranges"
    );
    // 1% of the content, against a measured 0.39%. A ceiling rather than
    // a pin: the point is to catch a change that makes the tree
    // proportionally expensive, not to freeze a constant.
    let ceiling = (len as u64) / 100;
    assert!(
        outboard < ceiling,
        "an outboard for {} MiB cost {} KiB — over the {} KiB ceiling",
        len / 1024 / 1024,
        outboard / 1024,
        ceiling / 1024,
    );
}
