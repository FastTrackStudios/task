//! The Files storage engines: content-addressed chunks, the version
//! store built over them, and the path confinement both depend on.
//!
//! Previously three crates — `task-files-chunk-store`,
//! `task-files-version-store` and `task-files-util` — which was one crate
//! per file rather than one per boundary. They were never independently
//! useful: the version store is meaningless without the chunk store
//! beneath it, and `files-util` existed only because path confinement had
//! been written twice and needed one home.
//!
//! ## Layout
//!
//! - [`chunk`] — streaming FastCDC (v2020) chunking, BLAKE3 addressing,
//!   chunks held in an `iroh-blobs` `FsStore`, with Files-owned chunk
//!   manifests kept outside the blob store so the index is rebuildable.
//! - [`version`] — `jj-lib`'s `Backend` implemented over that store, per
//!   ADR 0001: per-file version chains, recorded renames (`CopyHistory`),
//!   and divergent concurrent writes riding jj's own op-log.
//! - [`paths`] — path confinement. Security-critical, and therefore
//!   deliberately single-copy.
//!
//! `chunk` and `version` each keep their own `Error`, which is why they
//! stay namespaced rather than flattened to the root: both had one, and
//! two `Error` types cannot share a root.
//!
//! Nothing here knows about RPC, architect, or the org. It is the layer
//! `files-domain` is written against.

pub mod chunk;
pub mod paths;
pub mod version;

pub use paths::{PathError, confine, create_confined, safe_relative, to_utf8};

/// Run synchronous work on the blocking thread pool.
///
/// Everything the Files platform does that touches jj-lib or the chunk
/// store is synchronous — those futures are not `Send` on every path, so
/// they are driven with `pollster::block_on` rather than awaited inside
/// an `#[architect::rpc]` method's future. Running that work on
/// `tokio::task::spawn_blocking` rather than inline is the other half:
/// a full-tree scan, a multi-GB checkpoint, or a root replication must
/// not stall the runtime's other org RPCs (PR #280 review).
///
/// Generic over the error type, with `on_panic` supplying the caller's
/// own "the blocking task panicked" variant — that is the only thing
/// that differed between the two copies of this function.
pub async fn blocking<T, E, F, P>(f: F, on_panic: P) -> Result<T, E>
where
    F: FnOnce() -> Result<T, E> + Send + 'static,
    P: FnOnce(String) -> E,
    T: Send + 'static,
    E: Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(result) => result,
        Err(e) => Err(on_panic(e.to_string())),
    }
}
