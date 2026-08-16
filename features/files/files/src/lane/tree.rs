//! `TreeService` — reading the namespace, and the catalogue over it.
//!
//! Two halves that look alike and are not. `browse` and `resolve` read
//! the **live** tree: they touch the filesystem, and they answer for what
//! is there right now. `entry`, `catalogue`, `changes_since` and
//! `freshness` read the **catalogue**: a replicated projection of
//! structure that must answer with the filesystem absent, because a
//! folder whose holding location is down has to list rather than appear
//! empty (`files.catalogue.offline`).
//!
//! The live half delegates to the legacy `FilesService` methods, which
//! already carry the confinement guard, the internals-hiding rules and
//! the on-disk stub probe. Re-deriving any of that here would be a second
//! implementation of the same rules, and the second one is the one that
//! drifts.
//!
//! ## The catalogue is durable, and still derived
//!
//! Nothing populates a `Catalogue` from a replicated log yet, so this
//! lane builds one by walking the live tree the first time a catalogue
//! question is asked. What is new is that the result and its change log
//! are written to disk ([`CATALOGUES_ON_DISK`]) and read back before any
//! walk, with the process-global map in front as a cache.
//!
//! That ordering — disk, then tree — is the whole point. While the walk
//! came first, "the holding location is gone" and "the folder is empty"
//! produced the same answer, so `files.catalogue.offline` could not hold
//! from a cold process and a host holding structure without content had
//! nothing to serve. [`FilesBackend::browse_catalogued`] is the branch
//! that distinguishes them.
//!
//! It stays *derived* under `storage.tier.derived`: deleting the file
//! costs a walk, never data. Disposable means safe to lose, not obliged
//! to be lost.
//!
//! What is still missing:
//!
//! - **Convergence across servers.** The log now outlives the process,
//!   which was the prerequisite, but `Cursor` is a per-process `u64`
//!   that restarts at zero — two hosts' cursors are incomparable, and
//!   there is no merge rule for two servers upserting one path.
//!   `files.peering.replication` needs both.
//! - **Invalidation.** A file written by something other than the write
//!   lane is invisible to the catalogue until it is rebuilt. `note_write`
//!   folds in this crate's own mutations; the filesystem watcher is what
//!   should feed the rest, and wiring it is separate work.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use chrono::{DateTime, Utc};
use files_domain::catalogue::{Catalogue, Change};
use files_proto::error::FilesFault;
use files_proto::id::{ContentId, RootId};
use files_proto::model::{BrowseEntry, FileRootInfo, RootFlavor, TreeNode};
use files_proto::path::{RootPath, TreePath};
use files_proto::service::tree::{
    CatalogueDelta, CatalogueEntry, Cursor, EntryKind, Freshness, Hydration, TreeService,
};
use files_proto::{FilesError, FilesService};

use crate::backend::FilesBackend;
use crate::durable::Scoped;

/// How many changes one page of a catalogue delta carries.
///
/// The bound is what makes `more` meaningful: a caller that ignores it
/// and assumes one page is the whole catalogue is wrong from the first
/// root that exceeds this, rather than wrong only on somebody else's
/// hundred-thousand-file tree.
const PAGE: usize = 512;

/// Every root's catalogue, for this process only.
///
/// Global rather than a `FilesBackend` field because the backend is
/// owned elsewhere and this lane may not grow it a field yet.
///
/// **Keyed by host as well as root, and that is not paranoia.** A
/// `RootId` alone used to be enough because two backends in one process
/// were always different orgs with different roots. `files.peering.*`
/// makes the same root legitimately present on several hosts at once —
/// a server and its backup, two servers hosting one org — and a
/// process running both would otherwise serve one host's catalogue as
/// the other's. Which is exactly wrong in the one case that matters:
/// the host holding structure would answer with the catalogue of the
/// host holding content.
///
/// The in-memory copy is a cache over [`CATALOGUES_ON_DISK`], which is
/// the authority across restarts. Keeping both is what lets a hot
/// process answer without touching the disk and a cold one answer
/// without touching the *tree*.
type CatalogueKey = (std::path::PathBuf, RootId);

fn catalogues() -> &'static Mutex<HashMap<CatalogueKey, Catalogue>> {
    static CATALOGUES: OnceLock<Mutex<HashMap<CatalogueKey, Catalogue>>> = OnceLock::new();
    CATALOGUES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// This backend's key for a root. The data directory is the host.
