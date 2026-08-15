//! Acceptance criterion: "Property tests cover chunking determinism (same
//! input, same chunks)."
//!
//! FastCDC's whole value proposition for dedup is that identical bytes
//! chunk identically no matter how many times you feed them through — if
//! that weren't true, two saves of the same file could disagree on which
//! chunks they share, and dedup silently stops working. These properties
//! exercise the public API directly (chunker + manifest), not internals.

use proptest::prelude::*;
use task_files_chunk_store::{ChunkRef, ChunkerConfig, Manifest, chunk_to_vec};

fn chunk_refs(data: &[u8], config: ChunkerConfig) -> Vec<ChunkRef> {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let chunks = chunk_to_vec(data, config).await.unwrap();
        chunks
            .into_iter()
            .map(|bytes| ChunkRef {
                hash: blake3::hash(&bytes),
                len: bytes.len() as u64,
            })
            .collect()
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Chunking the same bytes twice, independently, produces the exact
    /// same sequence of (hash, length) chunk boundaries.
    #[test]
    fn same_input_same_chunks(data in prop::collection::vec(any::<u8>(), 0..50_000)) {
        let config = ChunkerConfig::with_avg_size(4096);
        let first = chunk_refs(&data, config);
        let second = chunk_refs(&data, config);
        prop_assert_eq!(first, second);
    }

    /// Determinism composes into the manifest layer: two chunkings of the
    /// same bytes produce manifests with the same FileId.
    #[test]
    fn same_input_same_file_id(data in prop::collection::vec(any::<u8>(), 0..50_000)) {
        let config = ChunkerConfig::with_avg_size(4096);
        let a = Manifest::new(chunk_refs(&data, config));
        let b = Manifest::new(chunk_refs(&data, config));
        prop_assert_eq!(a.file_id(), b.file_id());
    }

    /// Chunk boundaries are a lossless partition of the input: concatenating
    /// them in order reconstructs the original bytes exactly.
    #[test]
    fn chunks_concatenate_to_original(data in prop::collection::vec(any::<u8>(), 0..50_000)) {
        let config = ChunkerConfig::with_avg_size(4096);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let chunks = rt.block_on(chunk_to_vec(&data[..], config)).unwrap();
        let reassembled: Vec<u8> = chunks.into_iter().flatten().collect();
        prop_assert_eq!(reassembled, data);
    }
}
