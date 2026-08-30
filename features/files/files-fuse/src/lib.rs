//! The live tree as a filesystem — Files' answer to the Dropbox folder.
//!
//! Selective sync already gives a machine a *complete* tree in which
//! only some content is resident: what was not subscribed sits on disk
//! as a [pointer stub](files::stub), a few hundred bytes recording the
//! content's id, its real size, and its executable bit. Browsing works,
//! sizes are honest, and nothing is hidden.
//!
//! What was missing is the last step everybody expects from a cloud
//! folder: **opening a file you do not have gets you the file**. Until
//! now a DAW that opened a dehydrated take read the stub's bytes — a
//! line of text where two gigabytes of audio should be — and hydration
//! was something a person had to ask for by name, in another program.
//!
//! This is the seam where that stops being true. The kernel routes every
//! open through here, so a stub is fetched *before* the read that would
//! have failed, and the caller waits exactly as it would for a slow
//! disk.
//!
//! # A passthrough, deliberately
//!
//! It does not serve the tree from the version store. The live tree is
//! already on disk — that is what materialize produces — so this mirrors
//! the directory underneath and changes two answers:
//!
//! - `getattr` on a stub reports the content's real size and mode rather
//!   than the placeholder's, so `ls -l`, a file picker and a DAW asking
//!   "how big is this" all get the truth.
//! - `open` on a stub hydrates first, and only then hands back a handle.
//!
//! Everything else — writes, renames, mkdir — passes straight through to
//! the underlying file, which is what keeps the cadence engine, the
//! watcher and the checkpoint path working exactly as they do on an
//! unmounted tree. A write here is an ordinary write, the same claim the
//! WebDAV bridge makes and for the same reason.
//!
//! # What a caller feels
//!
//! An open that has to fetch blocks until the content is there. That is
//! the honest behaviour for a filesystem — `open(2)` has no way to say
//! "one moment" — and it is what every cloud filesystem does.
//!
//! It does not block the *mount*, though, which is the part worth
//! getting right: the session runs several worker threads and this
//! filesystem takes `&self`, so a forty-gigabyte fetch on one thread
//! leaves the others answering. A person browsing the tree while a take
//! downloads sees the tree.

#![cfg(target_os = "linux")]

use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The handle a live mount is kept alive by: hold it and the mount
/// stands, drop it and the filesystem is unmounted. Re-exported so an
/// embedder can name the type without depending on `fuser` itself —
/// which crate holds the FUSE dependency is this crate's business.
pub use fuser::BackgroundSession;

use fuser::{
    BsdFileFlags, Config, Errno, FileAttr, FileHandle, FileType, Filesystem, FopenFlags, INodeNo,
    LockOwner, MountOption, OpenFlags, RenameFlags, ReplyAttr, ReplyCreate, ReplyData,
    ReplyDirectory, ReplyEmpty, ReplyEntry, ReplyOpen, ReplyStatfs, ReplyWrite, Request, TimeOrNow,
    WriteFlags,
};

/// How long the kernel may trust what we told it.
///
/// Short, because the tree changes underneath us: a sync pull
/// materializes new content, and a cache that outlived the pull would
/// serve a stub's attributes for a file that is now resident. One second
/// costs a stat per second per open file and keeps the mount honest.
const TTL: Duration = Duration::from_secs(1);

/// Fetching content for a file something is trying to open.
///
/// A trait so this crate does not depend on the sync agent: the agent
/// implements it over its own backend, and a test implements it with a
/// closure. What arrives is the path *relative to the mounted root*,
/// which is how `FilesService::hydrate` names a file.
pub trait Hydrator: Send + Sync + 'static {
    /// Make `rel` resident, blocking until it is. Returning `Ok` means
    /// the content is on disk and the next read will find it.
    fn hydrate(&self, rel: &Path) -> Result<(), String>;
}

impl<F> Hydrator for F
where
    F: Fn(&Path) -> Result<(), String> + Send + Sync + 'static,
{
    fn hydrate(&self, rel: &Path) -> Result<(), String> {
        self(rel)
    }
}