fn key_of(backend: &FilesBackend, root_id: RootId) -> CatalogueKey {
    (backend.data_dir().to_path_buf(), root_id)
}

/// The durable catalogue, per org.
///
/// `storage.tier.derived` says derived state is disposable, and this
/// stays derived — losing the file costs a walk, not data. But
/// `files.catalogue.offline` promises a browse works with the
/// filesystem absent, and a process that rebuilds by walking on first
/// use cannot honour that from cold. Disposable means safe to lose, not
/// obliged to be lost.
static CATALOGUES_ON_DISK: Scoped<HashMap<RootId, Catalogue>> = Scoped::new("catalogue");

/// Write a root's catalogue through to disk.
///
/// Called wherever the in-memory copy changes, so the two cannot drift
/// into a state where the durable answer is older than the served one.
fn persist(backend: &FilesBackend, root_id: RootId, cat: &Catalogue) {
    CATALOGUES_ON_DISK.write(backend, |book| {
        book.insert(root_id, cat.clone());
    });
}

/// The registered root, or the typed fault naming the id we could not
/// find.
///
/// A free function rather than an inherent method: `backend.rs` is owned
/// elsewhere during this migration, and two lanes each adding an inherent
/// `root_or_fault` to `FilesBackend` would make every call site ambiguous.
fn root_of(backend: &FilesBackend, root_id: RootId) -> Result<FileRootInfo, FilesFault> {
    backend
        .registry_get(root_id.get())
        .ok_or(FilesFault::RootNotFound(root_id))
}

/// The legacy four-`String` error, onto the variants a caller branches on.
///
/// `NotFound` is the interesting one: the root's existence is checked
/// before the call, so a `NotFound` coming back can only be about the
/// path, and the caller gets a `PathNotFound` carrying it rather than
/// prose to parse.
fn fault_of(err: FilesError, path: &RootPath) -> FilesFault {
    match err {
        FilesError::NotFound(_) => FilesFault::PathNotFound(path.clone()),
        FilesError::AlreadyExists(m) => FilesFault::AlreadyRoot(m),
        FilesError::BadRequest(m) => FilesFault::Invalid(m),
        FilesError::Io(m) => FilesFault::Io(m),
    }
}

/// A file's mtime, or `now` when the filesystem will not say.
///
/// Falling back to `now` rather than to the epoch keeps the entry
/// plausible in a sorted listing; the field a caller reads for trust is
/// `confirmed_at`, which is genuinely when we looked.
fn modified_at(disk: &Path, now: DateTime<Utc>) -> DateTime<Utc> {
    std::fs::metadata(disk)
        .and_then(|m| m.modified())
        .map_or(now, DateTime::<Utc>::from)
}

/// One live-tree listing entry, as a catalogue record.
fn record(
    root: &FileRootInfo,
    path: RootPath,
    listed: &BrowseEntry,
    disk: &Path,
    now: DateTime<Utc>,
) -> CatalogueEntry {
    // A stub is the one case where we know the content address without
    // hashing: the stub file *is* the address. A resident file is
    // published unverified — `content: None` is what adoption's tail
    // looks like, and inventing an id here would be a lie the catalogue
    // then replicates.
    //
    // Detection is stat-bounded, as it is in `browse`: only a file small
    // enough to *be* a stub is opened, so cataloguing a folder of media
    // reads no content at all.
    let stub = (!listed.is_dir
        && root.flavor == RootFlavor::Media
        && listed.size.is_some_and(crate::stub::candidate_len))
    .then(|| crate::stub::probe(disk))
    .flatten();

    CatalogueEntry {
        root_id: RootId::new(root.id),
        path,
        kind: if listed.is_dir {
            EntryKind::Directory
        } else {
            EntryKind::File
        },
        size: stub
            .as_ref()
            .map_or_else(|| listed.size.unwrap_or(0), |s| s.size),
        content: stub.as_ref().map(|s| ContentId(s.file_id.clone())),
        hydration: if stub.is_some() {
            Hydration::Stub
        } else {
            Hydration::Resident
        },
        // A directory holds no bytes, so it is held nowhere — the field
        // is about content, and `locations` on a directory would make
        // reachability look like a property of the shape of the tree.
        locations: if listed.is_dir {
            Vec::new()
        } else {
            // Empty on a host holding structure only. The entry is real
            // and no location *here* answers for it, which is a
            // different statement from "nowhere does".
            root.path.clone().into_iter().collect()
        },
        modified_at: modified_at(disk, now),
        confirmed_at: now,
    }
}

