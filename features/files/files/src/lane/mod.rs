//! The v2 service lanes, one module per trait.
//!
//! Each module here implements one `#[architect::rpc]` trait from
//! `files_proto::service` for [`FilesBackend`](crate::backend::FilesBackend),
//! mirroring the spec section that trait owns. The mapping is in
//! `files_proto::service`'s own docs; the short version is that a
//! requirement has exactly one plausible home, so `t[impl
//! files.write.surface]` can only land in [`write`].
//!
//! Implementing on `FilesBackend` rather than on a per-lane struct is
//! deliberate for the migration: the backend already holds the registry,
//! the ignore cache, the cadence engine and the version stores, and a
//! lane that had to be handed all of that would be a rewrite rather than
//! a move. As legacy methods migrate, they move *out* of `backend.rs`
//! and *into* the lane that owns them — which is the mechanism by which
//! that file shrinks.
//!
//! ⚠️ A method is not reachable until it has a `permits.rs` row. The gate
//! enforces in production and fails closed, so a lane is mounted in the
//! same change that grants it.

pub mod curation;
pub mod roots;
pub mod sync;
pub mod tree;
pub mod version;

use files_proto::error::FilesFault;

use crate::error::Error;

/// Run blocking work on the pool, mapping to the v2 fault type.
///
/// Everything below this crate is synchronous — jj-lib and the chunk
/// store both — and a full-tree scan or a multi-GB checkpoint must not
/// stall the runtime's other org RPCs.
pub(crate) async fn blocking<T, F>(f: F) -> Result<T, FilesFault>
where
    F: FnOnce() -> Result<T, Error> + Send + 'static,
    T: Send + 'static,
{
    files_store::blocking(f, |e| Error::Io(std::io::Error::other(e)))
        .await
        .map_err(FilesFault::from)
}
