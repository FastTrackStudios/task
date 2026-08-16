//! Files platform (GitHub issue #255) version-store engine: jj-lib's
//! [`Backend`](jj_lib::backend::Backend) trait implemented over the CAS
//! chunk store ([`crate::chunk`]), per ADR 0001
//! (`apps/task/docs/adr/0001-files-version-store-jj-cas.md`).
//!
//! Everything above the `Backend` trait — op-log concurrency, divergent
//! changes, conflicted-tree merges, transactions — is jj-lib's own
//! machinery; this crate's job is a faithful, streaming, media-scale
//! implementation of that one trait plus the thin helpers Files' own tests
//! (and eventually its RPC layer, issue #257's own follow-ups) need on top:
//!
//! - [`repo::init_repo`] — wire [`VersionStoreBackend`] into a real jj repo.
//! - [`checkpoint::checkpoint`] — write one commit from an explicit list of
//!   writes/removes/renames (Files' "Session checkpoint" concept, at the
//!   lowest level).
//! - [`chain::version_chain`] — derive a file's per-version history from the
//!   commit DAG, following recorded renames.
//!
//! ```no_run
//! # async fn example() -> files_store::version::Result<()> {
//! use files_store::version::checkpoint::{checkpoint, Change};
//! use files_store::version::{chain, repo};
//! use jj_lib::backend::Backend as _;
//! use jj_lib::repo::Repo as _;
//! use jj_lib::repo_path::RepoPathBuf;
//!
//! let repo1 = repo::init_repo("/tmp/files-version-store-example".as_ref()).await?;
//! let root_id = repo1.store().root_commit_id().clone();
//! let path = RepoPathBuf::from_internal_string("mix.wav").unwrap();
//! let repo2 = checkpoint(
//!     &repo1,
//!     root_id,
//!     vec![Change::Write { path: path.clone(), content: b"v1".to_vec() }],
//!     "first save",
//! )
//! .await?;
//! let backend = repo2.store().backend_impl::<files_store::version::VersionStoreBackend>().unwrap();
//! let head = repo2.view().heads().iter().next().unwrap().clone();
//! let _history = chain::version_chain(backend, &head, &path).await?;
//! # Ok(())
//! # }
//! ```

pub mod backend;
pub mod chain;
pub mod checkpoint;
mod codec;
mod error;
pub mod gc;
mod objects;
pub mod repo;

pub use backend::VersionStoreBackend;
pub use error::{Error, Result};