/// Walk a root's live tree into a catalogue.
///
/// Breadth-first over an explicit queue rather than recursion, so a deep
/// tree costs heap rather than stack.
///
/// The load-bearing part is the failure branch. When a directory will not
/// list — permissions, an unmounted volume, an I/O error — its own entry
/// is **kept and re-marked `Unavailable`**, and only its subtree is
/// omitted. Dropping the entry, or keeping it as `Resident` with no
/// children, both produce the one thing the spec forbids: a folder that
/// looks empty because its content is out of reach.
// t[impl files.catalogue.complete] — every reachable path gets an entry
// t[impl files.catalogue.offline] — unreachable is marked, never missing
// t[impl files.catalogue.staleness] — every entry records when we looked
fn walk(root: &FileRootInfo, now: DateTime<Utc>) -> Catalogue {
    let mut cat = Catalogue::new(RootId::new(root.id));
    // An unplaced root has nothing to walk. Callers reach the catalogue
    // off disk before they get here, so this is the cold-and-unplaced
    // case: an empty catalogue, which `browse_catalogued` then reports
    // as an empty listing rather than as a missing root.
    let Some(root_dir) = root.local_tree().map(std::path::Path::to_path_buf) else {
        return cat;
    };
    let mut queue = vec![(RootPath::root(), root_dir.clone())];

    while let Some((at, dir)) = queue.pop() {
        // `hide_internals` only at the top level (the marker file and
        // version store live there and nowhere else); `.git` at every
        // depth on a software root, where a nested one is a submodule's
        // object store rather than this root's content.
        let listed = FilesBackend::list_dir(
            &dir,
            dir == root_dir,
            root.flavor == RootFlavor::Software,
        );

        let listed = match listed {
            Ok(listed) => listed,
            Err(_) if at.is_root() => {
                // The root itself will not list: there is no entry to
                // mark, so the *root* is what is unreachable. Recording
                // that keeps `freshness` from claiming a confirmed view
                // of a tree we never saw.
                if let Some(here) = root.path.as_deref() {
                    cat.set_location_reachable(here, false, now);
                }
                continue;
            }
            Err(_) => {
                if let Some(existing) = cat.get(&at).cloned() {
                    cat.upsert(CatalogueEntry {
                        hydration: Hydration::Unavailable,
                        ..existing
                    });
                }
                continue;
            }
        };

        for entry in listed {
            let Ok(path) = at.join(&entry.name) else {
                continue; // a name that is not a single component is not ours to invent a path for
            };
            let disk = dir.join(&entry.name);
            let is_dir = entry.is_dir;
            cat.upsert(record(root, path.clone(), &entry, &disk, now));
            if is_dir {
                queue.push((path, disk));
            }
        }
    }

    cat
}

/// One page of changes after `from`, and where to resume.
///
/// The next cursor is arithmetic rather than a peek at the log because
/// the domain's sequence advances by exactly one per logged change, so
/// `from + taken` names the position after the last item on this page
/// whether the page ended on an upsert or a removal.
///
/// A page is a slice of the *log*, so one path may appear more than once
/// in `changed` — a walk that finds a folder and then re-marks it
/// `Unavailable` logs both. Last-wins is the contract, and folding in
/// order is what a client must do anyway to converge.
// t[impl files.catalogue.bounded] — a page, never the whole log
fn page(cat: &Catalogue, from: &Cursor) -> CatalogueDelta {
    // Match the domain's own leniency: an unparseable cursor resyncs from
    // the start, so the cursor we hand back must be counted from there
    // too or the client would skip what it just received.
    let base: u64 = from.0.parse().unwrap_or(0);
    let all = cat.changes_since(from);
    let more = all.len() > PAGE;

    let mut changed = Vec::new();
    let mut removed = Vec::new();
    let mut taken = 0u64;
    for change in all.into_iter().take(PAGE) {
        taken += 1;
        match change {
            Change::Upserted(e) => changed.push(e),
            Change::Removed(p) => removed.push(p),
        }
    }

    CatalogueDelta {
        changed,
        removed,
        cursor: Cursor((base + taken).to_string()),
        more,
    }
}

