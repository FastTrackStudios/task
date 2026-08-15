//! [`RootsFs`] — the read-only collection at the top of the mount,
//! whose children are the org's WebDAV-visible File Roots.
//!
//! Finder and Explorer both mount a *single* URL, so the bridge needs
//! one collection that lists the roots; navigating into a child is
//! handled by [`crate::bridge`] switching to that root's
//! [`crate::live_tree_fs::LiveTreeFs`]. Nothing here touches a version
//! store, and nothing here is writable: roots are created through
//! `FilesService::create_root` (which does the marker file, the
//! registry entry, and the store init), never by a client dropping a
//! folder into a WebDAV mount.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dav_server::davpath::DavPath;
use dav_server::fs::{
    DavDirEntry, DavFile, DavFileSystem, DavMetaData, FsError, FsFuture, FsStream, OpenOptions,
    ReadDirMeta,
};
use files_proto::FileRootInfo;
use futures_util::stream;

use crate::naming::{self, RootSegment};

/// A root, or the mount point itself, as WebDAV metadata. Roots are
/// collections with no meaningful size.
#[derive(Debug, Clone)]
struct DirMeta {
    modified: SystemTime,
}

impl DavMetaData for DirMeta {
    fn len(&self) -> u64 {
        0
    }
    fn modified(&self) -> dav_server::fs::FsResult<SystemTime> {
        Ok(self.modified)
    }
    fn is_dir(&self) -> bool {
        true
    }
}

#[derive(Debug)]
struct RootEntry {
    segment: String,
    meta: DirMeta,
}

impl DavDirEntry for RootEntry {
    fn name(&self) -> Vec<u8> {
        self.segment.as_bytes().to_vec()
    }
    fn metadata(&'_ self) -> FsFuture<'_, Box<dyn DavMetaData>> {
        let meta = self.meta.clone();
        Box::pin(std::future::ready(Ok(
            Box::new(meta) as Box<dyn DavMetaData>
        )))
    }
    fn is_dir(&'_ self) -> FsFuture<'_, bool> {
        Box::pin(std::future::ready(Ok(true)))
    }
}

fn created_at(root: &FileRootInfo) -> SystemTime {
    let secs = root.created_at.timestamp();
    if secs >= 0 {
        UNIX_EPOCH + Duration::from_secs(secs as u64)
    } else {
        UNIX_EPOCH
    }
}

/// The mount's top-level collection: the visible roots, by segment.
#[derive(Debug, Clone)]
pub struct RootsFs {
    entries: Vec<RootSegment>,
}

impl RootsFs {
    pub fn new(entries: Vec<RootSegment>) -> Self {
        Self { entries }
    }

    /// The mount point's own mtime — the newest root, so a client's
    /// cache invalidates when a root appears.
    fn mount_meta(&self) -> DirMeta {
        let modified = self
            .entries
            .iter()
            .map(|e| created_at(&e.root))
            .max()
            .unwrap_or(UNIX_EPOCH);
        DirMeta { modified }
    }

    /// `""` (the mount itself) or a single segment naming a visible
    /// root. Anything deeper never reaches this filesystem — the bridge
    /// dispatches those to the root's own view — so a deeper path here
    /// means a client asked for a root that is hidden or gone.
    ///
    /// Resolution goes through [`naming::find`], the same call the
    /// bridge dispatches with: what this collection lists and what the
    /// bridge will route are the same answer by construction.
    fn lookup(&self, path: &DavPath) -> Result<Option<DirMeta>, FsError> {
        let rel = path.as_rel_ospath();
        let mut components = rel.components();
        let Some(first) = components.next() else {
            return Ok(Some(self.mount_meta()));
        };
        if components.next().is_some() {
            return Err(FsError::NotFound);
        }
        let segment = first.as_os_str().to_str().ok_or(FsError::NotFound)?;
        Ok(naming::find(&self.entries, segment).map(|e| DirMeta {
            modified: created_at(&e.root),
        }))
    }
}

fn readonly<T: Send + 'static>() -> FsFuture<'static, T> {
    Box::pin(std::future::ready(Err(FsError::Forbidden)))
}

impl DavFileSystem for RootsFs {
    fn open<'a>(
        &'a self,
        _path: &'a DavPath,
        _options: OpenOptions,
    ) -> FsFuture<'a, Box<dyn DavFile>> {
        // Every child of this collection is itself a collection, so
        // there is nothing here to open — and creating one is
        // `create_root`'s job, not a mount's.
        readonly()
    }

    fn read_dir<'a>(
        &'a self,
        path: &'a DavPath,
        _meta: ReadDirMeta,
    ) -> FsFuture<'a, FsStream<Box<dyn DavDirEntry>>> {
        Box::pin(async move {
            match self.lookup(path)? {
                // Only the mount itself has children here.
                Some(_) if path.as_rel_ospath().components().next().is_none() => {}
                Some(_) => return Ok(Box::pin(stream::empty()) as FsStream<Box<dyn DavDirEntry>>),
                None => return Err(FsError::NotFound),
            }
            let entries: Vec<Result<Box<dyn DavDirEntry>, FsError>> = self
                .entries
                .iter()
                .map(|e| {
                    Ok(Box::new(RootEntry {
                        segment: e.segment.clone(),
                        meta: DirMeta {
                            modified: created_at(&e.root),
                        },
                    }) as Box<dyn DavDirEntry>)
                })
                .collect();
            Ok(Box::pin(stream::iter(entries)) as FsStream<Box<dyn DavDirEntry>>)
        })
    }

    fn metadata<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, Box<dyn DavMetaData>> {
        Box::pin(async move {
            match self.lookup(path)? {
                Some(meta) => Ok(Box::new(meta) as Box<dyn DavMetaData>),
                None => Err(FsError::NotFound),
            }
        })
    }

    fn create_dir<'a>(&'a self, _path: &'a DavPath) -> FsFuture<'a, ()> {
        readonly()
    }

    fn remove_dir<'a>(&'a self, _path: &'a DavPath) -> FsFuture<'a, ()> {
        readonly()
    }

    fn remove_file<'a>(&'a self, _path: &'a DavPath) -> FsFuture<'a, ()> {
        readonly()
    }

    fn rename<'a>(&'a self, _from: &'a DavPath, _to: &'a DavPath) -> FsFuture<'a, ()> {
        readonly()
    }

    fn copy<'a>(&'a self, _from: &'a DavPath, _to: &'a DavPath) -> FsFuture<'a, ()> {
        readonly()
    }
}
