//! Getting one live-tree file's bytes into a root's object store, per
//! flavor — the seam [`crate::checkpoint`] writes through.
//!
//! Both flavors must satisfy the same two properties, and neither gets
//! them from jj's `Backend::write_file` alone:
//!
//! - **Bounded memory.** A checkpoint may cross files far larger than
//!   RAM. The CAS backend's `write_file` already streams (PR #280); jj's
//!   `GitBackend::write_file` does not — it `read_to_end`s the whole file
//!   into a `Vec` and `unwrap()`s the read (jj-lib 0.44
//!   `git_backend.rs:1068`), which is the same whole-root-OOM class PR
//!   #280 fixed for media, plus a panic path on a mid-read I/O error (PR
//!   #282 review). So [`ContentStore::Git`] writes the blob itself
//!   through gix's streaming object store instead.
//! - **Don't write what's already there.** A git blob id is a pure
//!   function of the bytes, so [`ContentStore::probe`] can compute a
//!   file's identity *without writing anything*; the checkpoint compares
//!   that to the head tree first and skips untouched files entirely. On
//!   a tracking day where two files out of ten thousand changed, the
//!   other 9998 cost one streaming hash each and no object writes. The
//!   CAS flavor's probe is not as cheap — its id is a chunk-manifest
//!   hash, so the probe still reads the bytes — but it likewise writes
//!   nothing. (Cheap *stat*-based skipping for both flavors belongs
//!   with the cadence engine's watcher state, #260.)
//! - **Don't copy what can be cloned.** Both CAS entry points here take
//!   the disk **path**, not a reader, which is what lets
//!   `ChunkStore::write_path` bring a large file in with a reflink
//!   rather than a copy. `Backend::write_file`'s signature cannot
//!   express that — it has only a reader — and going through it is how
//!   a media root's first checkpoint ends up writing a second copy of
//!   every byte in the root.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use jj_lib::backend::{Backend, FileId};
use jj_lib::object_id::ObjectId as _;
use jj_lib::repo::{ReadonlyRepo, Repo as _};
use jj_lib::repo_path::RepoPath;
use tokio_util::compat::TokioAsyncReadCompatExt as _;

use crate::error::{Error, Result};

pub enum ContentStore<'a> {
    /// Media roots: jj's own streaming `Backend::write_file` over the CAS
    /// chunk store. The concrete backend rides along when available so
    /// [`ContentStore::probe`] can derive ids without writing (see
    /// below); the trait object alone still writes fine.
    Cas {
        backend: &'a dyn Backend,
        version_store: Option<&'a files_store::version::VersionStoreBackend>,
    },
    /// Software roots: the colocated git object database, written
    /// directly (see the module doc for why not through jj).
    Git(gix::Repository),
}

impl<'a> ContentStore<'a> {
    /// Pick the writer matching `repo`'s backend. A git-backed repo is
    /// recognized through jj-lib's own accessor, so a repo that merely
    /// claims the name can't slip through.
    pub fn for_repo(repo: &'a Arc<ReadonlyRepo>, backend: &'a dyn Backend) -> Result<Self> {
        match jj_lib::git::get_git_repo(repo.store()) {
            Ok(git_repo) => Ok(Self::Git(git_repo)),
            Err(_) => Ok(Self::Cas {
                backend,
                version_store: repo
                    .store()
                    .backend_impl::<files_store::version::VersionStoreBackend>(),
            }),
        }
    }

