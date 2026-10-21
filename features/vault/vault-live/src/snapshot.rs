//! A cached [`Vault`] snapshot per vault root (zero-copy audit V12, V13).
//!
//! Every entity list (tasks, goals, milestones, workstreams, file
//! versions) and every search used to call [`Vault::open`], which reads
//! every page of the vault from disk. [`snapshot`] keeps the last
//! snapshot of each root in memory and hands out an `Arc` of it; a call
//! re-reads only the files that changed since.
//!
//! # Freshness
//!
//! The cache must never answer with text older than the disk: a task
//! written by one MCP call is listed by the next one, a millisecond
//! later, long before a debounced watcher has said anything. So the
//! watcher is not what keeps the cache correct; a stat walk is:
//!
//! 1. Every call walks the root (names only, as [`Vault::open`] did) and
//!    `stat`s each page: length, mtime and, on Unix, inode and ctime.
//!    A page whose stamp matches the one it was read with is reused (an
//!    `Arc` bump, no read, no copy); any other page is read again. A
//!    page that is gone is dropped, a new one is read.
//! 2. **Racily clean pages are not trusted** (git's rule for its
//!    index). A file modified in the same timestamp tick it was read in
//!    can be rewritten, same length, without its stamp changing. So a
//!    page is reused only if it was read at least [`RACY`] after its
//!    mtime; one read sooner is read again on the next call, until a
//!    read lands after the window.
//! 3. The vault watcher ([`crate::sync::Backend::start_watcher`])
//!    calls [`invalidate_path`] for every file it reports, so an edit
//!    the stamp cannot see (a filesystem with coarse timestamps, a tool
//!    that restores mtimes) is still read again once the watcher has
//!    seen it. [`invalidate`] drops a whole root.
//!
//! When nothing changed, the call returns the very `Arc` it returned
//! last time: no read, no page copy, no allocation beyond the walk.
//!
//! # Sharing (V13)
//!
//! [`crate::VaultPage::raw`] and [`crate::VaultBase::raw`] are
//! `Arc<str>`: a snapshot rebuilt after one page changed shares every
//! other page's text with the previous one, and cloning a [`Vault`]
//! no longer copies the vault.
//!
//! # Memory
//!
//! The cache holds at most [`MAX_CACHED_BYTES`] of page text across
//! all roots; past that, the least recently used roots are dropped
//! (their next call is a cold [`Vault::open`] again). A dropped root's
//! snapshot lives on for as long as a caller still holds its `Arc`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use rayon::prelude::*;

use crate::vault::{LoadError, LoadedEntry, Vault, VaultBase, VaultPage};
use crate::walker::{VaultEntry, walk_vault};

/// A page modified this close to the moment it was read is "racily
/// clean": a same-length rewrite inside one timestamp tick would leave
/// its stamp unchanged, so it is read again until a read lands later
/// than this after its mtime. Two seconds covers the coarsest
/// timestamps a vault is likely to live on (FAT and HFS+, one or two
/// seconds); on a nanosecond filesystem it only means a page written in
/// the last two seconds is read once more.
pub const RACY: Duration = Duration::from_secs(2);

/// Page text the cache keeps across all roots before it drops the
/// least recently used ones.
pub const MAX_CACHED_BYTES: usize = 64 * 1024 * 1024;

/// The snapshot of the vault at `root`, re-reading only what changed
/// since the last call for the same root. See the module docs for why
/// it is never staler than the disk.
///
/// # Errors
///
/// As [`Vault::open`] (which today never fails: an unreadable file is
/// skipped, a missing root is an empty vault).
pub fn snapshot(root: &Path) -> Result<Arc<Vault>, LoadError> {
    let slot = CACHE.slot(root);
    let snap = {
        let mut slot = lock(&slot);
        slot.last_used = Instant::now();
        slot.refresh(root)?
    };
    CACHE.evict_over_budget(root);
    Ok(snap)
}

/// Forget the cached snapshot of `root` (and of every cached root under
/// it): its next [`snapshot`] reads the vault from scratch.
pub fn invalidate(root: &Path) {
    CACHE.for_each_under(root, |slot| {
        slot.vault = None;
        slot.pages.clear();
        slot.dirty.clear();
    });
}

/// The watcher saw `abs_path` change: whichever cached root holds it
/// reads that file again on its next [`snapshot`], whatever its stamp
/// says. A path under no cached root is ignored.
pub fn invalidate_path(abs_path: &Path) {
    CACHE.for_each_holding(abs_path, |slot, rel| {
        slot.dirty.insert(rel);
    });
}