/// Read a root's catalogue, building it on first use.
///
/// The build happens outside the map's lock — walking a large tree while
/// holding it would serialise every other root's first read behind it —
/// so two concurrent first calls may both walk. The loser's work is
/// dropped rather than merged, which is correct because both walked the
/// same tree.
fn with_catalogue<T>(
    backend: &FilesBackend,
    root_id: RootId,
    f: impl FnOnce(&Catalogue) -> T,
) -> Result<T, FilesFault> {
    let root = root_of(backend, root_id)?;

    let key = key_of(backend, root_id);
    if let Some(cat) = catalogues()
        .lock()
        .expect("catalogue lock poisoned")
        .get(&key)
    {
        return Ok(f(cat));
    }

    // Disk before tree. A restart must not have to walk 48,000 files to
    // answer its first question, and a host holding structure without
    // content has no tree to walk at all — the walk below would report
    // its whole catalogue as empty rather than as elsewhere.
    if let Some(stored) = CATALOGUES_ON_DISK.read(backend, |book| book.get(&root_id).cloned()) {
        let mut guard = catalogues().lock().expect("catalogue lock poisoned");
        let cat = guard.entry(key).or_insert(stored);
        return Ok(f(cat));
    }

    // With a tree, walk it. Without one, derive from the commit graph —
    // trees say what exists, manifests say how big it is. That is the
    // whole of "replicate the commit graph, re-derive the catalogue
    // locally": the structure is what converged, and this is where it
    // becomes something to browse.
    let built = match root.local_tree() {
        Some(_) => walk(&root, Utc::now()),
        None => from_head(backend, &root, Utc::now()),
    };
    persist(backend, root_id, &built);
    let mut guard = catalogues().lock().expect("catalogue lock poisoned");
    let cat = guard.entry(key).or_insert(built);
    Ok(f(cat))
}

/// Build a catalogue from the root's head, with no filesystem involved.
///
/// Everything is `Unavailable`: this host knows the file exists and its
/// size, and holds none of its bytes. Directories are synthesised from
/// the paths, because a commit tree records files and the browse above
/// needs their parents to exist as entries.
fn from_head(backend: &FilesBackend, root: &FileRootInfo, now: DateTime<Utc>) -> Catalogue {
    let root_id = RootId::new(root.id);
    let mut cat = Catalogue::new(root_id);
    // No head yet is an empty catalogue, not a failure: a host that has
    // been given a root and not yet reconciled is in a legitimate state
    // and should say "nothing here yet" rather than refuse.
    let Ok(files) = backend.head_structure(root.id) else {
        return cat;
    };

    let mut dirs: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for (path, size) in files {
        let Ok(at) = RootPath::parse(&path) else {
            continue;
        };
        let mut parts: Vec<&str> = at.components().collect();
        parts.pop();
        let mut prefix = String::new();
        for part in parts {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            dirs.insert(prefix.clone());
        }
        cat.upsert(CatalogueEntry {
            root_id,
            path: at,
            kind: EntryKind::File,
            size,
            content: None,
            hydration: Hydration::Unavailable,
            // No location *here*. A different statement from "nowhere".
            locations: Vec::new(),
            modified_at: now,
            confirmed_at: now,
        });
    }
    for dir in dirs {
        let Ok(at) = RootPath::parse(&dir) else {
            continue;
        };
        cat.upsert(CatalogueEntry {
            root_id,
            path: at,
            kind: EntryKind::Directory,
            size: 0,
            content: None,
            hydration: Hydration::Unavailable,
            locations: Vec::new(),
            modified_at: now,
            confirmed_at: now,
        });
    }
    cat
}

impl FilesBackend {
    /// List from the catalogue, with no filesystem underneath.
    ///
    /// Everything comes back marked `Unavailable` rather than
    /// `Resident`: the entries are real and their bytes are not here, so
    /// claiming residency would send a caller to open a file that is not
    /// on this machine. `files.catalogue.offline` says a location being
    /// down is not a fact about the tree — it is a fact about the
    /// content, and this is where that distinction is made concrete.
    fn browse_catalogued(
        &self,
        root_id: RootId,
        path: &RootPath,
    ) -> Result<Vec<BrowseEntry>, FilesFault> {
        with_catalogue(self, root_id, |cat| {
            cat.children(path)
                .into_iter()
                .map(|e| BrowseEntry {
                    name: e
                        .path
                        .components()
                        .next_back()
                        .unwrap_or_default()
                        .to_string(),
                    is_dir: matches!(e.kind, EntryKind::Directory),
                    // The real size, from the catalogue rather than
                    // from a file that is not here. A host that
                    // answers "how big is this project" with zeroes
                    // because it holds no bytes is worse than one that
                    // refuses — it is confidently wrong.
                    size: (!matches!(e.kind, EntryKind::Directory)).then_some(e.size),
                    stub: true,
                    divergent: false,
                })
                .collect()
        })
    }
}