    /// This file's content id, computed **without writing anything** —
    /// `None` only when the backend has no way to know it in advance.
    ///
    /// Git: blob ids are a pure function of the bytes. CAS: the chunk
    /// store's `probe_path` runs the same size decision, hashing and
    /// manifest derivation as a real write, minus every store touch —
    /// which is what keeps an is-this-already-versioned comparison from
    /// persisting never-versioned bytes as orphaned chunks (PR #289
    /// review).
    pub fn probe(&self, disk_path: &Path) -> Result<Option<FileId>> {
        match self {
            Self::Cas {
                version_store: Some(vs),
                ..
            } => {
                // Path-taking, not reader-taking: the store's size
                // decision (chunk vs. store whole — see
                // `ChunkStore::write_path`) has to be the same one the
                // write will make, or an untouched file probes as
                // changed on every capture.
                let id = pollster::block_on(vs.chunks().probe_path(disk_path))
                    .map_err(|e| Error::Repo(format!("{}: probing: {e}", disk_path.display())))?;
                Ok(Some(FileId::new(id.as_bytes().to_vec())))
            }
            Self::Cas {
                version_store: None,
                ..
            } => Ok(None),
            Self::Git(git_repo) => {
                let len = std::fs::metadata(disk_path)?.len();
                let mut file = std::fs::File::open(disk_path)?;
                let oid = gix::objs::compute_stream_hash(
                    git_repo.object_hash(),
                    gix::objs::Kind::Blob,
                    &mut file,
                    len,
                    &mut gix::progress::Discard,
                    &AtomicBool::new(false),
                )
                .map_err(|e| Error::Repo(format!("{}: hashing: {e}", disk_path.display())))?;
                Ok(Some(FileId::new(oid.as_bytes().to_vec())))
            }
        }
    }

    /// Store the file's bytes and return its content id. `probed` is
    /// [`ContentStore::probe`]'s answer for the same file, threaded back
    /// in so the id isn't computed twice — and so an object already in
    /// the store is never rewritten.
    pub async fn write(
        &self,
        path: &RepoPath,
        disk_path: &Path,
        probed: Option<FileId>,
    ) -> Result<FileId> {
        match self {
            Self::Cas {
                version_store: Some(vs),
                ..
            } => {
                // By path, so a large file can be brought in with a
                // reflink instead of a copy (`ChunkStore::write_path`) —
                // the difference between a media root's first checkpoint
                // costing metadata and it costing a second copy of the
                // whole root. `Backend::write_file` cannot do this: its
                // signature has only a reader, no file to clone.
                let id =
                    vs.chunks().write_path(disk_path).await.map_err(|e| {
                        Error::Repo(format!("{}: writing: {e}", disk_path.display()))
                    })?;
                Ok(FileId::new(id.as_bytes().to_vec()))
            }
            Self::Cas {
                backend,
                version_store: None,
            } => {
                // `Backend::write_file` reads through futures-io; tokio's
                // file handle wears the `compat()` adapter (the same seam
                // the version-store backend uses internally).
                let mut disk = tokio::fs::File::open(disk_path).await?.compat();
                Ok(backend.write_file(path, &mut disk).await?)
            }
            Self::Git(git_repo) => Self::write_git_blob(git_repo, disk_path, probed),
        }
    }

    fn write_git_blob(
        git_repo: &gix::Repository,
        disk_path: &Path,
        probed: Option<FileId>,
    ) -> Result<FileId> {
        use gix::objs::{Exists as _, Write as _};

        let known = probed
            .as_ref()
            .map(|id| gix::ObjectId::from_bytes_or_panic(id.as_bytes()));
        if let (Some(id), Some(oid)) = (&probed, &known)
            && git_repo.objects.exists(oid)
        {
            // Identical bytes are already an object here — nothing to
            // write, and the blob is immutable, so this is complete.
            return Ok(id.clone());
        }

        let len = std::fs::metadata(disk_path)?.len();
        let mut file = std::fs::File::open(disk_path)?;
        let written = match known {
            Some(oid) => git_repo.objects.write_stream_with_known_id(
                gix::objs::Kind::Blob,
                len,
                &mut file,
                oid,
            ),
            None => git_repo
                .objects
                .write_stream(gix::objs::Kind::Blob, len, &mut file),
        }
        .map_err(|e| Error::Repo(format!("{}: writing blob: {e}", disk_path.display())))?;
        Ok(FileId::new(written.as_bytes().to_vec()))
    }
}