/// What the cache currently holds, for tests and diagnostics: the number
/// of roots and their page text in bytes.
#[must_use]
pub fn cached_bytes() -> (usize, usize) {
    let roots = CACHE.roots();
    let bytes = roots.iter().map(|s| lock(s).bytes).sum();
    (roots.len(), bytes)
}

static CACHE: LazyLock<Cache> = LazyLock::new(Cache::default);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Every cached root. The outer lock is held only to find or make a
/// root's slot; a refresh holds that root's own lock, so two roots
/// refresh in parallel and two callers of one root share one refresh.
#[derive(Default)]
struct Cache {
    roots: Mutex<HashMap<PathBuf, Arc<Mutex<Slot>>>>,
}

impl Cache {
    fn slot(&self, root: &Path) -> Arc<Mutex<Slot>> {
        let mut roots = lock(&self.roots);
        if let Some(slot) = roots.get(root) {
            return Arc::clone(slot);
        }
        let slot = Arc::new(Mutex::new(Slot::new(root)));
        roots.insert(root.to_path_buf(), Arc::clone(&slot));
        slot
    }

    fn roots(&self) -> Vec<Arc<Mutex<Slot>>> {
        lock(&self.roots).values().cloned().collect()
    }

    fn for_each_under(&self, root: &Path, mut f: impl FnMut(&mut Slot)) {
        let canonical = std::fs::canonicalize(root).ok();
        for slot in self.roots() {
            let mut slot = lock(&slot);
            let hit = slot.root.starts_with(root)
                || canonical
                    .as_deref()
                    .is_some_and(|c| slot.canonical.starts_with(c));
            if hit {
                f(&mut slot);
            }
        }
    }

    fn for_each_holding(&self, abs: &Path, mut f: impl FnMut(&mut Slot, String)) {
        for slot in self.roots() {
            let mut slot = lock(&slot);
            let rel = abs
                .strip_prefix(&slot.root)
                .or_else(|_| abs.strip_prefix(&slot.canonical));
            if let Ok(rel) = rel {
                let rel = crate::walker::rel_string(rel);
                f(&mut slot, rel);
            }
        }
    }

    /// Drop least recently used roots until the cache is under
    /// [`MAX_CACHED_BYTES`], never `keep` (the root just served).
    fn evict_over_budget(&self, keep: &Path) {
        let mut roots = lock(&self.roots);
        let mut sizes: Vec<(PathBuf, usize, Instant)> = roots
            .iter()
            .map(|(root, slot)| {
                let slot = lock(slot);
                (root.clone(), slot.bytes, slot.last_used)
            })
            .collect();
        let mut total: usize = sizes.iter().map(|(_, bytes, _)| bytes).sum();
        if total <= MAX_CACHED_BYTES {
            return;
        }
        sizes.sort_unstable_by_key(|(_, _, used)| *used);
        for (root, bytes, _) in sizes {
            if total <= MAX_CACHED_BYTES {
                break;
            }
            if root == keep {
                continue;
            }
            roots.remove(&root);
            total -= bytes;
        }
    }
}

/// What a page was read with: if any of it differs now, the file
/// changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stamp {
    len: u64,
    mtime: SystemTime,
    #[cfg(unix)]
    ino: u64,
    #[cfg(unix)]
    ctime: (i64, i64),
}

impl Stamp {
    fn of(meta: &std::fs::Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt as _;
        Self {
            len: meta.len(),
            mtime: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            #[cfg(unix)]
            ino: meta.ino(),
            #[cfg(unix)]
            ctime: (meta.ctime(), meta.ctime_nsec()),
        }
    }

    /// Whether a read at `read_at` can be trusted while the stamp
    /// stays the same (see [`RACY`]).
    fn settled_at(&self, read_at: SystemTime) -> bool {
        self.mtime
            .checked_add(RACY)
            .is_some_and(|settled| settled <= read_at)
    }
}

/// One cached file and what it was read with.
struct Cached {
    entry: CachedEntry,
    stamp: Stamp,
    /// Whether the stamp alone may vouch for this read (see [`RACY`]).
    trusted: bool,
}

#[derive(Clone)]
enum CachedEntry {
    Page(VaultPage),
    Base(VaultBase),
}

impl CachedEntry {
    /// Whether a fresh read says exactly what this one did (text and
    /// mtime; a base's parse follows from its text).
    fn same_as(&self, fresh: &Self) -> bool {
        match (self, fresh) {
            (Self::Page(a), Self::Page(b)) => a.mtime == b.mtime && a.raw == b.raw,
            (Self::Base(a), Self::Base(b)) => a.mtime == b.mtime && a.raw == b.raw,
            _ => false,
        }
    }

