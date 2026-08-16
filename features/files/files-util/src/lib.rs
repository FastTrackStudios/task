//! Shared primitives for the Files platform (issue #255): path
//! confinement ([`paths`]) and the blocking seam ([`blocking`]).
//!
//! Both were written twice — once in `files`, once in `files-storage` —
//! before the PR #284 review pointed out that the security-critical half
//! was then maintained in three places. One implementation lives here;
//! the crates above consume it and map its errors onto their own.

pub mod paths;

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