impl TreeService for FilesBackend {
    /// The live tree, not the catalogue — this is the listing that
    /// answers for what is on disk this instant.
    // t[impl files.ignore.retained] — ignored is absent from listings too
    async fn browse(
        &self,
        root_id: RootId,
        path: RootPath,
    ) -> Result<Vec<BrowseEntry>, FilesFault> {
        // Re-validate: the type is transparent on the wire, so a hostile
        // peer's `..` arrives having never seen `parse`.
        let path = path.validate()?;

        // A root accepted from another server has no live tree on this
        // disk. Walking its path would list an empty directory as though
        // the subtree were empty, rather than elsewhere — so it resolves
        // through its origin instead, and the caller cannot tell the
        // difference. That is what `files.topology.federation` means by
        // "a first-class item, not a download link".
        if self.remote_of(root_id).is_some() {
            return self.browse_remote(root_id, &path).await;
        }

        let root = crate::lane::root_or_fault(self, root_id)?;

        // No tree on this disk, and not a remote root either: this host
        // holds the org's *structure* and not its content
        // (`files.peering.replication`), or the holding location is
        // simply down (`files.catalogue.offline`). Both are the same
        // situation from here — we know what is there and cannot see it
        // — and the catalogue is exactly the answer.
        //
        // Distinguished from an empty directory on purpose. Walking a
        // path that is not there returns nothing, which reads as "this
        // folder is empty" when the truth is "this folder is
        // elsewhere", and that is the failure the rule names.
        // Deliberately not `lane_tree`, which refuses: this is the one
        // place an unplaced root is a normal answer rather than a
        // missing capability. `None` (this host holds structure) and
        // `Some(p)` where `p` is gone (the location is down) are the
        // same situation from here.
        if !root.local_tree().is_some_and(std::path::Path::exists) {
            return self.browse_catalogued(root_id, &path);
        }

        let mut listed = <Self as FilesService>::browse(
            self,
            root_id.get(),
            path.as_str().to_string(),
        )
        .await
        .map_err(|e| fault_of(e, &path))?;

        // The Ignore set governs listings, not only captures.
        //
        // `files.ignore.retained` says an ignored file is absent from
        // user-facing listings AND from history. It was only ever applied
        // to the second: the set decided what entered a version, and a
        // browse returned the raw directory — so a Mac writing to the NAS
        // put a `._name` beside every file a user could see, which is
        // most of what a 14,671-file album contains.
        //
        // `drive_browse` deliberately does NOT filter: it shows the raw
        // tree, internals included, which is the distinction the glossary
        // draws between the two.
        let store = self.store_of(&root);
        if let Ok(ignores) = crate::ignore::for_root(&store, root.flavor) {
            listed.retain(|entry| {
                let rel = if path.is_root() {
                    entry.name.clone()
                } else {
                    format!("{}/{}", path.as_str(), entry.name)
                };
                !crate::ignore::is_ignored(&ignores, &rel)
            });
        }
        Ok(listed)
    }

    /// The org tree — one resolver behind both the explorer and the
    /// WebDAV mount, so a mounted share and the app can never disagree
    /// about what the namespace contains.
    async fn resolve(&self, path: TreePath) -> Result<TreeNode, FilesFault> {
        let path = path.validate()?;
        <Self as FilesService>::tree_browse(self, path.as_str().to_string())
            .await
            .map_err(|e| match e {
                FilesError::NotFound(_) => FilesFault::TreePathNotFound(path.clone()),
                FilesError::AlreadyExists(m) => FilesFault::AlreadyRoot(m),
                FilesError::BadRequest(m) => FilesFault::Invalid(m),
                FilesError::Io(m) => FilesFault::Io(m),
            })
    }

    /// One entry, without listing its parent — what a link resolves
    /// through, and what an "open to metadata" costs.
    // t[impl files.catalogue.offline] — answered from structure, no filesystem
    async fn entry(&self, root_id: RootId, path: RootPath) -> Result<CatalogueEntry, FilesFault> {
        let path = path.validate()?;
        let this = self.clone();
        let wanted = path.clone();
        crate::lane::blocking(move || {
            Ok(with_catalogue(&this, root_id, |cat| cat.get(&wanted).cloned()))
        })
        .await??
        .ok_or(FilesFault::PathNotFound(path))
    }