    fn bytes(&self) -> usize {
        match self {
            Self::Page(p) => p.raw.len(),
            Self::Base(b) => b.raw.len(),
        }
    }
}

struct Slot {
    root: PathBuf,
    /// `root` as the filesystem names it, for watcher paths that come
    /// back canonicalized (macOS: `/private/var` for `/var`).
    canonical: PathBuf,
    /// The snapshot last handed out, returned as is while nothing
    /// changes.
    vault: Option<Arc<Vault>>,
    /// Every file in `vault`, by vault-relative path.
    pages: HashMap<String, Cached>,
    /// Files the watcher reported since the last refresh.
    dirty: HashSet<String>,
    bytes: usize,
    last_used: Instant,
}

impl Slot {
    fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            canonical: std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf()),
            vault: None,
            pages: HashMap::new(),
            dirty: HashSet::new(),
            bytes: 0,
            last_used: Instant::now(),
        }
    }

    /// Bring the snapshot up to date with the disk and return it.
    fn refresh(&mut self, root: &Path) -> Result<Arc<Vault>, LoadError> {
        let entries = walk_vault(root);
        let dirty = std::mem::take(&mut self.dirty);
        let mut previous = std::mem::take(&mut self.pages);
        let mut changed = previous.len() != entries.len();

        // Stat every file; keep the ones whose stamp vouches for the
        // text we hold, queue the rest for a read.
        let mut order: Vec<String> = Vec::with_capacity(entries.len());
        let mut kept: HashMap<String, Cached> = HashMap::with_capacity(entries.len());
        let mut to_read: Vec<(VaultEntry, Stamp, Option<Cached>)> = Vec::new();
        for entry in entries {
            let Ok(meta) = std::fs::metadata(&entry.abs_path) else {
                continue;
            };
            let stamp = Stamp::of(&meta);
            let prev = previous.remove(&entry.rel_path);
            order.push(entry.rel_path.clone());
            match prev {
                Some(cached)
                    if cached.trusted
                        && cached.stamp == stamp
                        && !dirty.contains(&entry.rel_path) =>
                {
                    kept.insert(entry.rel_path, cached);
                }
                prev => to_read.push((entry, stamp, prev)),
            }
        }
        // Whatever is left in `previous` is gone from disk.
        changed |= !previous.is_empty();

        let read_at = SystemTime::now();
        let read: Vec<(String, Cached, bool)> = to_read
            .into_par_iter()
            .filter_map(|(entry, stamp, prev)| {
                let rel = entry.rel_path.clone();
                let fresh = match crate::vault::load_entry(entry).ok()? {
                    LoadedEntry::Page(p) => CachedEntry::Page(p),
                    LoadedEntry::Base(b) => CachedEntry::Base(b),
                };
                // A page read again only to be sure (racily clean, or
                // reported by the watcher) that turns out unchanged keeps
                // its old text, so the snapshot does not change either.
                let (entry, same) = match prev {
                    Some(prev) if prev.entry.same_as(&fresh) => (prev.entry, true),
                    _ => (fresh, false),
                };
                Some((
                    rel,
                    Cached {
                        entry,
                        stamp,
                        trusted: stamp.settled_at(read_at),
                    },
                    same,
                ))
            })
            .collect();
        for (rel, cached, same) in read {
            changed |= !same;
            kept.insert(rel, cached);
        }
        // A file that vanished between the walk and its read.
        changed |= kept.len() != order.len();

        let property_types = crate::vault::read_property_types(root);
        if let Some(vault) = &self.vault
            && !changed
            && vault.property_types == property_types
        {
            self.pages = kept;
            return Ok(Arc::clone(vault));
        }

        // Rebuild in walk order, as `Vault::open` would have: each page
        // is an `Arc` bump on its text, never a copy of it.
        let mut pages = Vec::with_capacity(order.len());
        let mut bases = Vec::new();
        let mut bytes = 0;
        for rel in &order {
            let Some(cached) = kept.get(rel) else {
                continue;
            };
            bytes += cached.entry.bytes();
            match &cached.entry {
                CachedEntry::Page(p) => pages.push(p.clone()),
                CachedEntry::Base(b) => bases.push(b.clone()),
            }
        }
        let vault = Arc::new(Vault {
            root: root.to_path_buf(),
            pages,
            bases,
            property_types,
        });
        self.pages = kept;
        self.bytes = bytes;
        self.vault = Some(Arc::clone(&vault));
        Ok(vault)
    }
}

