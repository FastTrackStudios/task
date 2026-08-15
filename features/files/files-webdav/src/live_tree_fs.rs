//! [`LiveTreeFs`] — the filesystem view one mounted File Root presents.
//!
//! This is the "custom filesystem view exposing the current head of
//! live trees only" from issue #274: a [`dav_server::localfs::LocalFs`]
//! based at the root's live tree, wrapped in a decorator that makes the
//! root's own internals — the marker file and the version-store
//! directory — not exist as far as WebDAV is concerned.
//!
//! **Why that is the whole of "no version history".** Files keeps a
//! root's history *inside* the root, under
//! [`files::STORE_DIR`]. A plain `LocalFs` over the live tree would
//! therefore serve the entire jj repo and CAS as ordinary files, and a
//! Finder user could delete it. Hiding those two names is what makes
//! the mount show exactly the current head and nothing else — there is
//! no other history surface on this bridge to close, because the bridge
//! has no version-addressed URL space at all.
//!
//! **Confinement.** Every path is guarded by
//! [`LiveTreeFs::guard`]: canonicalize the resolved target (or its
//! parent, for a file about to be created) and require it to stay under
//! the root's own canonical live tree. That is deliberately the same
//! canonicalize-then-prefix-check `FilesBackend::browse` uses, so a
//! symlink planted inside a root cannot read or write outside it —
//! textual `..` is already rejected earlier, by `DavPath` normalization.

use std::path::{Path, PathBuf};

use dav_server::davpath::DavPath;
use dav_server::fs::{
    DavDirEntry, DavFile, DavFileSystem, DavMetaData, DavProp, FsError, FsFuture, FsResult,
    FsStream, OpenOptions, ReadDirMeta,
};
use dav_server::localfs::LocalFs;
use futures_util::StreamExt as _;

/// Names inside a root that the bridge must not reveal: the root's
/// marker file and its version store.
const HIDDEN: [&str; 2] = [files::MARKER_FILE, files::STORE_DIR];

/// Is `name` one of a root's internals?
///
/// **Case-insensitively**, because this bridge exists for macOS (APFS)
/// and Windows (NTFS), which are case-insensitive: a byte-exact
/// comparison let `.FTS-FILES` sail past the name check while the OS
/// happily resolved it to the real `.fts-files`, handing the whole jj
/// repo + CAS to the mount — readable and recursively deletable — and
/// the name never had to be *listed* to be guessed (PR #287 review).
/// Case-folding on Linux too is deliberate: it costs nothing, and the
/// guarantee should not depend on which filesystem a root happens to
/// sit on.
fn is_hidden(name: &[u8]) -> bool {
    HIDDEN
        .iter()
        .any(|h| h.as_bytes().eq_ignore_ascii_case(name))
}

#[derive(Clone)]
pub struct LiveTreeFs {
    inner: LocalFs,
    /// Canonicalized live tree — the confinement boundary.
    base: PathBuf,
    /// Canonicalized version store, when it exists — the *resolved*
    /// boundary the name check above approximates. Held so
    /// [`LiveTreeFs::confine`] can reject a target that lands inside
    /// the store however it got there: a case variant on a
    /// case-insensitive volume, a symlink planted in the live tree, or
    /// any future spelling nobody thought of.
    store: Option<PathBuf>,
}

impl std::fmt::Debug for LiveTreeFs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveTreeFs")
            .field("base", &self.base)
            .finish()
    }
}

impl LiveTreeFs {
    /// View of the live tree at `base`. `base` must already be
    /// canonical — it comes from the Files registry, which canonicalized
    /// (and org-confined) it at `create_root` time.
    pub fn new(base: impl Into<PathBuf>) -> Self {
        let base = base.into();
        let store = base.join(files::STORE_DIR).canonicalize().ok();
        Self {
            // `public = false`: files the bridge creates are the
            // server user's own (0600/0700), matching how the rest of
            // an org's data root is written. `case_insensitive = false`
            // (documented as "a *lot* of overhead"), `macos = false`.
            inner: *LocalFs::new(&base, false, false, false),
            base,
            store,
        }
    }

    /// Reject a path that names, or passes through, one of a root's
    /// internals, or that resolves outside the root's live tree.
    fn guard(&self, path: &DavPath) -> FsResult<()> {
        let rel = path.as_rel_ospath();
        if rel
            .components()
            .any(|c| is_hidden(c.as_os_str().as_encoded_bytes()))
        {
            // `NotFound`, not `Forbidden`: to a WebDAV client these
            // names simply are not part of the tree.
            return Err(FsError::NotFound);
        }
        self.confine(&self.base.join(rel))
    }