    /// The initial sync. `None` starts from the beginning; a cursor
    /// resumes, which is what makes a client that dropped mid-page able
    /// to carry on rather than start over.
    // t[impl files.catalogue.complete]
    async fn catalogue(
        &self,
        root_id: RootId,
        cursor: Option<Cursor>,
    ) -> Result<CatalogueDelta, FilesFault> {
        let from = cursor.unwrap_or_else(|| Cursor("0".to_string()));
        let this = self.clone();
        crate::lane::blocking(move || Ok(with_catalogue(&this, root_id, |cat| page(cat, &from))))
            .await?
    }

    /// Everything after `cursor` — the reconnect path, which never
    /// re-lists the tree.
    // t[impl files.catalogue.concurrent] — resume from a cursor, never re-list
    async fn changes_since(
        &self,
        root_id: RootId,
        cursor: Cursor,
    ) -> Result<CatalogueDelta, FilesFault> {
        let this = self.clone();
        crate::lane::blocking(move || Ok(with_catalogue(&this, root_id, |cat| page(cat, &cursor))))
            .await?
    }

    /// What a view reads to say "as of" instead of implying now.
    ///
    /// Reported for every registered root, building a catalogue for any
    /// that has not been asked about yet: a root missing from this list
    /// would read as a root with nothing to say about its currency, which
    /// is the silent staleness the spec forbids.
    // t[impl files.catalogue.staleness]
    async fn freshness(&self) -> Result<Vec<Freshness>, FilesFault> {
        let this = self.clone();
        crate::lane::blocking(move || {
            let now = Utc::now();
            let mut out = Vec::new();
            for root in this.registry_list() {
                let id = RootId::new(root.id);
                // A root released between the list and the read is not an
                // error for the others — it simply has no freshness.
                if let Ok(f) = with_catalogue(&this, id, |cat| cat.freshness(now)) {
                    out.push(f);
                }
            }
            Ok(out)
        })
        .await
    }
}

// ── Keeping the catalogue honest about writes ─────────────────────────

/// Stat one live-tree path into a catalogue record.
///
/// The walk builds records from a `BrowseEntry` it already has; a write
/// knows only the path it touched, so this is the same construction from
/// a bare stat. Stub detection stays stat-bounded, exactly as in
/// [`record`].
fn record_of(root: &FileRootInfo, path: &RootPath, now: DateTime<Utc>) -> Option<CatalogueEntry> {
    let disk = root.local_tree()?.join(path.as_str());
    let meta = std::fs::metadata(&disk).ok()?;
    let is_dir = meta.is_dir();
    let size = if is_dir { 0 } else { meta.len() };

    let stub = (!is_dir && root.flavor == RootFlavor::Media && crate::stub::candidate_len(size))
        .then(|| crate::stub::probe(&disk))
        .flatten();

    Some(CatalogueEntry {
        root_id: RootId::new(root.id),
        path: path.clone(),
        kind: if is_dir {
            EntryKind::Directory
        } else {
            EntryKind::File
        },
        size: stub.as_ref().map_or(size, |s| s.size),
        content: stub.as_ref().map(|s| ContentId::new(s.file_id.clone())),
        hydration: if stub.is_some() {
            Hydration::Stub
        } else {
            Hydration::Resident
        },
        locations: if is_dir {
            Vec::new()
        } else {
            root.path.clone().into_iter().collect()
        },
        modified_at: meta.modified().map_or(now, DateTime::<Utc>::from),
        confirmed_at: now,
    })
}