/// Snapshot statistics for one root, for the tests below.
#[cfg(test)]
fn is_cached(root: &Path) -> bool {
    lock(&CACHE.slot(root)).vault.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, rel: &str, text: &str) {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, text).unwrap();
    }

    fn texts(vault: &Vault) -> Vec<(String, String)> {
        let mut out: Vec<_> = vault
            .pages
            .iter()
            .map(|p| (p.rel_path.clone(), p.raw.to_string()))
            .collect();
        out.sort_unstable();
        out
    }

    /// The snapshot reads what `Vault::open` reads, pages and bases.
    #[test]
    fn snapshot_matches_open() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.md", "alpha");
        write(dir.path(), "Notes/b.md", "beta");
        write(dir.path(), "Views/T.base", "views: []\n");
        let snap = snapshot(dir.path()).unwrap();
        let open = Vault::open(dir.path()).unwrap();
        assert_eq!(texts(&snap), texts(&open));
        assert_eq!(snap.bases.len(), open.bases.len());
        assert!(is_cached(dir.path()));
    }

    /// Nothing changed: the same `Arc`, no rebuild, whether the page is
    /// settled (trusted on its stamp) or was just written (read again,
    /// found equal).
    #[test]
    fn unchanged_vault_returns_the_same_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.md", "alpha");
        let first = snapshot(dir.path()).unwrap();
        let again = snapshot(dir.path()).unwrap();
        assert!(Arc::ptr_eq(&first, &again));
        std::fs::File::options()
            .write(true)
            .open(dir.path().join("a.md"))
            .unwrap()
            .set_modified(SystemTime::now() - RACY * 5)
            .unwrap();
        let first = snapshot(dir.path()).unwrap();
        let second = snapshot(dir.path()).unwrap();
        assert!(Arc::ptr_eq(&first, &second));
    }

    /// A write is seen by the very next call, with no watcher and no
    /// wait, even a same-length rewrite inside one timestamp tick (the
    /// racily-clean rule), and an added and a removed page too.
    #[test]
    fn writes_are_seen_immediately() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.md", "todo");
        write(dir.path(), "gone.md", "bye");
        let _ = snapshot(dir.path()).unwrap();
        write(dir.path(), "a.md", "done");
        write(dir.path(), "new.md", "hello");
        std::fs::remove_file(dir.path().join("gone.md")).unwrap();
        let snap = snapshot(dir.path()).unwrap();
        assert_eq!(
            texts(&snap),
            vec![
                ("a.md".to_owned(), "done".to_owned()),
                ("new.md".to_owned(), "hello".to_owned()),
            ]
        );
    }

    /// A settled page is shared, not copied, into the next snapshot when
    /// another page changes (V13).
    #[test]
    fn unchanged_pages_share_their_text_across_snapshots() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "old.md", "settled text");
        // Old enough to be trusted on its stamp alone.
        let past = SystemTime::now() - RACY * 5;
        std::fs::File::options()
            .write(true)
            .open(dir.path().join("old.md"))
            .unwrap()
            .set_modified(past)
            .unwrap();
        let first = snapshot(dir.path()).unwrap();
        write(dir.path(), "fresh.md", "x");
        let second = snapshot(dir.path()).unwrap();
        assert!(!Arc::ptr_eq(&first, &second));
        let text =
            |v: &Vault| Arc::clone(&v.pages.iter().find(|p| p.rel_path == "old.md").unwrap().raw);
        assert!(Arc::ptr_eq(&text(&first), &text(&second)));
    }

    /// A page the watcher reported is read again even though its stamp
    /// did not move (a tool that restored the mtime).
    #[test]
    fn invalidate_path_forces_a_read() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("p.md");
        write(dir.path(), "p.md", "aaaa");
        let past = SystemTime::now() - RACY * 5;
        let set_past = || {
            std::fs::File::options()
                .write(true)
                .open(&file)
                .unwrap()
                .set_modified(past)
                .unwrap();
        };
        set_past();
        let _ = snapshot(dir.path()).unwrap();
        // Same length, mtime put back. On Unix the ctime still moves;
        // the watcher's word is what this test pins either way.
        std::fs::write(&file, "bbbb").unwrap();
        set_past();
        invalidate_path(&file);
        let snap = snapshot(dir.path()).unwrap();
        assert_eq!(texts(&snap), vec![("p.md".to_owned(), "bbbb".to_owned())]);

        invalidate(dir.path());
        assert!(!is_cached(dir.path()));
        let snap = snapshot(dir.path()).unwrap();
        assert_eq!(texts(&snap), vec![("p.md".to_owned(), "bbbb".to_owned())]);
    }
}