/// Inode ↔ path.
///
/// A passthrough has no inode of its own to hand out, so it mints them
/// and remembers what each one meant; the kernel only ever asks about
/// inodes it has been given.
#[derive(Default)]
struct Inodes {
    paths: HashMap<u64, PathBuf>,
    inodes: HashMap<PathBuf, u64>,
    next: u64,
}

impl Inodes {
    fn rooted(at: &Path) -> Self {
        let root = u64::from(INodeNo::ROOT);
        Self {
            paths: HashMap::from([(root, at.to_path_buf())]),
            inodes: HashMap::from([(at.to_path_buf(), root)]),
            next: root + 1,
        }
    }

    fn path(&self, ino: INodeNo) -> Option<PathBuf> {
        self.paths.get(&u64::from(ino)).cloned()
    }

    fn for_path(&mut self, path: &Path) -> INodeNo {
        if let Some(ino) = self.inodes.get(path) {
            return INodeNo(*ino);
        }
        let ino = self.next;
        self.next += 1;
        self.paths.insert(ino, path.to_path_buf());
        self.inodes.insert(path.to_path_buf(), ino);
        INodeNo(ino)
    }

    /// Follow a rename: the same file under a new name, so the inode
    /// goes with it. A stale map would answer about the old path
    /// forever.
    fn renamed(&mut self, from: &Path, to: &Path) {
        if let Some(ino) = self.inodes.remove(from) {
            self.paths.insert(ino, to.to_path_buf());
            self.inodes.insert(to.to_path_buf(), ino);
        }
    }
}

/// The mounted tree.
pub struct LiveTree {
    /// The directory this mirrors — the root's live tree.
    backing: PathBuf,
    hydrator: Arc<dyn Hydrator>,
    inodes: Mutex<Inodes>,
    handles: Mutex<HashMap<u64, Arc<File>>>,
    next_fh: Mutex<u64>,
}

impl LiveTree {
    /// A filesystem mirroring `backing`, fetching through `hydrator`.
    #[must_use]
    pub fn new(backing: impl Into<PathBuf>, hydrator: Arc<dyn Hydrator>) -> Self {
        let backing = backing.into();
        Self {
            inodes: Mutex::new(Inodes::rooted(&backing)),
            backing,
            hydrator,
            handles: Mutex::new(HashMap::new()),
            next_fh: Mutex::new(1),
        }
    }

    /// Mount at `mountpoint`, serving on background threads.
    ///
    /// The returned handle unmounts on drop, which is what makes a
    /// mount's lifetime the caller's to manage — an agent that exits
    /// should not leave a dead mount behind for somebody to clear up
    /// with `fusermount -u`.
    pub fn mount(self, mountpoint: &Path) -> std::io::Result<fuser::BackgroundSession> {
        let mut config = Config::default();
        config.mount_options = vec![
            MountOption::FSName("task-files".into()),
            MountOption::Subtype("taskfiles".into()),
            MountOption::NoAtime,
            MountOption::DefaultPermissions,
        ];
        // Several, so a long fetch on one request does not stop the
        // mount answering others — see the module docs.
        config.n_threads = Some(4);
        config.clone_fd = true;
        fuser::spawn_mount(self, mountpoint, &config)
    }

    /// This path relative to the mount root — how `hydrate` names it.
    fn relative(&self, path: &Path) -> PathBuf {
        path.strip_prefix(&self.backing)
            .unwrap_or(path)
            .to_path_buf()
    }

