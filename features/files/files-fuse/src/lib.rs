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
    ReplyDirectory, ReplyEmpty, ReplyEntry, ReplyOpen, ReplyStatfs, ReplyWrite, ReplyXattr,
    Request, TimeOrNow,
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

/// Where a path's tags come from.
///
/// The filesystem does not know what a tag *is* — same reason it does
/// not know what sync is. It knows that a file manager asks for
/// `user.xdg.tags`, and it knows who to ask.
///
/// The answer is derived, not stored: a note's tags live in its
/// frontmatter, a project's org lives in the place it was given. Writing
/// them into xattrs as well would be two records of one fact, drifting
/// the moment somebody edits the note. So they are computed per query
/// and the vault stays the authority — the same trade as reporting a
/// stub at its content's size rather than at its own.
pub trait Tags: Send + Sync + 'static {
    /// Tags for `rel` within this root, most significant first. Empty
    /// when the path has none, which is the ordinary case.
    fn tags(&self, rel: &Path) -> Vec<String>;
}

/// Nothing has tags — the default, so a mount that has no use for them
/// costs nothing and says so honestly rather than inventing any.
pub struct Untagged;

impl Tags for Untagged {
    fn tags(&self, _rel: &Path) -> Vec<String> {
        Vec::new()
    }
}

/// The attribute a Linux file manager reads tags from. Dolphin and
/// Nautilus both use it; it is a comma-separated list.
const TAGS_XATTR: &str = "user.xdg.tags";

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

/// Making a folder where no root is yet.
///
/// `mkdir` inside a root is an ordinary directory in that project. One
/// at a *place* — `days-to-praise/Projects/New Song` — is somebody
/// asking for a new project, in the only vocabulary a file manager has.
/// Answering it by making a directory in the skeleton would be the
/// worst of both: it appears, and it is gone at the next mount.
///
/// So the filesystem asks whoever composed it, and that decides whether
/// a place is a project position and where its bytes should live. The
/// default refuses, which is right for a mount of a single root — there
/// are no places there to create.
pub trait Composer: Send + Sync + 'static {
    /// A directory was asked for at `place`, which no root owns. Make it
    /// a root, or say why not.
    ///
    /// `by` is whoever asked, as the kernel reports them. A project made
    /// by making a folder should still know who made it — that is what
    /// the app needs to say whose it is and who may share it, and the
    /// only moment the answer is available for free.
    fn create_root(&self, place: &Path, by: By) -> Result<Placed, String>;
}

/// Who asked, as the kernel reports them on the request.
///
/// The OS user, not a Task account: this layer has no session and no
/// business inventing one. It is enough to attribute the folder, and
/// what turns it into an account is the app's job, which has the
/// sign-in this does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct By {
    pub uid: u32,
    pub gid: u32,
    /// The process that made the folder — Finder, a shell, a DAW.
    /// Useful in a log line when somebody asks how a project appeared.
    pub pid: u32,
}

/// Nothing may be created — the default.
pub struct Fixed;

impl Composer for Fixed {
    fn create_root(&self, _place: &Path, _by: By) -> Result<Placed, String> {
        Err("this mount does not create roots".into())
    }
}

/// One root, and where it appears in the composed tree.
#[derive(Clone)]
pub struct Placed {
    /// Its path in the tree people are shown — `org/Projects/Name`.
    /// Empty for a mount of a single root at the mountpoint itself.
    pub place: String,
    /// Its live tree on disk. Nothing about this resembles `place`, and
    /// that is the point.
    pub backing: PathBuf,
    pub hydrator: Arc<dyn Hydrator>,
    pub tags: Arc<dyn Tags>,
}