/// Fold a completed mutation into this root's catalogue.
///
/// Called by the lanes that change a live tree — `crate::lane::write` and
/// `crate::lane::upload` — because nothing else does. Without it a write
/// is invisible to `entry`, `catalogue` and `changes_since` until the
/// process restarts, which is the difference between a catalogue that is
/// *stale* (allowed, and reported by `freshness`) and one that is *wrong*
/// (not allowed at all).
///
/// It **updates** rather than invalidates. Dropping the catalogue would
/// discard the change log with it, forcing every subscribed client to
/// re-list a tree that changed by one file — the exact re-listing
/// `files.catalogue.concurrent` says a client never has to do. Upserting
/// keeps the log, so the write arrives as a delta.
///
/// A root with no resident catalogue is left alone: it will be walked on
/// first read and see the write then. Nothing here builds one, because a
/// write must not pay for a full walk of a tree nobody has browsed.
// t[impl files.catalogue.concurrent] — a write arrives as a delta
pub(crate) fn note_write(
    backend: &FilesBackend,
    root: &FileRootInfo,
    touched: &[RootPath],
    removed: &[RootPath],
) {
    let root_id = RootId::new(root.id);
    let updated = {
        let mut guard = catalogues().lock().expect("catalogue lock poisoned");
        let Some(cat) = guard.get_mut(&key_of(backend, root_id)) else {
            return;
        };
        let now = Utc::now();

        for path in removed {
            cat.remove(path);
        }
        for path in touched {
            // A touched path that is gone was moved away or deleted; the
            // caller may not have distinguished the two, so resolve it
            // here rather than requiring them to.
            match record_of(root, path, now) {
                Some(entry) => cat.upsert(entry),
                None => cat.remove(path),
            }
        }
        cat.clone()
    };
    // Outside the lock, and unconditional: a durable copy that lags the
    // served one is worse than none, because a restart would answer
    // confidently with a tree that is one write out of date.
    persist(backend, root_id, &updated);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(path: &str) -> FileRootInfo {
        FileRootInfo {
            id: uuid::Uuid::from_bytes([3; 16]),
            name: "Session".into(),
            path: Some(path.into()),
            flavor: RootFlavor::Media,
            created_at: Utc::now(),
            project_version: None,
        }
    }

    /// The walk is what turns a live tree into structure, so its shape —
    /// depth, not just the top level — is the thing worth pinning.
    #[test]
    // t[verify files.catalogue.complete]
    fn a_walk_reaches_every_depth() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("Sessions/Song")).unwrap();
        std::fs::write(tmp.path().join("Sessions/Song/mix.wav"), b"take one").unwrap();
        std::fs::write(tmp.path().join("notes.txt"), b"hi").unwrap();

        let cat = walk(&root(&tmp.path().to_string_lossy()), Utc::now());

        assert_eq!(cat.len(), 4, "Sessions, Song, mix.wav, notes.txt");
        let mix = cat.get(&RootPath::parse("Sessions/Song/mix.wav").unwrap()).unwrap();
        assert_eq!(mix.kind, EntryKind::File);
        assert_eq!(mix.size, 8);
        assert_eq!(mix.hydration, Hydration::Resident);
        assert!(
            cat.get(&RootPath::parse("Sessions").unwrap())
                .unwrap()
                .locations
                .is_empty(),
            "a directory holds no bytes, so it is held nowhere"
        );
    }

    #[test]
    // t[verify files.catalogue.staleness]
    fn an_unreadable_root_is_reported_unreachable_rather_than_empty_and_current() {
        let cat = walk(&root("/definitely/not/a/path"), Utc::now());
        let f = cat.freshness(Utc::now());
        assert!(
            !f.reachable,
            "a tree we could not read must not present as confirmed"
        );
        assert_eq!(f.entries, 0);
    }

    #[test]
    fn a_page_stops_at_the_bound_and_says_so() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..(PAGE + 10) {
            std::fs::write(tmp.path().join(format!("f{i}")), b"x").unwrap();
        }
        let cat = walk(&root(&tmp.path().to_string_lossy()), Utc::now());

        let first = page(&cat, &Cursor("0".into()));
        assert_eq!(first.changed.len(), PAGE);
        assert!(first.more, "a caller must not read one page as the whole");

        let second = page(&cat, &first.cursor);
        assert_eq!(second.changed.len(), 10, "and resumes exactly where it left");
        assert!(!second.more);
    }

    #[test]
    // t[verify files.catalogue.concurrent]
    fn an_unplaceable_cursor_resyncs_and_the_next_cursor_counts_from_the_start() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a"), b"x").unwrap();
        std::fs::write(tmp.path().join("b"), b"x").unwrap();
        let cat = walk(&root(&tmp.path().to_string_lossy()), Utc::now());

        let d = page(&cat, &Cursor("not-a-number".into()));
        assert_eq!(d.changed.len(), 2, "safe direction is to send everything");
        assert_eq!(
            d.cursor,
            Cursor("2".into()),
            "and the resume point counts the resync, or the client re-receives it forever"
        );
    }
}