    /// What the kernel is told about `path`.
    ///
    /// The one place this filesystem contradicts the disk, and it does
    /// so towards the truth: a stub's `size` is the content's size, not
    /// the placeholder's, because every caller asking is asking about
    /// the file they think they have.
    fn attr(&self, path: &Path) -> std::io::Result<FileAttr> {
        use std::os::unix::fs::PermissionsExt as _;

        let meta = std::fs::symlink_metadata(path)?;
        let ino = self.inodes.lock().expect("inode lock").for_path(path);
        let kind = if meta.is_dir() {
            FileType::Directory
        } else if meta.file_type().is_symlink() {
            FileType::Symlink
        } else {
            FileType::RegularFile
        };

        let stub = (kind == FileType::RegularFile)
            .then(|| files::stub::probe(path))
            .flatten();
        let (size, executable) = match &stub {
            Some(s) => (s.size, s.executable),
            None => (meta.len(), meta.permissions().mode() & 0o111 != 0),
        };
        let perm = match kind {
            FileType::Directory => 0o755,
            _ if executable => 0o755,
            _ => 0o644,
        };

        Ok(FileAttr {
            ino,
            size,
            // Blocks follow the *logical* size for the same reason: `du`
            // on a dehydrated tree should say what the project weighs,
            // not what this machine happens to be holding.
            blocks: size.div_ceil(512),
            atime: meta.accessed().unwrap_or(UNIX_EPOCH),
            mtime: meta.modified().unwrap_or(UNIX_EPOCH),
            ctime: meta.modified().unwrap_or(UNIX_EPOCH),
            crtime: meta.created().unwrap_or(UNIX_EPOCH),
            kind,
            perm,
            nlink: if kind == FileType::Directory { 2 } else { 1 },
            uid: unsafe { libc::getuid() },
            gid: unsafe { libc::getgid() },
            rdev: 0,
            blksize: 512,
            flags: 0,
        })
    }

    fn handle(&self, fh: FileHandle) -> Option<Arc<File>> {
        self.handles
            .lock()
            .expect("handle lock")
            .get(&u64::from(fh))
            .cloned()
    }

    fn keep(&self, file: File) -> FileHandle {
        let mut next = self.next_fh.lock().expect("fh lock");
        let fh = *next;
        *next += 1;
        drop(next);
        self.handles
            .lock()
            .expect("handle lock")
            .insert(fh, Arc::new(file));
        FileHandle(fh)
    }
}

/// The errno an I/O error carries, defaulting to EIO.
fn errno(e: &std::io::Error) -> Errno {
    Errno::from_i32(e.raw_os_error().unwrap_or(libc::EIO))
}

impl Filesystem for LiveTree {
    fn lookup(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEntry) {
        let Some(dir) = self.inodes.lock().expect("inode lock").path(parent) else {
            reply.error(Errno::ENOENT);
            return;
        };
        match self.attr(&dir.join(name)) {
            Ok(attr) => reply.entry(&TTL, &attr, fuser::Generation(0)),
            Err(e) => reply.error(errno(&e)),
        }
    }

    fn getattr(&self, _req: &Request, ino: INodeNo, _fh: Option<FileHandle>, reply: ReplyAttr) {
        let Some(path) = self.inodes.lock().expect("inode lock").path(ino) else {
            reply.error(Errno::ENOENT);
            return;
        };
        match self.attr(&path) {
            Ok(attr) => reply.attr(&TTL, &attr),
            Err(e) => reply.error(errno(&e)),
        }
    }

    fn readdir(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        offset: u64,
        mut reply: ReplyDirectory,
    ) {
        let Some(dir) = self.inodes.lock().expect("inode lock").path(ino) else {
            reply.error(Errno::ENOENT);
            return;
        };
        let listing = match std::fs::read_dir(&dir) {
            Ok(listing) => listing,
            Err(e) => {
                reply.error(errno(&e));
                return;
            }
        };

        let mut entries: Vec<(INodeNo, FileType, String)> = vec![
            (ino, FileType::Directory, ".".into()),
            (ino, FileType::Directory, "..".into()),
        ];
        for entry in listing.flatten() {
            let path = entry.path();
            // The root's own machinery is not part of the tree somebody
            // mounted — the same hiding the WebDAV bridge does.
            if matches!(
                path.file_name().and_then(|n| n.to_str()),
                Some(".fts-files" | ".fts-root.json")
            ) {
                continue;
            }
            let kind = match entry.file_type() {
                Ok(t) if t.is_dir() => FileType::Directory,
                Ok(t) if t.is_symlink() => FileType::Symlink,
                Ok(_) => FileType::RegularFile,
                Err(_) => continue,
            };
            let child = self.inodes.lock().expect("inode lock").for_path(&path);
            entries.push((child, kind, entry.file_name().to_string_lossy().into_owned()));
        }

        for (i, (child, kind, name)) in entries.into_iter().enumerate().skip(offset as usize) {
            if reply.add(child, (i + 1) as u64, kind, name) {
                break;
            }
        }
        reply.ok();
    }

