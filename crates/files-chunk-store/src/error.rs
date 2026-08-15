use std::io;

/// Errors returned by [`crate::ChunkStore`] and the manifest codec.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] io::Error),

    /// An iroh-blobs operation (add/read/status/shutdown) failed.
    #[error("chunk blob store error: {0}")]
    Store(String),

    /// A manifest's bytes did not decode (bad magic/version/length, or a
    /// hash/length pair was truncated).
    #[error("malformed chunk manifest: {0}")]
    Manifest(String),

    /// A [`crate::ChunkerConfig`] fell outside the bounds
    /// `fastcdc::v2020::AsyncStreamCDC` requires.
    #[error("invalid chunker config: {0}")]
    InvalidConfig(String),

    /// No manifest is on disk for this `FileId`.
    #[error("unknown file id: {0}")]
    UnknownFileId(String),

    /// A manifest names a chunk hash that isn't in the blob store, or the
    /// stored chunk is a different length than the manifest recorded.
    #[error("chunk {0} missing or corrupt in blob store")]
    MissingChunk(String),

    /// [`crate::ChunkStore::gc`] was called on a store opened without
    /// chunk-level GC enabled (`ChunkStore::open`/`open_with_config`, not
    /// [`crate::ChunkStore::open_with_gc`]).
    #[error("chunk-level GC is not enabled for this store (open it with ChunkStore::open_with_gc)")]
    GcDisabled,
}

pub type Result<T> = std::result::Result<T, Error>;