/// The mounted tree.
///
/// **One mount, however many roots.** A mount per root is what this did
/// first, and on a machine holding forty-six of them a file manager
/// showed forty-six unrelated devices — the shape `place` exists to
/// replace, and the same one the macOS extension composes.
///
/// The directories *above* each root — `codywright`, `Projects` — are
/// not on anybody's disk. They are segments of the places, and the
/// skeleton is where they are given somewhere to be: an empty directory
/// tree the caller creates, one directory per place, holding nothing.
/// Everything at or below a place resolves into that root's backing
/// instead; everything above it resolves into the skeleton.
///
/// That keeps this a passthrough — no synthetic-node cases threaded
/// through twenty filesystem methods, one function that decides which
/// real directory a shown path means.
pub struct LiveTree {
    /// Where the shape of the tree lives: empty directories mirroring
    /// the places, so the parents are real to `readdir` and `stat`.
    skeleton: PathBuf,
    /// The roots, longest place first so the deepest owner of a path
    /// wins — a root placed inside another root's place answers for its
    /// own files.
    ///
    /// Behind a lock because a `mkdir` at a place adds one while the
    /// mount is live: a new project should appear in the folder
    /// somebody just made it in, not after a remount.
    roots: Mutex<Vec<Placed>>,
    /// Who decides whether a place may become a root.
    composer: Arc<dyn Composer>,
    inodes: Mutex<Inodes>,
    handles: Mutex<HashMap<u64, Arc<File>>>,
    next_fh: Mutex<u64>,
}

impl LiveTree {
    /// A filesystem mirroring `backing`, fetching through `hydrator`.
    #[must_use]
    pub fn new(backing: impl Into<PathBuf>, hydrator: Arc<dyn Hydrator>) -> Self {
        Self::tagged(backing, hydrator, Arc::new(Untagged))
    }

    /// The same, with something to answer tag queries — see [`Tags`].
    #[must_use]
    pub fn tagged(
        backing: impl Into<PathBuf>,
        hydrator: Arc<dyn Hydrator>,
        tags: Arc<dyn Tags>,
    ) -> Self {
        let backing = backing.into();
        Self::composed(
            backing.clone(),
            vec![Placed {
                place: String::new(),
                backing,
                hydrator,
                tags,
            }],
        )
    }

    /// Every root at its place, under one mount.
    ///
    /// `skeleton` must already hold an empty directory for each place —
    /// the caller knows the places and can make them; this crate is not
    /// in the business of creating directories outside the tree it
    /// serves.
    #[must_use]
    pub fn composed(skeleton: impl Into<PathBuf>, roots: Vec<Placed>) -> Self {
        Self::composing(skeleton, roots, Arc::new(Fixed))
    }

    /// The same, able to turn a `mkdir` at a place into a new root.
    #[must_use]
    pub fn composing(
        skeleton: impl Into<PathBuf>,
        mut roots: Vec<Placed>,
        composer: Arc<dyn Composer>,
    ) -> Self {
        // Longest place first: `resolve` takes the first match, and for
        // `a/b/c/take.wav` both a root at `a/b` and one at `a/b/c` are
        // prefixes. The deeper one owns it.
        roots.sort_by(|a, b| b.place.len().cmp(&a.place.len()));
        Self {
            skeleton: skeleton.into(),
            roots: Mutex::new(roots),
            composer,
            inodes: Mutex::new(Inodes::rooted(Path::new(""))),
            handles: Mutex::new(HashMap::new()),
            next_fh: Mutex::new(1),
        }
    }

    /// The root a shown path belongs to, if any.
    fn owner(&self, shown: &Path) -> Option<Placed> {
        let text = shown.to_string_lossy();
        self.roots
            .lock()
            .expect("roots lock")
            .iter()
            .find(|r| {
                r.place.is_empty()
                    || text == r.place.as_str()
                    || text.starts_with(&format!("{}/", r.place))
            })
            .cloned()
    }