    /// The whole point of this filesystem.
    ///
    /// A dehydrated file is fetched here, before the handle exists — so
    /// the read that follows finds content, and a caller that cannot be
    /// told "wait" simply waits.
    fn open(&self, _req: &Request, ino: INodeNo, flags: OpenFlags, reply: ReplyOpen) {
        let Some(path) = self.inodes.lock().expect("inode lock").path(ino) else {
            reply.error(Errno::ENOENT);
            return;
        };

        if files::stub::probe(&path).is_some() {
            let rel = self.relative(&path);
            tracing::info!(path = %rel.display(), "fuse: fetching content for an open");
            if let Err(e) = self.hydrator.hydrate(&rel) {
                tracing::warn!(path = %rel.display(), error = %e, "fuse: could not fetch it");
                // EIO rather than ENOENT: the file exists and is named
                // in the tree; what failed is getting its bytes, which
                // is an I/O failure and reads as one.
                reply.error(Errno::EIO);
                return;
            }
        }

        let raw = flags.0;
        let accmode = raw & libc::O_ACCMODE;
        let mut opts = std::fs::OpenOptions::new();
        opts.read(accmode == libc::O_RDONLY || accmode == libc::O_RDWR);
        opts.write(accmode == libc::O_WRONLY || accmode == libc::O_RDWR);
        if raw & libc::O_APPEND != 0 {
            opts.append(true);
        }
        if raw & libc::O_TRUNC != 0 {
            opts.truncate(true);
        }
        match opts.open(&path) {
            Ok(file) => reply.opened(self.keep(file), FopenFlags::empty()),
            Err(e) => reply.error(errno(&e)),
        }
    }

    fn read(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        offset: u64,
        size: u32,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyData,
    ) {
        use std::os::unix::fs::FileExt as _;
        let Some(file) = self.handle(fh) else {
            reply.error(Errno::EBADF);
            return;
        };
        let mut buf = vec![0u8; size as usize];
        match file.read_at(&mut buf, offset) {
            Ok(n) => {
                buf.truncate(n);
                reply.data(&buf);
            }
            Err(e) => reply.error(errno(&e)),
        }
    }

    fn write(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        offset: u64,
        data: &[u8],
        _write_flags: WriteFlags,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyWrite,
    ) {
        use std::os::unix::fs::FileExt as _;
        let Some(file) = self.handle(fh) else {
            reply.error(Errno::EBADF);
            return;
        };
        match file.write_at(data, offset) {
            Ok(n) => reply.written(n as u32),
            Err(e) => reply.error(errno(&e)),
        }
    }

    fn create(
        &self,
        _req: &Request,
        parent: INodeNo,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        flags: i32,
        reply: ReplyCreate,
    ) {
        let Some(dir) = self.inodes.lock().expect("inode lock").path(parent) else {
            reply.error(Errno::ENOENT);
            return;
        };
        let path = dir.join(name);
        let mut opts = std::fs::OpenOptions::new();
        opts.create(true).read(true).write(true);
        if flags & libc::O_TRUNC != 0 {
            opts.truncate(true);
        }
        match opts.open(&path) {
            Ok(file) => match self.attr(&path) {
                Ok(attr) => reply.created(&TTL, &attr, fuser::Generation(0), self.keep(file), FopenFlags::empty()),
                Err(e) => reply.error(errno(&e)),
            },
            Err(e) => reply.error(errno(&e)),
        }
    }

