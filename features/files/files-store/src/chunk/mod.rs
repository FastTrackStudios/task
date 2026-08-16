//! Files platform (GitHub issue #255) content-addressed chunk store.
//!
//! This is the CAS substrate ADR 0001
//! (`apps/task/docs/adr/0001-files-version-store-jj-cas.md`) calls for:
//! streaming FastCDC (v2020) chunking, blake3 content addressing, chunks
//! held in an [`iroh_blobs`] `FsStore`, and Files-owned chunk manifests
//! kept as plain files *outside* that store so it is rebuildable — the
//! manifest is the only thing that turns a bag of content-addressed
//! chunks back into a file. A file's [`FileId`] is the hash of its
//! manifest, so byte-identical saves always resolve to the same id.
//!
//! This crate is the substrate only: it knows nothing about File Roots,
//! versions, or jj. The version-store crate (a future jj `Backend`) is
//! the consumer — see issue #257.
//!
//! ```no_run
//! # async fn example() -> files_store::chunk::Result<()> {
//! use files_store::chunk::ChunkStore;
//!
//! let store = ChunkStore::open("/tmp/files-cas-example").await?;
//! let file_id = store.write_stream(&b"hello, files"[..]).await?;
//! let bytes = store.read_to_vec(file_id).await?;
//! assert_eq!(bytes, b"hello, files");
//! store.shutdown().await?;
//! # Ok(())
//! # }
//! ```

mod chunker;
mod error;
pub mod gc;
mod manifest;
mod store;

pub use chunker::{ChunkerConfig, chunk_to_vec};
pub use error::{Error, Result};
pub use gc::{GcConfig, GcStats};
pub use manifest::{ChunkRef, FileId, Manifest};
pub use store::ChunkStore;

// Consumers building `ChunkRef`s / verifying chunk hashes must use the
// SAME blake3 this crate hashes with — re-exported so they can't drift
// onto their own version (issue #264's sync layer is the first).
pub use blake3;