    /// Canonicalize `target` — falling back to its parent when the
    /// target does not exist yet (a `PUT`/`MKCOL` destination) — and
    /// require the result to stay under [`LiveTreeFs::base`] and
    /// *outside* the version store.
    ///
    /// The store check is the one that actually holds: it runs on the
    /// resolved path, so it catches every route into the store the
    /// textual name check cannot — a case variant on APFS/NTFS, a
    /// symlink inside the live tree pointing at `.fts-files`, a
    /// hardlinked directory. The name check stays as the cheap first
    /// pass and as what makes the internals invisible in listings.
    fn confine(&self, target: &Path) -> FsResult<()> {
        let resolved = match target.canonicalize() {
            Ok(p) => p,
            Err(_) => {
                let parent = target.parent().ok_or(FsError::NotFound)?;
                // A not-yet-existing parent is a genuine 404/409 for
                // every operation this guards; let the inner fs produce
                // the accurate status rather than guessing one here.
                match parent.canonicalize() {
                    Ok(p) => p,
                    Err(_) => return Ok(()),
                }
            }
        };
        if resolved != self.base && !resolved.starts_with(&self.base) {
            return Err(FsError::Forbidden);
        }
        if let Some(store) = &self.store
            && (resolved == *store || resolved.starts_with(store))
        {
            // `NotFound`, matching the name check: the store is not
            // part of this tree, by whatever spelling it was reached.
            return Err(FsError::NotFound);
        }
        Ok(())
    }
}

/// `guard` then delegate — the shape every method below shares.
macro_rules! guarded {
    ($self:ident, $path:expr, $call:expr) => {{
        if let Err(e) = $self.guard($path) {
            return Box::pin(std::future::ready(Err(e)));
        }
        $call
    }};
}

impl DavFileSystem for LiveTreeFs {
    fn open<'a>(
        &'a self,
        path: &'a DavPath,
        options: OpenOptions,
    ) -> FsFuture<'a, Box<dyn DavFile>> {
        guarded!(self, path, self.inner.open(path, options))
    }

    fn read_dir<'a>(
        &'a self,
        path: &'a DavPath,
        meta: ReadDirMeta,
    ) -> FsFuture<'a, FsStream<Box<dyn DavDirEntry>>> {
        guarded!(self, path, {
            Box::pin(async move {
                let stream = self.inner.read_dir(path, meta).await?;
                // The listing is where internals would otherwise become
                // visible; drop them here rather than relying on the
                // client never asking for them by name (`guard` covers
                // that half).
                let filtered = stream.filter(|entry| {
                    let keep = match entry {
                        Ok(e) => !is_hidden(&e.name()),
                        Err(_) => true,
                    };
                    std::future::ready(keep)
                });
                Ok(Box::pin(filtered) as FsStream<Box<dyn DavDirEntry>>)
            })
        })
    }

    fn metadata<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, Box<dyn DavMetaData>> {
        guarded!(self, path, self.inner.metadata(path))
    }

    fn symlink_metadata<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, Box<dyn DavMetaData>> {
        guarded!(self, path, self.inner.symlink_metadata(path))
    }

    fn create_dir<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, ()> {
        guarded!(self, path, self.inner.create_dir(path))
    }

    fn remove_dir<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, ()> {
        guarded!(self, path, self.inner.remove_dir(path))
    }

    fn remove_file<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, ()> {
        guarded!(self, path, self.inner.remove_file(path))
    }

    fn rename<'a>(&'a self, from: &'a DavPath, to: &'a DavPath) -> FsFuture<'a, ()> {
        guarded!(self, from, guarded!(self, to, self.inner.rename(from, to)))
    }

    fn copy<'a>(&'a self, from: &'a DavPath, to: &'a DavPath) -> FsFuture<'a, ()> {
        guarded!(self, from, guarded!(self, to, self.inner.copy(from, to)))
    }

    fn have_props<'a>(
        &'a self,
        path: &'a DavPath,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + 'a>> {
        self.inner.have_props(path)
    }

    fn get_props<'a>(&'a self, path: &'a DavPath, do_content: bool) -> FsFuture<'a, Vec<DavProp>> {
        guarded!(self, path, self.inner.get_props(path, do_content))
    }

    fn get_prop<'a>(&'a self, path: &'a DavPath, prop: DavProp) -> FsFuture<'a, Vec<u8>> {
        guarded!(self, path, self.inner.get_prop(path, prop))
    }

    fn patch_props<'a>(
        &'a self,
        path: &'a DavPath,
        patch: Vec<(bool, DavProp)>,
    ) -> FsFuture<'a, Vec<(http::StatusCode, DavProp)>> {
        guarded!(self, path, self.inner.patch_props(path, patch))
    }

    fn set_accessed<'a>(
        &'a self,
        path: &'a DavPath,
        tm: std::time::SystemTime,
    ) -> FsFuture<'a, ()> {
        guarded!(self, path, self.inner.set_accessed(path, tm))
    }

    fn set_modified<'a>(
        &'a self,
        path: &'a DavPath,
        tm: std::time::SystemTime,
    ) -> FsFuture<'a, ()> {
        guarded!(self, path, self.inner.set_modified(path, tm))
    }

    fn get_quota(&'_ self) -> FsFuture<'_, (u64, Option<u64>)> {
        self.inner.get_quota()
    }
}