    /// Is `shown` a place where a *new* root belongs?
    ///
    /// Yes when some root already sits directly inside the same parent:
    /// `days-to-praise/Projects/New Song` is a project because
    /// `days-to-praise/Projects` already holds one. That keeps the rule
    /// to something a person can predict — a new folder beside existing
    /// projects is a project, a new folder inside one is a folder — and
    /// needs no schema saying which depths mean what.
    fn is_root_position(&self, shown: &Path) -> bool {
        let Some(parent) = shown.parent().map(|p| p.to_string_lossy().into_owned()) else {
            return false;
        };
        self.roots
            .lock()
            .expect("roots lock")
            .iter()
            .any(|r| Path::new(&r.place).parent().map(|p| p.to_string_lossy().into_owned())
                == Some(parent.clone()))
    }

    /// Where a shown path actually is on disk.
    ///
    /// Inside a root, its backing; above every root, the skeleton. This
    /// is the only place the composition exists — every filesystem
    /// method below works on what this returns and needs to know
    /// nothing about places.
    fn real(&self, shown: &Path) -> PathBuf {
        match self.owner(shown) {
            Some(root) => {
                let rest = shown
                    .strip_prefix(&root.place)
                    .unwrap_or_else(|_| Path::new(""));
                root.backing.join(rest)
            }
            None => self.skeleton.join(shown),
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

    /// A shown path as its own root names it — what `hydrate` and the
    /// tag provider expect, since both speak in paths within a root.
    fn within_root(&self, shown: &Path) -> PathBuf {
        match self.owner(shown) {
            Some(root) => shown
                .strip_prefix(&root.place)
                .unwrap_or(shown)
                .to_path_buf(),
            None => shown.to_path_buf(),
        }
    }

    /// What the kernel is told about `path`.
    ///
    /// The one place this filesystem contradicts the disk, and it does
    /// so towards the truth: a stub's `size` is the content's size, not
    /// the placeholder's, because every caller asking is asking about
    /// the file they think they have.
    fn attr(&self, shown: &Path) -> std::io::Result<FileAttr> {
        use std::os::unix::fs::PermissionsExt as _;

        let path = &self.real(shown);
        let meta = std::fs::symlink_metadata(path)?;
        let ino = self.inodes.lock().expect("inode lock").for_path(shown);
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
        let Some(parent_shown) = self.inodes.lock().expect("inode lock").path(parent) else {
            reply.error(Errno::ENOENT);
            return;
        };
        match self.attr(&parent_shown.join(name)) {
            Ok(attr) => reply.entry(&TTL, &attr, fuser::Generation(0)),
            Err(e) => reply.error(errno(&e)),
        }
    }

    fn getattr(&self, _req: &Request, ino: INodeNo, _fh: Option<FileHandle>, reply: ReplyAttr) {
        let Some(shown) = self.inodes.lock().expect("inode lock").path(ino) else {
            reply.error(Errno::ENOENT);
            return;
        };
        match self.attr(&shown) {
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
        let Some(shown) = self.inodes.lock().expect("inode lock").path(ino) else {
            reply.error(Errno::ENOENT);
            return;
        };
        let dir = self.real(&shown);
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
            let child_shown = shown.join(entry.file_name());
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
            let child = self
                .inodes
                .lock()
                .expect("inode lock")
                .for_path(&child_shown);
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
        let Some(shown) = self.inodes.lock().expect("inode lock").path(ino) else {
            reply.error(Errno::ENOENT);
            return;
        };
        let path = self.real(&shown);

        if files::stub::probe(&path).is_some() {
            let rel = self.within_root(&shown);
            let Some(root) = self.owner(&shown) else {
                // Above every root there are only directories, and a
                // directory is never a stub.
                reply.error(Errno::EIO);
                return;
            };
            tracing::info!(path = %rel.display(), "fuse: fetching content for an open");
            if let Err(e) = root.hydrator.hydrate(&rel) {
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
        let Some(parent_shown) = self.inodes.lock().expect("inode lock").path(parent) else {
            reply.error(Errno::ENOENT);
            return;
        };
        let child = parent_shown.join(name);
        let path = self.real(&child);
        let mut opts = std::fs::OpenOptions::new();
        opts.create(true).read(true).write(true);
        if flags & libc::O_TRUNC != 0 {
            opts.truncate(true);
        }
        match opts.open(&path) {
            Ok(file) => match self.attr(&child) {
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
        let Some(shown) = self.inodes.lock().expect("inode lock").path(ino) else {
            reply.error(Errno::ENOENT);
            return;
        };
        let path = self.real(&shown);
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
        match self.attr(&shown) {
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
        let Some(parent_shown) = self.inodes.lock().expect("inode lock").path(parent) else {
            reply.error(Errno::ENOENT);
            return;
        };
        let child = parent_shown.join(name);

        // A folder made beside existing projects is a new project, and
        // this is where that happens. Without it the directory would be
        // created in the skeleton — visible, and gone at the next mount,
        // which is a worse answer than refusing.
        if self.owner(&child).is_none() && self.is_root_position(&child) {
            let by = By {
                uid: _req.uid(),
                gid: _req.gid(),
                pid: _req.pid(),
            };
            match self.composer.create_root(&child, by) {
                Ok(placed) => {
                    {
                        let mut roots = self.roots.lock().expect("roots lock");
                        roots.push(placed);
                        roots.sort_by(|a, b| b.place.len().cmp(&a.place.len()));
                    }
                    // The skeleton needs the parents too, or a listing
                    // of the level above will not show it.
                    let _ = std::fs::create_dir_all(self.skeleton.join(&child));
                    match self.attr(&child) {
                        Ok(attr) => reply.entry(&TTL, &attr, fuser::Generation(0)),
                        Err(e) => reply.error(errno(&e)),
                    }
                }
                Err(why) => {
                    tracing::warn!(place = %child.display(), error = %why, "fuse: could not make a project here");
                    reply.error(Errno::EIO);
                }
            }
            return;
        }

        match std::fs::create_dir(self.real(&child)).and_then(|()| self.attr(&child)) {
            Ok(attr) => reply.entry(&TTL, &attr, fuser::Generation(0)),
            Err(e) => reply.error(errno(&e)),
        }
    }

    fn unlink(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        let Some(parent_shown) = self.inodes.lock().expect("inode lock").path(parent) else {
            reply.error(Errno::ENOENT);
            return;
        };
        match std::fs::remove_file(self.real(&parent_shown.join(name))) {
            Ok(()) => reply.ok(),
            Err(e) => reply.error(errno(&e)),
        }
    }

    fn rmdir(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        let Some(parent_shown) = self.inodes.lock().expect("inode lock").path(parent) else {
            reply.error(Errno::ENOENT);
            return;
        };
        match std::fs::remove_dir(self.real(&parent_shown.join(name))) {
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
        // A rename across roots would move a file into a different tree
        // with a different history — a sync decision, not a rename, and
        // `std::fs::rename` across filesystems fails anyway. Refusing
        // here says so with EXDEV instead of a confusing errno.
        if self.owner(&from).map(|r| r.place) != self.owner(&to).map(|r| r.place) {
            reply.error(Errno::EXDEV);
            return;
        }
        match std::fs::rename(self.real(&from), self.real(&to)) {
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

    /// Read one extended attribute.
    ///
    /// `user.xdg.tags` is answered from the [`Tags`] provider when it
    /// has something to say, so a note's frontmatter and a project's org
    /// reach the file manager as tags without being written to disk
    /// twice. Everything else — and a path the provider has no tags for
    /// — passes through to the real file, so tags somebody set by hand
    /// still work.
    fn getxattr(&self, _req: &Request, ino: INodeNo, name: &OsStr, size: u32, reply: ReplyXattr) {
        let Some(shown) = self.inodes.lock().expect("inode lock").path(ino) else {
            reply.error(Errno::ENOENT);
            return;
        };
        let path = self.real(&shown);

        let derived = (name == OsStr::new(TAGS_XATTR))
            .then(|| {
                self.owner(&shown)
                    .map(|r| r.tags.tags(&self.within_root(&shown)))
                    .unwrap_or_default()
            })
            .filter(|tags| !tags.is_empty())
            .map(|tags| tags.join(",").into_bytes());

        let value = match derived {
            Some(bytes) => bytes,
            None => match xattr::get(&path, name) {
                Ok(Some(bytes)) => bytes,
                // ENODATA is the answer for "no such attribute", and it
                // is not an error a caller should see as a failure.
                Ok(None) => {
                    reply.error(Errno::ENODATA);
                    return;
                }
                Err(e) => {
                    reply.error(errno(&e));
                    return;
                }
            },
        };

        // The two-call protocol: size 0 asks how big, anything else
        // asks for the bytes and must be told ERANGE if it guessed low.
        if size == 0 {
            reply.size(value.len() as u32);
        } else if (size as usize) < value.len() {
            reply.error(Errno::ERANGE);
        } else {
            reply.data(&value);
        }
    }

    /// Which attributes a path has — the real ones, plus `user.xdg.tags`
    /// when the provider has tags for it and the file does not already
    /// carry its own.
    fn listxattr(&self, _req: &Request, ino: INodeNo, size: u32, reply: ReplyXattr) {
        let Some(shown) = self.inodes.lock().expect("inode lock").path(ino) else {
            reply.error(Errno::ENOENT);
            return;
        };
        let path = self.real(&shown);

        // NUL-terminated names, back to back — the kernel's format.
        let mut names: Vec<u8> = Vec::new();
        let mut has_tags = false;
        if let Ok(existing) = xattr::list(&path) {
            for name in existing {
                has_tags |= name == OsStr::new(TAGS_XATTR);
                names.extend_from_slice(name.as_encoded_bytes());
                names.push(0);
            }
        }
        let derived_tags = self
            .owner(&shown)
            .map(|r| r.tags.tags(&self.within_root(&shown)))
            .unwrap_or_default();
        if !has_tags && !derived_tags.is_empty() {
            names.extend_from_slice(TAGS_XATTR.as_bytes());
            names.push(0);
        }

        if size == 0 {
            reply.size(names.len() as u32);
        } else if (size as usize) < names.len() {
            reply.error(Errno::ERANGE);
        } else {
            reply.data(&names);
        }
    }

    /// Set one, straight through to the file.
    ///
    /// Tagging through the mount is an ordinary write to the tree
    /// underneath, exactly like every other write here — so a tag
    /// somebody adds in their file manager lands on the NAS and is
    /// there for every other machine that mounts it.
    fn setxattr(
        &self,
        _req: &Request,
        ino: INodeNo,
        name: &OsStr,
        value: &[u8],
        _flags: i32,
        _position: u32,
        reply: ReplyEmpty,
    ) {
        let Some(shown) = self.inodes.lock().expect("inode lock").path(ino) else {
            reply.error(Errno::ENOENT);
            return;
        };
        let path = self.real(&shown);
        match xattr::set(&path, name, value) {
            Ok(()) => reply.ok(),
            Err(e) => reply.error(errno(&e)),
        }
    }

    fn removexattr(&self, _req: &Request, ino: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        let Some(shown) = self.inodes.lock().expect("inode lock").path(ino) else {
            reply.error(Errno::ENOENT);
            return;
        };
        let path = self.real(&shown);
        match xattr::remove(&path, name) {
            Ok(()) => reply.ok(),
            Err(e) => reply.error(errno(&e)),
        }
    }

    fn statfs(&self, _req: &Request, _ino: INodeNo, reply: ReplyStatfs) {
        // Deliberately not the org's total size: writes land on the
        // disk underneath, and a mount that claimed otherwise would tell
        // a DAW it has room it does not have.
        reply.statfs(0, 0, 0, 0, 0, 512, 255, 0);
    }
}
