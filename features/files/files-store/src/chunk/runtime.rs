//! The one tokio runtime every iroh-blobs `FsStore` in the process runs on.
//!
//! iroh-blobs' own `FsStore::load_with_opts` builds a private
//! multi-thread runtime for each store, sized to the number of cores
//! (threads `iroh-blob-store-N`). A server holds one chunk store per root,
//! so that came to `roots × cores` threads: 1,280 for 40 roots on 32
//! cores. Every thread can take its own glibc malloc arena, and the free
//! bytes parked in those arenas grew the server's RSS by about
//! 250 MiB/h while the heap actually in use stayed flat.
//!
//! [`load_fs_store`] opens stores on one shared runtime instead, through
//! the `load_with_opts_on` entry point of the vendored iroh-blobs (see
//! `vendor/iroh-blobs/PATCHED.md`). Its size does not depend on the
//! number of stores: [`threads`] workers plus at most [`threads`]
//! blocking threads, all named [`THREAD_NAME`].
//!
//! Size it with [`THREADS_ENV`] (`TASK_BLOB_STORE_THREADS`, default
//! [`DEFAULT_THREADS`]), read once, the first time a store is opened.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use iroh_blobs::store::fs::FsStore;
use iroh_blobs::store::fs::options::Options;

/// Environment variable that sets the shared runtime's worker count.
pub const THREADS_ENV: &str = "TASK_BLOB_STORE_THREADS";

/// Worker count when [`THREADS_ENV`] is unset or not a positive integer.
pub const DEFAULT_THREADS: usize = 4;

/// Name of every thread of the shared runtime. Fits the kernel's
/// 15-byte `comm`, so it reads back whole from `/proc/<pid>/task/*/comm`.
pub const THREAD_NAME: &str = "task-blob-store";

/// Parse a [`THREADS_ENV`] value; `None`, empty, zero or garbage fall back
/// to [`DEFAULT_THREADS`].
fn parse_threads(value: Option<&str>) -> usize {
    value
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_THREADS)
}

/// Worker threads of the shared blob-store runtime: [`THREADS_ENV`], or
/// [`DEFAULT_THREADS`]. Read once per process.
pub fn threads() -> usize {
    static THREADS: OnceLock<usize> = OnceLock::new();
    *THREADS.get_or_init(|| parse_threads(std::env::var(THREADS_ENV).ok().as_deref()))
}

/// Upper bound on the shared runtime's threads: its workers plus its
/// blocking pool, which is capped at the same size.
pub fn max_threads() -> usize {
    2 * threads()
}

/// Handle of the shared runtime, built on first use.
///
/// The runtime lives in a static and is never dropped: dropping a runtime
/// from inside async code panics, and stores are opened from async code.
pub fn handle() -> tokio::runtime::Handle {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            let n = threads();
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(n)
                .max_blocking_threads(n)
                .thread_name(THREAD_NAME)
                .enable_time()
                .build()
                .expect("building the shared blob-store runtime")
        })
        .handle()
        .clone()
}

/// Open (or create) an iroh-blobs `FsStore` on the shared runtime. The
/// replacement for `FsStore::load_with_opts`.
pub async fn load_fs_store(db_path: PathBuf, options: Options) -> std::io::Result<FsStore> {
    FsStore::load_with_opts_on(db_path, options, handle())
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))
}

/// Open (or create) an `FsStore` rooted at `root` with default options on
/// the shared runtime. The replacement for `FsStore::load`.
pub async fn load_fs_store_at(root: impl AsRef<Path>) -> std::io::Result<FsStore> {
    let root = root.as_ref();
    load_fs_store(root.join("blobs.db"), Options::new(root)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threads_env_parsing() {
        assert_eq!(parse_threads(None), DEFAULT_THREADS);
        assert_eq!(parse_threads(Some("")), DEFAULT_THREADS);
        assert_eq!(parse_threads(Some("0")), DEFAULT_THREADS);
        assert_eq!(parse_threads(Some("lots")), DEFAULT_THREADS);
        assert_eq!(parse_threads(Some("1")), 1);
        assert_eq!(parse_threads(Some(" 8 ")), 8);
    }

    #[test]
    fn thread_name_fits_comm() {
        // Linux truncates thread names to 15 bytes in /proc/*/comm.
        assert!(THREAD_NAME.len() <= 15);
    }
}