    fn setattr(
        &self,
        _req: &Request,
        ino: INodeNo,
        _mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
        size: Option<u64>,
        _atime: Option<TimeOrNow>,
        _mtime: Option<TimeOrNow>,
        _ctime: Option<SystemTime>,
        _fh: Option<FileHandle>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<BsdFileFlags>,
        reply: ReplyAttr,
    ) {
        let Some(path) = self.inodes.lock().expect("inode lock").path(ino) else {
            reply.error(Errno::ENOENT);
            return;
        };
        // Truncation is the one a DAW actually issues (open with
        // O_TRUNC, or ftruncate before a rewrite); the rest are accepted
        // and reported back, which is what a passthrough over one
        // person's own files can honestly do.
        if let Some(size) = size {
            if let Err(e) = std::fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .and_then(|f| f.set_len(size))
            {
                reply.error(errno(&e));
                return;
            }
        }
        match self.attr(&path) {
            Ok(attr) => reply.attr(&TTL, &attr),
            Err(e) => reply.error(errno(&e)),
        }
    }

    fn mkdir(
        &self,
        _req: &Request,
        parent: INodeNo,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        reply: ReplyEntry,
    ) {
        let Some(dir) = self.inodes.lock().expect("inode lock").path(parent) else {
            reply.error(Errno::ENOENT);
            return;
        };
        let path = dir.join(name);
        match std::fs::create_dir(&path).and_then(|()| self.attr(&path)) {
            Ok(attr) => reply.entry(&TTL, &attr, fuser::Generation(0)),
            Err(e) => reply.error(errno(&e)),
        }
    }

    fn unlink(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        let Some(dir) = self.inodes.lock().expect("inode lock").path(parent) else {
            reply.error(Errno::ENOENT);
            return;
        };
        match std::fs::remove_file(dir.join(name)) {
            Ok(()) => reply.ok(),
            Err(e) => reply.error(errno(&e)),
        }
    }

    fn rmdir(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        let Some(dir) = self.inodes.lock().expect("inode lock").path(parent) else {
            reply.error(Errno::ENOENT);
            return;
        };
        match std::fs::remove_dir(dir.join(name)) {
            Ok(()) => reply.ok(),
            Err(e) => reply.error(errno(&e)),
        }
    }

    fn rename(
        &self,
        _req: &Request,
        parent: INodeNo,
        name: &OsStr,
        newparent: INodeNo,
        newname: &OsStr,
        _flags: RenameFlags,
        reply: ReplyEmpty,
    ) {
        let (from_dir, to_dir) = {
            let inodes = self.inodes.lock().expect("inode lock");
            (inodes.path(parent), inodes.path(newparent))
        };
        let (Some(from_dir), Some(to_dir)) = (from_dir, to_dir) else {
            reply.error(Errno::ENOENT);
            return;
        };
        let (from, to) = (from_dir.join(name), to_dir.join(newname));
        match std::fs::rename(&from, &to) {
            Ok(()) => {
                self.inodes.lock().expect("inode lock").renamed(&from, &to);
                reply.ok();
            }
            Err(e) => reply.error(errno(&e)),
        }
    }

    fn release(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        self.handles
            .lock()
            .expect("handle lock")
            .remove(&u64::from(fh));
        reply.ok();
    }

    fn fsync(&self, _req: &Request, _ino: INodeNo, fh: FileHandle, _datasync: bool, reply: ReplyEmpty) {
        match self.handle(fh).map(|f| f.sync_all()) {
            Some(Ok(())) => reply.ok(),
            Some(Err(e)) => reply.error(errno(&e)),
            None => reply.error(Errno::EBADF),
        }
    }

    fn statfs(&self, _req: &Request, _ino: INodeNo, reply: ReplyStatfs) {
        // Deliberately not the org's total size: writes land on the
        // disk underneath, and a mount that claimed otherwise would tell
        // a DAW it has room it does not have.
        reply.statfs(0, 0, 0, 0, 0, 512, 255, 0);
    }
}
