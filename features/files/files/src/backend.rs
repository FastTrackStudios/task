//! [`FilesBackend`]: server-side [`FilesService`] impl. Wraps
//! [`Registry`] (root identity) and one
//! `task_files_version_store::VersionStoreBackend`-backed jj repo per
//! root (opened lazily, cached for the process's lifetime — see
//! [`crate::repo_open`]).
//!
//! **All the real work below is synchronous**, driven through
//! `pollster::block_on` wherever it touches `task-files-version-store`
//! (which is itself async). This isn't a style choice: jj-lib's own
//! async fns aren't `Send` on every path (see `repo_open`'s module
//! doc), and `#[architect::rpc]` methods must return a `Send` future —
//! so none of this crate's logic can `.await` jj-lib directly from
//! inside an `async fn` without poisoning the RPC method's future.
//! Every `FilesService` method below runs its sync `*_inner` body on
//! `tokio::task::spawn_blocking` (same convention as `task-server`'s
//! `notifier.rs`/`mcp.rs`) rather than inline on the calling async
//! task — a full-tree scan or a multi-GB checkpoint must not stall the
//! shared runtime's other org RPCs (PR #280 review).
//!
//! **Filesystem confinement.** `create_root` and `drive_browse` accept
//! a caller-supplied path; both are confined to [`FilesBackend::confine_root`]
//! (this org's `<data_root>/orgs/<slug>/files/` — see
//! [`FilesBackend::new`]) rather than the whole server filesystem.
//! `permits.rs` mounts `create_root`/`drive_browse` at plain member
//! tier, same as every other CRUD verb on this router — the intended
//! authorization boundary is "any member of *this* org", not "root on
//! the box", so path arguments must never reach outside this org's own
//! subtree (they could otherwise read/ingest another org's data, since
//! every org's `OrgAppState` shares one `data_root`). A full Storage
//! Location grant model (ADR 0001, out of scope for #259) will
//! eventually make placement an explicit, operator-governed axis; this
//! confinement is the minimum viable stopgap until then. `browse`
//! (root-scoped) is confined the same way, against the *root's own*
//! canonicalized path rather than the whole org tree — see
//! `browse_inner`'s doc for how that also closes the absolute-subpath
//! and symlink-escape holes.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::Utc;
use files_proto::{
    BrowseEntry, ChainEntry, CheckpointInfo, FileRootInfo, FilesError, FilesEvent, FilesService,
    GcReport, HydrationChange, HydrationReport, NamedVersion, ProjectVersion, RootFlavor,
    SavePoint, SnapshotInfo, VersionRef,
};
use jj_lib::backend::{Backend, ChangeId, CommitId};
use jj_lib::object_id::{HexPrefix, ObjectId as _, PrefixResolution};
use jj_lib::repo::{ReadonlyRepo, Repo as _};
use jj_lib::repo_path::{RepoPath, RepoPathBuf};
use task_files_version_store::VersionStoreBackend;
use uuid::Uuid;

use crate::badges;
use crate::cadence::journal::{CheckpointRecord, SnapshotRecord};
use crate::cadence::{
    ActivitySink, CadenceConfig, CadenceEngine, Clock, Due, DueKind, Journal, RootWatcher,
    SystemClock,
};
use crate::certify::MidHashHook;
use crate::checkpoint::Capture;
use crate::consts::{GIT_DIR, MARKER_FILE, STORE_DIR};
use crate::error::Error;
use crate::git_root;
use crate::hydration;
use crate::ignore;
use crate::registry::{Registry, RootMarker, read_root_marker};
use crate::repo_open;
use crate::scan;
use crate::stub;
use crate::versions::VaultVersions;

/// Default `keep_newer` window for [`FilesService::gc_root`]: nothing
/// written in the last minute is ever swept, so a sweep can't race a
/// checkpoint that is mid-write on another connection (the
/// concurrent-writer guard `Backend::gc`'s own contract describes).
const DEFAULT_GC_KEEP_NEWER_SECS: u64 = 60;

/// One root's live jj state: the repo handle (reassigned after every
/// `checkpoint_now`) and its current checkpoint head. `head` is tracked
/// explicitly rather than re-derived from `repo.view().heads()` on
/// every call — see `checkpoint::checkpoint`'s own doc example, which
/// establishes this as the pattern for reading back the commit a
/// checkpoint just produced.
struct RootRuntime {
    repo: Arc<ReadonlyRepo>,
    head: CommitId,
    /// Tip of the auto-snapshot branch hanging off `head`, or `None`
    /// when the session has taken none since the last checkpoint
    /// (issue #260 — snapshots branch off the checkpoint line rather
    /// than extending it, see [`crate::cadence`]).
    snapshot_head: Option<CommitId>,
}

/// What one dehydrate attempt did (issue #263). `Dirty` — on-disk
/// content differs from the checkpoint head — is an outcome, not an
/// error: the policy apply pass classifies it structurally and moves
/// on, rather than matching error-message substrings (PR #289 review).
enum DehydrateOutcome {
    Done(BrowseEntry),
    Dirty,
}

/// Which kind of capture a write is — the one difference that decides
/// what it parents on and how it is recorded (issue #260).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptureKind {
    /// Ephemeral auto-snapshot: parented on the snapshot branch, never
    /// a chain entry.
    Snapshot,
    /// Certified Session checkpoint: parented on the checkpoint head.
    Checkpoint,
}

impl From<DueKind> for CaptureKind {
    fn from(kind: DueKind) -> Self {
        match kind {
            DueKind::Snapshot => Self::Snapshot,
            DueKind::Checkpoint => Self::Checkpoint,
        }
    }
}

/// What one performed capture produced — the wire payload plus which
/// kind it was, so [`FilesBackend::tick`] can report a cadence pass.
#[derive(Debug, Clone)]
pub enum Captured {
    Snapshot(SnapshotInfo),
    Checkpoint(CheckpointInfo),
}

/// Exactly the state a watcher hint needs: which roots exist, their
/// Ignore sets, and the cadence engine to report into.
///
/// This is a *slice* of [`FilesBackend`] rather than a clone of it, and
/// deliberately so. A watcher lives in the backend's `watchers` map and
/// its callback holds this sink; handing it a whole backend clone would
/// close a reference cycle (watchers map → watcher → callback → backend
/// clone → the same watchers map `Arc`) that no drop could ever break,
/// so a released org would leak its backend and keep watching (PR #283
/// review). `Hints` holds no watcher map and no driver handle, so the
/// cycle simply does not exist.
struct Hints {
    registry: Arc<Registry>,
    ignores: Arc<Mutex<HashMap<Uuid, Arc<jj_lib::gitignore::GitIgnoreFile>>>>,
    cadence: Arc<CadenceEngine>,
}

impl Hints {
    /// The root's whole Ignore set (flavor seed + its stored patterns),
    /// compiled on first touch and cached.
    fn ignore_of(
        ignores: &Mutex<HashMap<Uuid, Arc<jj_lib::gitignore::GitIgnoreFile>>>,
        root: &FileRootInfo,
    ) -> Result<Arc<jj_lib::gitignore::GitIgnoreFile>, Error> {
        if let Some(set) = ignores
            .lock()
            .expect("ignore cache lock poisoned")
            .get(&root.id)
        {
            return Ok(set.clone());
        }
        let set = ignore::for_root(&repo_open::store_dir(Path::new(&root.path)), root.flavor)?;
        ignores
            .lock()
            .expect("ignore cache lock poisoned")
            .insert(root.id, set.clone());
        Ok(set)
    }

    /// Note `paths` as activity on `root_id`, returning how many
    /// survived the root's Ignore set.
    fn note(&self, root_id: Uuid, paths: &[String]) -> Result<u32, Error> {
        let root = self
            .registry
            .get(root_id)
            .ok_or_else(|| Error::NotFound(root_id.to_string()))?;
        let ignores = Self::ignore_of(&self.ignores, &root)?;
        Ok(self
            .cadence
            .note_activity(root_id, paths, &ignores, root.flavor))
    }
}

/// Watcher hints land here (see [`crate::cadence::watcher`]): the
/// backend is what knows a root's flavor and Ignore set, so it is what
/// turns a raw path list into cadence activity.
impl ActivitySink for Hints {
    fn note_activity(&self, root_id: Uuid, paths: Vec<String>) {
        if let Err(err) = self.note(root_id, &paths) {
            tracing::debug!(%root_id, %err, "files watcher hint dropped");
        }
    }
}

/// Where, besides its own org directory, this org may hold live trees.
///
/// Implemented by the server over the deployment's Storage Location
/// registry (`files_storage::StorageCore::live_tree_boundaries`). It is a
/// trait rather than a direct dependency so this crate — which is about
/// versioning file trees — stays independent of the placement layer, and
/// so a test can grant a boundary without standing up a registry.
///
/// Called on every path resolution, so implementations must be cheap:
/// a read-lock over an in-memory registry, not I/O.
pub trait LocationBoundaries: Send + Sync {
    /// Absolute directories that are permitted, in addition to the org
    /// directory. Empty means "no locations" — the default everywhere.
    fn permitted(&self) -> Vec<PathBuf>;
}

#[derive(Clone, architect::HasDispatcher)]
pub struct FilesBackend {
    data_dir: PathBuf,
    /// Canonicalized once at construction — the boundary `create_root`
    /// / `drive_browse` path arguments must resolve inside (see the
    /// module doc's "Filesystem confinement" section).
    confine_root: PathBuf,
    /// Extra directories this org may hold live trees in, beyond
    /// `confine_root` — the deployment's Storage Locations, resolved per
    /// grant. `None` on a backend built without a storage registry
    /// (tests, and any single-machine deployment), where the org
    /// directory is the only permitted boundary exactly as before.
    ///
    /// Holds the registry rather than a snapshot of paths, because a
    /// grant issued after boot must take effect without restarting the
    /// server — a boundary cached at construction would mean "your new
    /// Storage Location works tomorrow".
    boundaries: Option<Arc<dyn LocationBoundaries>>,
    registry: Arc<Registry>,
    /// The org vault holding the curated version entities (issue
    /// #261). Separate from `data_dir`: a File Root's *content* is
    /// never vault-replicated, but the Named / Project Version pages
    /// that reference it are ordinary vault files, and that is exactly
    /// what carries them offline-first to every device.
    versions: VaultVersions,
    repos: Arc<Mutex<HashMap<Uuid, RootRuntime>>>,
    /// One lock per root, serializing every write that reads this
    /// root's state before changing it: `checkpoint_now` (two
    /// concurrent checkpoints must not both read the same head and
    /// silently orphan one commit — PR #280 review), the curation
    /// writes (two namings must not claim one vault page path), and
    /// `gc_root` (a sweep must not miss a name that lands after it
    /// snapshotted its protect set). Created lazily, never removed
    /// (roots are not deleted in v1).
    root_locks: Arc<Mutex<HashMap<Uuid, Arc<Mutex<()>>>>>,
    /// The cadence state machine (issue #260): when each root's session
    /// snapshots, and when it ends in a checkpoint.
    cadence: Arc<CadenceEngine>,
    /// Per-root Ignore sets, compiled on first touch.
    ignores: Arc<Mutex<HashMap<Uuid, Arc<jj_lib::gitignore::GitIgnoreFile>>>>,
    /// Live filesystem watchers, one per watched root.
    watchers: Arc<Mutex<HashMap<Uuid, RootWatcher>>>,
    /// Set by [`FilesBackend::enable_watching`]: newly created roots
    /// start watched too, rather than only on the next restart.
    watch_new_roots: Arc<std::sync::atomic::AtomicBool>,
    /// The cadence driver task, kept so it can be stopped — see
    /// [`FilesBackend::spawn_cadence_driver`].
    driver: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    /// Test seam — see [`FilesBackend::set_mid_hash_hook`].
    hook: Arc<Mutex<Option<MidHashHook>>>,
    /// Test seam — see [`FilesBackend::set_mid_flip_hook`]: invoked
    /// between a restart's terminal checkpoint and its clear phase,
    /// which is the window a mid-flip save lands in (issue #268).
    flip_hook: Arc<Mutex<Option<MidHashHook>>>,
    /// The transcoder that generates derived media (issue #269), when
    /// one is configured (the server injects ffmpeg; a test injects a
    /// fake; unset means the `rendition` RPC 404s and checkpoints skip
    /// warm-up). Behind an `Arc` so a checkpoint can spawn a best-effort
    /// warm-up that outlives the call.
    transcoder: Arc<Mutex<Option<Arc<dyn files_transcode::Transcoder>>>>,
    /// Per-root rendition stores (issue #269), opened once and cached —
    /// a rendition store owns a private iroh-blobs `FsStore`, and
    /// opening a second on one dir while the first is alive hangs (the
    /// same trap the repo cache avoids).
    rendition_stores: Arc<Mutex<HashMap<Uuid, Arc<files_transcode::RenditionStore>>>>,
    /// Serializes rendition-store OPENS across concurrent callers (the
    /// checkpoint warm-up task and a `rendition` request race the same
    /// dir; two opens of one `FsStore` hang). Held across the async
    /// open, so a `tokio` mutex.
    rendition_open_lock: Arc<tokio::sync::Mutex<()>>,
    /// Per-rendition generation locks (issue #269, AC 2: "generates once").
    /// Two Review-page clients requesting the *same* uncached proxy would
    /// otherwise both run the full ffmpeg encode; a per-key lock makes the
    /// loser wait and hit the cache. Keyed `root:source:kind`; entries are
    /// dropped once no one holds them, so the map stays bounded.
    rendition_gen_locks: Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
    /// Fan-out hub behind `#[subscribe] fn events` — every successful
    /// root creation / checkpoint publishes here. Sliding mailbox: a
    /// slow subscriber loses its *oldest* queued events, correct for
    /// these state-shaped payloads (same convention as
    /// `task::TaskBackend`).
    events: architect::PubSub<FilesEvent>,
}

// Manual impl: `PubSub` and the repo cache carry no `Debug`.
impl std::fmt::Debug for FilesBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FilesBackend")
            .field("data_dir", &self.data_dir)
            .finish_non_exhaustive()
    }
}

/// A shared-confinement refusal, in this crate's vocabulary. A rejected
/// or escaping path is a bad request; an I/O fault underneath is one.
fn confinement(err: task_files_util::PathError) -> Error {
    match err {
        task_files_util::PathError::Io(e) => Error::BadRequest(e.to_string()),
        other => Error::BadRequest(other.to_string()),
    }
}

fn to_files_error(err: Error) -> FilesError {
    match err {
        Error::NotFound(m) => FilesError::NotFound(m),
        Error::AlreadyExists(m) => FilesError::AlreadyExists(m),
        Error::BadRequest(m) => FilesError::BadRequest(m),
        Error::Io(e) => FilesError::Io(e.to_string()),
        Error::Json(e) => FilesError::Io(format!("registry json: {e}")),
        Error::VersionStore(e) => FilesError::Io(format!("version store: {e}")),
        Error::Repo(m) => FilesError::Io(format!("jj repo: {m}")),
        Error::JjBackend(e) => FilesError::Io(format!("jj backend: {e}")),
    }
}

/// Run a sync `*_inner` call on the blocking thread pool — the seam
/// every `FilesService` method below uses (see the module doc). The
/// closure captures a cheap `Clone` of `self` (every field is an
/// `Arc`/`PathBuf`), never `self` by reference, so it satisfies
/// `spawn_blocking`'s `'static` bound.
///
/// The seam itself lives in `task-files-util`, shared with
/// `files-storage` — it was a verbatim copy in both (PR #284 review).
async fn blocking<T, F>(f: F) -> Result<T, FilesError>
where
    F: FnOnce() -> Result<T, Error> + Send + 'static,
    T: Send + 'static,
{
    task_files_util::blocking(f, |e| Error::Io(std::io::Error::other(e)))
        .await
        .map_err(to_files_error)
}

impl FilesBackend {
    /// `data_dir` holds the root registry and (for roots the server
    /// hosts) their version stores; `vault_root` is the org vault the
    /// Named / Project Version entities are written into and scanned
    /// from. They are deliberately two directories: root *content* is
    /// never vault-replicated, curation always is.
    pub fn new(
        data_dir: impl Into<PathBuf>,
        vault_root: impl Into<PathBuf>,
    ) -> Result<Self, FilesError> {
        Self::with_cadence(
            data_dir,
            vault_root,
            CadenceConfig::default(),
            Arc::new(SystemClock),
        )
    }

    /// A backend whose cadence engine (issue #260) runs on `config` and
    /// `clock`. Tests use this with a [`crate::cadence::TestClock`]:
    /// quiescence and debounce are simulated, never slept (spec #255's
    /// Testing Decisions).
    pub fn with_cadence(
        data_dir: impl Into<PathBuf>,
        vault_root: impl Into<PathBuf>,
        config: CadenceConfig,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, FilesError> {
        let data_dir = data_dir.into();
        let registry = Registry::open(&data_dir).map_err(to_files_error)?;
        let confine_root = data_dir
            .canonicalize()
            .map_err(|e| to_files_error(Error::Io(e)))?;
        Ok(Self {
            data_dir,
            confine_root,
            boundaries: None,
            registry: Arc::new(registry),
            versions: VaultVersions::new(vault_root),
            repos: Arc::new(Mutex::new(HashMap::new())),
            root_locks: Arc::new(Mutex::new(HashMap::new())),
            cadence: Arc::new(CadenceEngine::new(config, clock)),
            ignores: Arc::new(Mutex::new(HashMap::new())),
            watchers: Arc::new(Mutex::new(HashMap::new())),
            watch_new_roots: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            driver: Arc::new(Mutex::new(None)),
            hook: Arc::new(Mutex::new(None)),
            flip_hook: Arc::new(Mutex::new(None)),
            transcoder: Arc::new(Mutex::new(None)),
            rendition_stores: Arc::new(Mutex::new(HashMap::new())),
            rendition_gen_locks: Arc::new(Mutex::new(HashMap::new())),
            rendition_open_lock: Arc::new(tokio::sync::Mutex::new(())),
            events: architect::PubSub::sliding(256),
        })
    }

    /// Inject the transcoder that generates derived media (issue #269).
    /// The server sets `files_transcode::FfmpegTranscoder`; a test sets
    /// a fake. Until set, `rendition` fails NotFound and checkpoints
    /// take no warm-up.
    pub fn set_transcoder(&self, transcoder: Arc<dyn files_transcode::Transcoder>) {
        *self.transcoder.lock().expect("transcoder lock") = Some(transcoder);
    }

    /// The cadence engine driving this backend's sessions.
    #[must_use]
    pub fn cadence(&self) -> &Arc<CadenceEngine> {
        &self.cadence
    }

    /// Install the certification test seam (see
    /// [`crate::certify::MidHashHook`]): a callback run between the
    /// pre-read `stat` of each file and the read itself, so a test can
    /// make a file change mid-hash deterministically. Production never
    /// calls this.
    #[doc(hidden)]
    pub fn set_mid_hash_hook(&self, hook: Option<MidHashHook>) {
        *self.hook.lock().expect("hook lock poisoned") = hook;
    }

    /// Test seam only: runs between a restart's terminal checkpoint
    /// and its clear phase — the mid-flip window AC 3 of issue #268 is
    /// about. Production never sets one.
    pub fn set_mid_flip_hook(&self, hook: Option<MidHashHook>) {
        *self.flip_hook.lock().expect("flip hook lock poisoned") = hook;
    }

    #[must_use]
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// This org's files area — the boundary every caller-supplied path
    /// is confined to (see the module doc's "Filesystem confinement").
    ///
    /// Exposed as the boundary itself rather than as a
    /// `is_confined(path) -> bool` helper so that other surfaces over
    /// the same roots — the WebDAV bridge (`files-webdav`, issue #274)
    /// checks a root's live tree before handing a filesystem view of it
    /// to a network client — can call [`task_files_util::confine`]
    /// directly and *keep the error kind*. A `bool` collapses
    /// `PathError::Escapes` (a genuine confinement breach, alert-worthy)
    /// into `PathError::Io` (a temporarily-unmounted volume, EIO), and
    /// reporting the second as the first is both a false alarm and the
    /// wrong status code (PR #287 review).
    /// Permit live trees inside this org's Storage Locations, on top of
    /// its own directory. Without this a backend confines to the org
    /// directory alone — the pre-locations behaviour, and still the right
    /// answer for a single-machine deployment.
    #[must_use]
    pub fn with_location_boundaries(mut self, boundaries: Arc<dyn LocationBoundaries>) -> Self {
        self.boundaries = Some(boundaries);
        self
    }

    #[must_use]
    pub fn confine_root(&self) -> &Path {
        &self.confine_root
    }

    /// Every registered root, unprojected — the org-tree resolver's
    /// join input (the lineage overlay is browse-time garnish it
    /// doesn't need).
    pub(crate) fn registry_list(&self) -> Vec<FileRootInfo> {
        self.registry.list()
    }

    /// The org vault the curated version entities live in.
    #[must_use]
    pub fn vault_root(&self) -> &Path {
        self.versions.vault_root()
    }

    /// Run `f` against one root's live version-store backend — the
    /// spec's "secondary harness" seam (Testing Decisions), for the
    /// store-level properties that are invisible at the RPC surface:
    /// chunk presence after a GC pass, dedup ratios, streaming.
    ///
    /// It hands out the *cached* repo's backend rather than opening a
    /// second one, which matters: two `FsStore`s over one on-disk
    /// chunk store in a single process is the shape that used to hang
    /// (see `tests/rpc_surface.rs`). `f` is synchronous; drive any
    /// async work in it with `pollster::block_on`, as this crate does
    /// everywhere it touches jj-lib.
    ///
    /// Media roots only — a software root's objects are git's, and
    /// there is no [`VersionStoreBackend`] under it. Use
    /// [`FilesBackend::with_repo`] for anything flavor-agnostic.
    pub fn with_version_store<R>(
        &self,
        root_id: Uuid,
        f: impl FnOnce(&VersionStoreBackend) -> R,
    ) -> Result<R, FilesError> {
        self.with_repo(root_id, |repo| {
            let backend = repo
                .store()
                .backend_impl::<VersionStoreBackend>()
                .ok_or_else(|| {
                    to_files_error(Error::Repo(
                        "root's repo is not a VersionStoreBackend".into(),
                    ))
                })?;
            Ok(f(backend))
        })?
    }

    /// [`FilesBackend::with_version_store`] one level lower: the cached
    /// jj repo handle itself, for the store-level properties that need
    /// a transaction rather than just the backend.
    ///
    /// Deliberately the **cached** handle, never a reloaded one — a
    /// test that writes a commit through it and doesn't touch the cache
    /// reproduces exactly what a second process does to this one: the
    /// op log on disk moves forward while this backend's handle stays
    /// where it was. That is the condition [`FilesBackend::reload_repo`]
    /// exists for, and it cannot be built with two `FilesBackend`s in
    /// one process — two `FsStore`s over one store hangs (see
    /// `tests/rpc_surface.rs`).
    pub fn with_repo<R>(
        &self,
        root_id: Uuid,
        f: impl FnOnce(&Arc<ReadonlyRepo>) -> R,
    ) -> Result<R, FilesError> {
        let root = self.get_root_info(root_id).map_err(to_files_error)?;
        let (repo, _head) = self.ensure_repo(&root).map_err(to_files_error)?;
        Ok(f(&repo))
    }

    /// Fabricate a divergence for DEV/DEMO SEEDING: two checkpoints off
    /// the current head, each writing `path` differently, so the file
    /// ends up divergent and `divergences` / `resolve_divergence` have
    /// something to operate on. The ordinary write path never produces
    /// this — concurrent saves on two replicas do — so this is the
    /// deterministic stand-in a seed needs, built on the same public
    /// `with_repo` + version-store `checkpoint` primitive the divergence
    /// tests use.
    pub async fn seed_divergent_file(
        &self,
        root_id: Uuid,
        path: &str,
        side_a: &[u8],
        side_b: &[u8],
    ) -> Result<(), FilesError> {
        use jj_lib::repo_path::RepoPathBuf;
        use task_files_version_store::checkpoint::{Change, checkpoint};

        let this = self.clone();
        let path = path.to_string();
        let (a, b) = (side_a.to_vec(), side_b.to_vec());
        tokio::task::spawn_blocking(move || {
            this.with_repo(root_id, |repo| {
                let base =
                    repo.view().heads().iter().next().cloned().ok_or_else(|| {
                        FilesError::NotFound("no checkpoint head to diverge".into())
                    })?;
                let rp = RepoPathBuf::from_internal_string(&path)
                    .map_err(|e| FilesError::BadRequest(format!("{path}: {e:?}")))?;
                pollster::block_on(checkpoint(
                    repo,
                    base.clone(),
                    vec![Change::Write {
                        path: rp.clone(),
                        content: a,
                    }],
                    "seed: side A",
                ))
                .map_err(|e| FilesError::Io(format!("seed side A: {e}")))?;
                pollster::block_on(checkpoint(
                    repo,
                    base,
                    vec![Change::Write {
                        path: rp,
                        content: b,
                    }],
                    "seed: side B",
                ))
                .map_err(|e| FilesError::Io(format!("seed side B: {e}")))?;
                Ok::<(), FilesError>(())
            })?
        })
        .await
        .map_err(|e| FilesError::Io(format!("seed join: {e}")))?
    }

    fn publish(&self, event: FilesEvent) {
        self.events.publish(event);
    }

    /// Best-effort flush of every cached root's chunk store
    /// (`ChunkStore::shutdown`) — call before dropping a `FilesBackend`
    /// whose process is about to reopen the same roots (a real server
    /// exit, or a test simulating a restart). Not required for the
    /// correctness of any RPC method — jj-lib's own commit path is
    /// already durable — but iroh-blobs' `FsStore` may hold buffered
    /// writes / file-backed resources open until this (or the process)
    /// actually exits; see `ChunkStore::shutdown`'s own doc.
    /// Also stops the cadence (issue #260): the driver task is aborted
    /// and every watcher dropped, so a backend that has been shut down
    /// is inert rather than still ticking against a store the next
    /// backend is about to open (PR #283 review).
    pub async fn shutdown(&self) {
        if let Some(driver) = self.driver.lock().expect("driver lock poisoned").take() {
            driver.abort();
        }
        self.watchers.lock().expect("watcher map poisoned").clear();
        let repos: Vec<Arc<ReadonlyRepo>> = self
            .repos
            .lock()
            .expect("repo cache lock poisoned")
            .values()
            .map(|rt| rt.repo.clone())
            .collect();
        for repo in repos {
            if let Some(backend) = repo.store().backend_impl::<VersionStoreBackend>() {
                let _ = backend.chunks().shutdown().await;
            }
        }
    }

    /// The registry's own record — no Vault lookups. Every inner
    /// caller (`browse` / `chain` / `checkpoint_now` / curation) wants
    /// exactly this; only `list_roots`/`get_root` project the lineage
    /// badge on top (see [`FilesBackend::with_project_version`]), so
    /// the hot paths never pay for a vault scan they don't read (PR
    /// #288 review).
    fn get_root_info(&self, id: Uuid) -> Result<FileRootInfo, Error> {
        self.registry
            .get(id)
            .ok_or_else(|| Error::NotFound(id.to_string()))
    }

    /// Project each root's CURRENT lineage — its highest-numbered
    /// [`ProjectVersion`] entity (issue #261) — onto the roots
    /// `list_roots`/`get_root` return. ONE vault scan for the whole
    /// list, not one per root, and a vault that can't be read degrades
    /// to un-badged roots rather than failing the listing: the badge is
    /// decoration on a registry-owned answer.
    fn with_project_version(&self, mut roots: Vec<FileRootInfo>) -> Vec<FileRootInfo> {
        let mut current: HashMap<Uuid, ProjectVersion> = HashMap::new();
        match self.versions.all_project_versions() {
            Ok(all) => {
                for pv in all {
                    current
                        .entry(pv.root_id)
                        .and_modify(|held| {
                            if pv.number > held.number {
                                *held = pv.clone();
                            }
                        })
                        .or_insert(pv);
                }
            }
            Err(e) => tracing::warn!(
                ?e,
                "reading Project Versions failed; listing roots without lineage badges"
            ),
        }
        for root in &mut roots {
            root.project_version = current.get(&root.id).cloned();
        }
        roots
    }

    /// Canonicalize `requested` and confirm it resolves inside
    /// [`FilesBackend::confine_root`] — the org-scoping check for
    /// `create_root` (a not-yet-existing marker means `requested`
    /// itself must exist as a directory, checked by the caller first)
    /// and `drive_browse`.
    ///
    /// The check itself is `task_files_util::confine`, shared with
    /// `files-storage`'s grant-prefix enforcement: it was written three
    /// times across the platform, so a hardening fix to one copy left
    /// the others escapable (PR #284 review).
    /// Resolve a caller-supplied path, refusing anything outside a
    /// permitted boundary.
    ///
    /// The org's own directory is always permitted. Beyond it, a path is
    /// permitted when it falls inside a Storage Location this org holds a
    /// live-tree grant on (issue #262) — which is how a File Root can
    /// point at media on a NAS that was never going to fit in the org
    /// directory, without the boundary check degrading into "anywhere on
    /// the filesystem".
    ///
    /// Order matters for the error message: the org directory is tried
    /// first and its rejection is what the caller sees when NO boundary
    /// matches, because "outside `<org>/files`" is the answer that makes
    /// sense on the overwhelming majority of deployments, which have no
    /// locations at all.
    /// Re-register a folder that carries a marker this registry has
    /// never seen — restored from a backup, or carried over from
    /// another deployment. Its id and name come from the marker, not
    /// from the caller, so everything that already references the root
    /// (Named Versions, reviews, placements) keeps resolving.
    ///
    /// The store inside the folder is opened, never re-initialised: the
    /// history came with it.
    fn adopt_marked_root(
        &self,
        canonical: &Path,
        canonical_str: String,
        marker: RootMarker,
        flavor: RootFlavor,
    ) -> Result<FileRootInfo, Error> {
        let repo = repo_open::open_or_init_repo(canonical, flavor)?;
        let head = Self::head_of(&repo, flavor)?;
        let root = FileRootInfo {
            id: marker.id,
            name: marker.name,
            path: canonical_str,
            flavor,
            created_at: Utc::now(),
            project_version: None,
        };
        self.registry.insert(root.clone())?;
        self.set_heads(root.id, repo, head, None);
        self.ignore_of(&root)?;
        if self
            .watch_new_roots
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            if let Err(err) = self.watch_root(root.id) {
                tracing::warn!(root_id = %root.id, ?err, "files: adopted root not watched");
            }
        }
        tracing::info!(root = %root.id, path = %root.path, "files: adopted a marked root");
        self.publish(FilesEvent::RootCreated(root.clone()));
        Ok(root)
    }

    fn confine(&self, requested: &Path) -> Result<PathBuf, Error> {
        let own = task_files_util::confine(requested, &self.confine_root);
        if own.is_ok() {
            return own.map_err(confinement);
        }
        if let Some(boundaries) = &self.boundaries {
            for boundary in boundaries.permitted() {
                if let Ok(path) = task_files_util::confine(requested, &boundary) {
                    return Ok(path);
                }
            }
        }
        own.map_err(confinement)
    }

    /// The commit a checkpoint on this root builds on. Media roots read
    /// jj's own view head; software roots follow git's checked-out
    /// branch instead, so checkpoints continue the branch a developer
    /// (or CI) is actually on rather than an arbitrary head of a repo
    /// that may carry many (see [`git_root::head_commit`]).
    fn head_of(repo: &Arc<ReadonlyRepo>, flavor: RootFlavor) -> Result<CommitId, Error> {
        match flavor {
            RootFlavor::Software => git_root::head_commit(repo),
            RootFlavor::Media => Ok(repo
                .view()
                .heads()
                .iter()
                .next()
                .cloned()
                .unwrap_or_else(|| repo.store().root_commit_id().clone())),
        }
    }

    /// Backend + current head for `root`, opening (and caching) the
    /// repo on first touch.
    ///
    /// A software root's cached repo is *refreshed* on every call, not
    /// just opened once: its history has a second author (a human or CI
    /// running plain `git` in the same checkout), and serving a cached
    /// view would mean chains that never show their commits and
    /// checkpoints that parent onto a stale head, forking history behind
    /// git's back (PR #282 review). Refreshing is a git-ref read, which
    /// is what the colocated promise costs. Media roots have no second
    /// author — Files owns their store outright — so their cache stands.
    fn ensure_repo(&self, root: &FileRootInfo) -> Result<(Arc<ReadonlyRepo>, CommitId), Error> {
        let cached = {
            let repos = self.repos.lock().expect("repo cache lock poisoned");
            repos
                .get(&root.id)
                .map(|rt| (rt.repo.clone(), rt.head.clone()))
        };
        let repo = match (cached, root.flavor) {
            (Some((repo, head)), RootFlavor::Media) => return Ok((repo, head)),
            (Some((repo, _)), RootFlavor::Software) => git_root::import_from_git(repo)?,
            (None, _) => repo_open::open_or_init_repo(Path::new(&root.path), root.flavor)?,
        };
        let (head, snapshot_head) = Self::heads_of(&repo, root)?;
        self.repos.lock().expect("repo cache lock poisoned").insert(
            root.id,
            RootRuntime {
                repo: repo.clone(),
                head: head.clone(),
                snapshot_head,
            },
        );
        Ok((repo, head))
    }

    /// The root's `(checkpoint head, snapshot-branch tip)`.
    ///
    /// The cadence journal, not the view, is what says which head is the
    /// *checkpoint* head (issue #260): a root mid-session carries a
    /// snapshot branch alongside its checkpoint line, so "the first view
    /// head" is a coin flip between them — and picking the snapshot
    /// would put ephemeral captures straight into every version chain,
    /// which is exactly what branching them was for. The journal is also
    /// the right authority across processes: whoever writes a checkpoint
    /// rewrites it atomically in the same breath, so a second writer's
    /// checkpoint is visible here the moment it lands.
    ///
    /// Media only. Git is a software root's authority — [`head_of`]
    /// already follows its checked-out branch, and that flavor takes no
    /// auto-snapshots at all (see [`FilesBackend::capture_inner`]).
    fn heads_of(
        repo: &Arc<ReadonlyRepo>,
        root: &FileRootInfo,
    ) -> Result<(CommitId, Option<CommitId>), Error> {
        if root.flavor == RootFlavor::Software {
            return Ok((Self::head_of(repo, root.flavor)?, None));
        }
        let journal = Self::journal_of(root)?;
        let snapshot_head = journal
            .snapshot_head
            .as_deref()
            .and_then(CommitId::try_from_hex);
        let Some(recorded) = journal
            .checkpoint_head
            .as_deref()
            .and_then(CommitId::try_from_hex)
        else {
            // No journal (a root that has never captured, or one whose
            // journal was lost): the view is all there is.
            return Ok((Self::head_of(repo, root.flavor)?, snapshot_head));
        };

        // The journal names where *our* checkpoint line was; the view
        // may have moved past it. A writer with no journal of its own —
        // raw `jj`, a test writing straight through the `Backend` trait
        // — leaves a view head that descends from the recorded one, and
        // that head is the honest answer (#286's "a checkpoint written
        // behind the cache" case). Snapshot commits are excluded by id
        // rather than by ancestry: they descend from the recorded head
        // too, and following one would put every ephemeral capture back
        // into the version chain.
        let known_snapshots: std::collections::HashSet<String> = journal
            .snapshots
            .iter()
            .map(|s| s.snapshot_id.clone())
            .chain(journal.snapshot_head.clone())
            .collect();
        let mut head = recorded;
        for candidate in repo.view().heads() {
            if *candidate == head || known_snapshots.contains(&candidate.hex()) {
                continue;
            }
            let descends = pollster::block_on(repo.index().is_ancestor(&head, candidate))
                .map_err(|e| Error::Repo(format!("comparing heads: {e}")))?;
            if descends {
                head = candidate.clone();
            }
        }
        Ok((head, snapshot_head))
    }

    /// The root's cadence journal (issue #260).
    fn journal_of(root: &FileRootInfo) -> Result<Journal, Error> {
        Journal::load(&repo_open::store_dir(Path::new(&root.path)))
    }

    /// The tip of the root's auto-snapshot branch, if its session has
    /// taken one since the last checkpoint.
    fn snapshot_head_of(&self, root_id: Uuid) -> Option<CommitId> {
        self.repos
            .lock()
            .expect("repo cache lock poisoned")
            .get(&root_id)
            .and_then(|rt| rt.snapshot_head.clone())
    }

    /// [`FilesBackend::ensure_repo`], but re-read from the op log
    /// first — the only honest input for anything that walks the DAG.
    ///
    /// The cache is only ever advanced by *this* process's own writes
    /// (`create_root` / `checkpoint_now` call `set_head`), and
    /// `root_locks` is a `Mutex` in this process's memory, not a lock
    /// on disk. A second process writing the same store is a real,
    /// shipped path: `establish_for_url` falls back to the CLI's own
    /// embedded backend whenever the dial fails, so `task files
    /// checkpoint` can write commits the server's cached handle has
    /// never seen. Sweeping from that stale index would treat those
    /// commits as unreachable garbage, and `keep_newer` doesn't save
    /// them — it is a race guard against writes happening *now*, not
    /// against a handle that has been stale for an hour.
    ///
    /// `reload_at_head` goes through the repo's own `RepoLoader`, so
    /// it reuses this root's existing `Store` (and the one `FsStore`
    /// under it) rather than opening a second one — see
    /// `with_version_store`'s doc for why that distinction matters.
    ///
    /// **Software roots need nothing extra here.** Their authority is
    /// git, and [`FilesBackend::ensure_repo`] already re-imports its
    /// refs on every call for exactly the same reason this exists —
    /// that flavor's "second author" is a developer running plain
    /// `git`, ours is a second process on the same store. Re-reading
    /// the op log on top of a fresh import would be a second answer to
    /// a question git has already answered.
    fn reload_repo(&self, root: &FileRootInfo) -> Result<(Arc<ReadonlyRepo>, CommitId), Error> {
        let (cached, head) = self.ensure_repo(root)?;
        if root.flavor == RootFlavor::Software {
            return Ok((cached, head));
        }
        let repo = pollster::block_on(cached.reload_at_head())
            .map_err(|e| Error::Repo(format!("reloading {} at head: {e}", root.id)))?;
        let (head, snapshot_head) = Self::heads_of(&repo, root)?;
        self.set_heads(root.id, repo.clone(), head.clone(), snapshot_head);
        Ok((repo, head))
    }

    /// The read-path counterpart of [`FilesBackend::reload_repo`]:
    /// open the root's store **only if it already exists**, then
    /// re-read it at head. `Ok(None)` when the root has no store yet
    /// (or its volume is not mounted) — a read must never initialize
    /// one, and must never serve a snapshot frozen at this process's
    /// last write (PR #288 review; the same staleness `reload_repo`
    /// exists for on the write/GC side).
    fn reload_existing_repo(
        &self,
        root: &FileRootInfo,
    ) -> Result<Option<(Arc<ReadonlyRepo>, CommitId)>, Error> {
        // Disk first, cache second: a root whose volume went away
        // still has a live handle in this process, and reloading that
        // handle at head fails with a bare "Failed to read operation
        // heads" where the honest answer is "there is no store here
        // right now". Evict it so a remount reopens cleanly.
        if !repo_open::store_dir(Path::new(&root.path)).exists() {
            self.repos
                .lock()
                .expect("repo cache lock poisoned")
                .remove(&root.id);
            return Ok(None);
        }
        let cached = {
            let repos = self.repos.lock().expect("repo cache lock poisoned");
            repos.get(&root.id).map(|rt| rt.repo.clone())
        };
        let repo = match cached {
            Some(repo) => match root.flavor {
                // Git is the authority for a software root, and
                // importing its refs is how the jj view catches up.
                RootFlavor::Software => git_root::import_from_git(repo)?,
                RootFlavor::Media => pollster::block_on(repo.reload_at_head())
                    .map_err(|e| Error::Repo(format!("reloading {} at head: {e}", root.id)))?,
            },
            None => match repo_open::open_existing_repo(Path::new(&root.path), root.flavor)? {
                Some(repo) => repo,
                None => return Ok(None),
            },
        };
        let (head, snapshot_head) = Self::heads_of(&repo, root)?;
        self.set_heads(root.id, repo.clone(), head.clone(), snapshot_head);
        Ok(Some((repo, head)))
    }

    /// Set both heads at once — what a capture does (issue #260): a
    /// checkpoint moves the line and closes the branch, a snapshot
    /// leaves the line alone and extends the branch.
    fn set_heads(
        &self,
        root_id: Uuid,
        repo: Arc<ReadonlyRepo>,
        head: CommitId,
        snapshot_head: Option<CommitId>,
    ) {
        self.repos.lock().expect("repo cache lock poisoned").insert(
            root_id,
            RootRuntime {
                repo,
                head,
                snapshot_head,
            },
        );
    }

    fn root_lock(&self, root_id: Uuid) -> Arc<Mutex<()>> {
        self.root_locks
            .lock()
            .expect("root lock map poisoned")
            .entry(root_id)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    fn create_root_inner(
        &self,
        path: String,
        name: String,
        flavor: RootFlavor,
    ) -> Result<FileRootInfo, Error> {
        let requested = PathBuf::from(&path);
        let metadata =
            std::fs::metadata(&requested).map_err(|e| Error::BadRequest(format!("{path}: {e}")))?;
        if !metadata.is_dir() {
            return Err(Error::BadRequest(format!("{path}: not a directory")));
        }
        // Org confinement (see module doc) — before anything else, so
        // a rejected path never even reaches the marker/registry
        // checks below.
        let canonical = self.confine(&requested)?;
        let canonical_str = canonical
            .to_str()
            .ok_or_else(|| Error::BadRequest(format!("{path}: not valid UTF-8")))?
            .to_string();

        // A marker means this folder was already a root. That is not
        // automatically a conflict — it is how a MOVED or RENAMED root
        // announces itself, and moving folders around is the normal way
        // this material gets organised.
        //
        // A root records the absolute path it was created at, while its
        // marker and its whole version store live inside the folder and
        // travel with it. So when the marker names a root whose
        // registered path is no longer where the folder is, the right
        // answer is to re-point the registry, not to refuse: refusing
        // strands the root permanently — the old path is dead and the
        // real folder can never be re-added.
        //
        // Only an exact re-registration of a root already living here is
        // a genuine duplicate.
        if let Some(existing) = read_root_marker(&canonical) {
            return match self.registry.get(existing.id) {
                Some(known) if Path::new(&known.path) == canonical => {
                    Err(Error::AlreadyExists(canonical_str))
                }
                // Known root, different path: the folder MOVED. The
                // right answer is to re-point the registry and carry on
                // — the marker and the whole version store travelled
                // with the folder, so nothing is actually lost — but
                // that cannot be done safely yet, and a wrong attempt
                // is worse than a clear refusal:
                //
                // - updating only the registry leaves the open repo
                //   addressing the OLD directory, and reads fail with
                //   "commit not found";
                // - re-opening at the new path HANGS, because the
                //   store's redb lock is held by a handle that dropping
                //   the cached `Arc` does not release — iroh-blobs runs
                //   its own runtime and needs an explicit shutdown.
                //
                // So this needs a close-the-root's-store seam first
                // (see plans/media-roots-at-scale.md). Until then, say
                // exactly what happened and where the folder was
                // registered, which is at least actionable.
                Some(known) => Err(Error::BadRequest(format!(
                    "{canonical_str} is root {} ({}), which was registered at {} — \
                     moving or renaming a registered root is not supported yet; \
                     move it back, or restart the server to pick it up at its new path",
                    known.id, known.name, known.path
                ))),
                // Marker for a root this registry has never seen: a
                // folder restored from a backup, or carried in from
                // another deployment. Adopt it under its own id so its
                // existing history stays reachable.
                None => self.adopt_marked_root(&canonical, canonical_str, existing, flavor),
            };
        }
        // Ancestor/descendant containment, not just exact-path — roots
        // never overlap on disk (glossary "File Root"); an outer root
        // whose live tree contains an inner root's `.fts-files` would
        // otherwise ingest that inner root's entire version store as
        // ordinary content on every checkpoint.
        if let Some(existing) = self.registry.conflicting_root(&canonical) {
            return Err(Error::AlreadyExists(format!(
                "{canonical_str} overlaps existing root {} ({})",
                existing.id, existing.path
            )));
        }

        let repo = repo_open::open_or_init_repo(&canonical, flavor)?;
        let head = Self::head_of(&repo, flavor)?;

        let id = Uuid::new_v4();
        let created_at = Utc::now();
        let marker = serde_json::json!({ "id": id, "name": name });
        std::fs::write(
            canonical.join(MARKER_FILE),
            serde_json::to_vec_pretty(&marker)?,
        )?;

        let root = FileRootInfo {
            id,
            name,
            path: canonical_str,
            flavor,
            created_at,
            // A freshly created root is lineage 1 with no restart
            // behind it, so it wears no Project Version badge until
            // one is recorded in its marker (issue #261).
            project_version: None,
        };
        self.registry.insert(root.clone())?;
        self.set_heads(id, repo, head, None);
        // Compile the Ignore set now, at creation, so the very first
        // capture already excludes the flavor's junk (glossary: "seeded
        // by root flavor, edited per root").
        self.ignore_of(&root)?;
        if self
            .watch_new_roots
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            if let Err(err) = self.watch_root(id) {
                tracing::warn!(root_id = %id, ?err, "files: new root not watched");
            }
        }
        self.publish(FilesEvent::RootCreated(root.clone()));
        Ok(root)
    }

    /// `hide_internals` hides the root's own bookkeeping (the marker
    /// file and the version store) — set only when listing a root's top
    /// level through `browse`, never through `drive_browse`, which shows
    /// the raw tree. `hide_git` additionally hides `.git`, which is a
    /// root's own object store on the software flavor but ordinary
    /// content on a media one.
    pub(crate) fn list_dir(
        dir: &Path,
        hide_internals: bool,
        hide_git: bool,
    ) -> Result<Vec<BrowseEntry>, Error> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let name_os = entry.file_name();
            let Some(name) = name_os.to_str() else {
                continue; // non-UTF8 names are out of scope for v1
            };
            if hide_internals && (name == MARKER_FILE || name == STORE_DIR) {
                continue;
            }
            if hide_git && name == GIT_DIR {
                continue;
            }
            let file_type = entry.file_type()?;
            let size = if file_type.is_file() {
                Some(entry.metadata()?.len())
            } else {
                None
            };
            out.push(BrowseEntry {
                name: name.to_string(),
                is_dir: file_type.is_dir(),
                size,
                // Resident by definition (this listing is the live
                // tree); root browsing overlays the version store's
                // stub/divergence state in `browse_inner`, Drive
                // browsing has no root context and leaves both false.
                stub: false,
                divergent: false,
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    /// Root-scoped browse. The escape guard is canonicalize-then-
    /// prefix-check against the root's own (already-canonical)
    /// `path`, not a component-string scan: `root_path.join(subpath)`
    /// with an ABSOLUTE `subpath` replaces the base entirely (std
    /// `PathBuf::join` semantics), so a `..`-free string like `/etc`
    /// would otherwise sail through. Canonicalizing the resolved
    /// target also follows symlinks to their real location, so a
    /// symlink inside the root pointing outside it is caught by the
    /// same prefix check — resolving the true escape, not just the
    /// textual one.
    fn browse_inner(&self, root_id: Uuid, subpath: String) -> Result<Vec<BrowseEntry>, Error> {
        let root = self.get_root_info(root_id)?;
        let root_path = PathBuf::from(&root.path);
        let requested = if subpath.is_empty() {
            root_path.clone()
        } else {
            root_path.join(&subpath)
        };
        // A subpath that isn't on disk may still be TRACKED — a
        // directory whose whole content is pointer stubs (issue #266).
        // The store answers for it; the escape guard is the repo-path
        // parse (jj rejects `..` and absolute components), so this
        // branch can't reach outside the root either.
        //
        // ONLY `NotFound` falls through to the store: EACCES, ELOOP,
        // EIO or an unmounted volume mean we cannot see the live tree,
        // and answering from the store would report every resident file
        // as a stub (PR #288 review). Those propagate. An absolute
        // subpath is refused here too — `repo_dir` would otherwise trim
        // its leading `/` and answer with a root-relative listing.
        if Path::new(&subpath).is_absolute() {
            return Err(Error::BadRequest(format!(
                "subpath escapes the root: {subpath}"
            )));
        }
        let canonical_target = match requested.canonicalize() {
            Ok(target) => Some(target),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(Error::Io(e)),
        };
        if let Some(target) = &canonical_target {
            // The resident case still goes through the platform's one
            // confinement check (PR #284 review) rather than an inline
            // prefix compare — same guard `files-storage` applies to a
            // Storage grant's prefix, so a hardening fix reaches both.
            task_files_util::confine(target, &root_path).map_err(|e| match e {
                task_files_util::PathError::Escapes { .. } => {
                    Error::BadRequest(format!("subpath escapes the root: {subpath}"))
                }
                other => Error::BadRequest(other.to_string()),
            })?;
            if !std::fs::metadata(target)?.is_dir() {
                return Err(Error::BadRequest(format!("{subpath}: not a directory")));
            }
        }
        // `.git` is hidden at every depth on a software root (a nested
        // one is a submodule's object store — not this root's content),
        // while the marker/store pair only ever exists at the top level.
        let mut entries = match &canonical_target {
            Some(target) => {
                let mut listed = Self::list_dir(
                    target,
                    *target == root_path,
                    root.flavor == RootFlavor::Software,
                )?;
                // On-disk pointer stubs (issue #263) are resident *files*
                // to the raw listing but stubs to the platform: flag them
                // and report the LOGICAL size the stub preserves, not the
                // placeholder's own few bytes. Detection is stat-bounded —
                // only a file small enough to be a stub has its header
                // read, so listing a directory of media opens nothing.
                // Lenient per file: one vanished/unreadable/malformed
                // small file lists as the ordinary file it appears to
                // be rather than failing the whole directory (PR #289
                // review). Media only — software roots have no stubs.
                if root.flavor == RootFlavor::Media {
                    for entry in &mut listed {
                        if entry.is_dir {
                            continue;
                        }
                        if let Some(len) = entry.size
                            && stub::candidate_len(len)
                            && let Some(s) = stub::probe(&target.join(&entry.name))
                        {
                            entry.stub = true;
                            entry.size = Some(s.size);
                        }
                    }
                }
                listed
            }
            None => Vec::new(),
        };
        // Overlay the version store's view: tracked-but-not-resident
        // paths join the listing as pointer stubs, and paths whose
        // state differs between visible heads wear the divergence badge
        // (issue #266's explorer renders both). Reading is
        // OPEN-ONLY — browsing must never initialize a store (PR #288
        // review) — and reloads to head first, because the cached
        // handle is only advanced by this process's own writes and a
        // second writer (the CLI's embedded backend, the cadence
        // engine) would otherwise stay invisible forever.
        let dir = badges::repo_dir(&subpath)?;
        let tracked = match self.reload_existing_repo(&root)? {
            Some((repo, head)) => {
                let heads: BTreeSet<CommitId> = match root.flavor {
                    // A software root's authority is git's refs, whose
                    // head `head_of` already resolved; jj's op-log view
                    // is an import of it, not a second opinion.
                    RootFlavor::Software => BTreeSet::from([head.clone()]),
                    RootFlavor::Media => repo.view().heads().iter().cloned().collect(),
                };
                badges::tracked_dir(repo.store().backend(), &head, &heads, &dir)?
            }
            // No store yet (never checkpointed, or the volume is not
            // mounted): the live tree is the whole truth.
            None => badges::TrackedDir::empty(),
        };
        if canonical_target.is_none() && tracked.is_empty() {
            // Neither on disk nor in any head's tree.
            return Err(Error::NotFound(format!("{root_id}:{subpath}")));
        }
        badges::annotate(&tracked, &mut entries);
        Ok(entries)
    }

    fn drive_browse_inner(&self, path: String) -> Result<Vec<BrowseEntry>, Error> {
        let confined = self.confine(Path::new(&path))?;
        let metadata = std::fs::metadata(&confined)?;
        if !metadata.is_dir() {
            return Err(Error::BadRequest(format!("{path}: not a directory")));
        }
        Self::list_dir(&confined, false, false)
    }

    fn chain_inner(&self, root_id: Uuid, path: String) -> Result<Vec<ChainEntry>, Error> {
        let root = self.get_root_info(root_id)?;
        let (repo, head) = self.ensure_repo(&root)?;
        // Both flavors derive chains through the same DAG walk, against
        // jj's `Backend` trait rather than either concrete backend —
        // that is what "the chain/history RPC works identically on a
        // software root" means in code (issue #273).
        let backend = repo.store().backend();
        let repo_path = RepoPathBuf::from_internal_string(&path)
            .map_err(|e| Error::BadRequest(format!("{path:?}: {e}")))?;
        let entries = pollster::block_on(task_files_version_store::chain::version_chain(
            backend, &head, &repo_path,
        ))?;
        // Curated metadata (issue #261): the Vault, not the store, is
        // where names live — so every chain read resolves them fresh
        // from the vault pages rather than caching a projection. That
        // costs one vault scan per call, the same live-scan bargain
        // every other vault-backed slice makes (`WorkstreamBackend`);
        // if it ever measures slow, the fix is a shared vault snapshot
        // on the backend, never a second authority on names.
        //
        // Names are decoration on a store-owned answer, so a vault
        // that can't be read degrades this call to an uncurated chain
        // rather than failing it — the opposite of `protected_commits`,
        // where an unreadable page must stop the sweep.
        let mut names_by_commit: HashMap<String, Vec<String>> = HashMap::new();
        match self.versions.named_versions(Some(root_id)) {
            Ok(named) => {
                for named in named {
                    names_by_commit
                        .entry(named.commit_id)
                        .or_default()
                        .push(named.name);
                }
            }
            Err(e) => tracing::warn!(
                %root_id,
                ?e,
                "reading Named Versions failed; serving the chain uncurated"
            ),
        }
        // Save points are the automatic counterpart of names: also
        // metadata the commit graph does not hold (glossary — "display
        // metadata, not a version"), joined on here from the root's
        // cadence journal, which records them against the checkpoint
        // that closed the session they were marked in (issue #260).
        let journal = Self::journal_of(&root).unwrap_or_default();
        Ok(entries
            .into_iter()
            .map(|e| {
                let commit_id = e.commit_id.hex();
                let mut names = names_by_commit.get(&commit_id).cloned().unwrap_or_default();
                names.sort();
                ChainEntry {
                    save_points: journal.save_points_for(&commit_id),
                    commit_id,
                    path: e.path.as_internal_file_string().to_string(),
                    file_id: e.file_id.hex(),
                    renamed_from: e
                        .renamed_from
                        .map(|p| p.as_internal_file_string().to_string()),
                    names,
                }
            })
            .collect())
    }

    /// The `(commit, change)` pair `commit_ref` names in `root`'s
    /// store — the validation every curation write does before writing
    /// a Vault entity, so a reference can never name a commit that
    /// isn't there.
    ///
    /// `commit_ref` may be a full hex id or an unambiguous hex prefix,
    /// because a prefix is what every human-facing surface prints
    /// (`task files chain` shows twelve characters, and jj itself is
    /// prefix-addressed throughout). An ambiguous prefix is a bad
    /// request, never a coin flip.
    ///
    /// Goes through jj's `Backend` trait rather than
    /// [`VersionStoreBackend`], so curation works the same on both
    /// flavors: a Named Version of a commit in a software root's
    /// colocated git repo is an ordinary Vault entity like any other
    /// (issue #273 generalized the chain and the checkpoint writer the
    /// same way — naming is no different).
    fn resolve_commit(
        &self,
        root: &FileRootInfo,
        commit_ref: &str,
    ) -> Result<(CommitId, ChangeId), Error> {
        let (repo, _head) = self.ensure_repo(root)?;
        Self::resolve_commit_in(&repo, root, commit_ref)
    }

    /// [`FilesBackend::resolve_commit`] against a repo handle the
    /// caller already holds — the read-path variant: it opens nothing,
    /// so a read surface (`browse_at`) can resolve without the
    /// store-initializing side effect `ensure_repo` carries (the
    /// read-must-never-init rule of PR #288, re-flagged for time-travel
    /// browsing by PR #290's review).
    fn resolve_commit_in(
        repo: &Arc<ReadonlyRepo>,
        root: &FileRootInfo,
        commit_ref: &str,
    ) -> Result<(CommitId, ChangeId), Error> {
        let backend = repo.store().backend();

        // A full id is just an even-length hex string as far as
        // `CommitId::try_from_hex` is concerned — it happily decodes a
        // twelve-character prefix into a six-byte id that no object
        // will ever match. So the exact lookup has to be *tried*, not
        // assumed, with prefix resolution as the fallback.
        if let Some(id) = CommitId::try_from_hex(commit_ref) {
            if let Ok(commit) = pollster::block_on(backend.read_commit(&id)) {
                return Ok((id, commit.change_id));
            }
        }
        let prefix = HexPrefix::try_from_hex(commit_ref)
            .ok_or_else(|| Error::BadRequest(format!("{commit_ref:?}: not a hex commit id")))?;
        let commit_id = match repo.index().resolve_commit_id_prefix(&prefix) {
            Ok(PrefixResolution::SingleMatch(id)) => id,
            Ok(PrefixResolution::AmbiguousMatch) => {
                return Err(Error::BadRequest(format!(
                    "{commit_ref:?}: ambiguous commit prefix in root {}",
                    root.id
                )));
            }
            Ok(PrefixResolution::NoMatch) => {
                return Err(Error::NotFound(format!(
                    "commit {commit_ref} in root {}",
                    root.id
                )));
            }
            Err(e) => return Err(Error::Repo(format!("resolving {commit_ref:?}: {e}"))),
        };
        let commit = pollster::block_on(backend.read_commit(&commit_id))
            .map_err(|_| Error::NotFound(format!("commit {commit_ref} in root {}", root.id)))?;
        Ok((commit_id, commit.change_id))
    }

    fn name_version_inner(
        &self,
        root_id: Uuid,
        commit_id: String,
        name: String,
    ) -> Result<NamedVersion, Error> {
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err(Error::BadRequest("a Named Version needs a name".into()));
        }
        let root = self.get_root_info(root_id)?;
        // Same lock a checkpoint and a GC pass take: it serializes the
        // read-then-write over the vault snapshot (so two namings can't
        // both claim one page path) *and* keeps a naming from landing
        // inside a sweep that has already snapshotted its protect set.
        let lock = self.root_lock(root_id);
        let _guard = lock.lock().expect("root lock poisoned");
        let (commit_id, change_id) = self.resolve_commit(&root, &commit_id)?;
        self.versions.create_named_version(
            root_id,
            &root.name,
            name,
            change_id.hex(),
            commit_id.hex(),
        )
    }

    fn unname_version_inner(&self, id: Uuid) -> Result<NamedVersion, Error> {
        let named = self.versions.named_version(id)?;
        let lock = self.root_lock(named.root_id);
        let _guard = lock.lock().expect("root lock poisoned");
        self.versions.delete_named_version(id)?;
        Ok(named)
    }

    /// The file's review if one exists, following renames: a review
    /// keyed under any previous path in the file's chain is this
    /// file's review (renames must not fork the conversation).
    ///
    /// The reach-back is only as good as the store's copy records:
    /// `Change::Rename` writers (the sync engine, a WebDAV MOVE) record
    /// them, but the v1 scan checkpoint captures a plain filesystem
    /// rename as remove+add (ADR 0001: "detection may start simple"),
    /// which this lookup cannot see through. FUTURE: content-id rename
    /// detection in the scan checkpoint closes that gap for reviews and
    /// chains alike.
    fn find_review_inner(
        &self,
        root_id: Uuid,
        file_path: &str,
    ) -> Result<Option<files_proto::Review>, Error> {
        if let Some(review) = self.versions.review_by_file(root_id, file_path)? {
            return Ok(Some(review));
        }
        // The chain already follows copy records back through renames —
        // an untracked path simply has no chain (and so no review).
        let chain = self
            .chain_inner(root_id, file_path.to_string())
            .unwrap_or_default();
        for entry in &chain {
            if entry.path != file_path
                && let Some(review) = self.versions.review_by_file(root_id, &entry.path)?
            {
                return Ok(Some(review));
            }
        }
        Ok(None)
    }

    /// Get-or-create the review for `(root, file_path)` (issue #270).
    /// Returns `(review, created)` so the RPC wrapper knows whether to
    /// publish `ReviewCreated`.
    fn review_for_file_inner(
        &self,
        root_id: Uuid,
        file_path: String,
    ) -> Result<(files_proto::Review, bool), Error> {
        let root = self.get_root_info(root_id)?;
        Self::require_media(&root, "review")?;
        // Same lock as the curation writes: two first-asks for one file
        // must not both scan an empty vault and write two pages.
        let lock = self.root_lock(root_id);
        let _guard = lock.lock().expect("root lock poisoned");
        if let Some(mut existing) = self.find_review_inner(root_id, &file_path)? {
            // Found under a previous path — re-key to the current one
            // so the exact lookup hits next time and the page reads
            // true.
            if existing.file_path != file_path {
                existing.file_path = file_path;
                existing = self.versions.update_review(existing)?;
            }
            return Ok((existing, false));
        }
        // First ask: the file must actually be a versioned member of
        // this root — an untracked path has no versions to review.
        let (_disk, repo_path) = self.resolve_root_file(&root, &file_path)?;
        let (repo, head) = self.reload_repo(&root)?;
        if Self::head_file(&repo, &head, &repo_path)?.is_none() {
            return Err(Error::NotFound(format!(
                "{file_path}: not tracked by the checkpoint head"
            )));
        }
        let review = self
            .versions
            .create_review(root_id, &root.name, file_path)?;
        Ok((review, true))
    }

    /// The root lock guarding a comment's vault writes, via its review
    /// (a comment page doesn't carry the root id itself).
    fn root_lock_for_review(
        &self,
        comment: &files_proto::ReviewComment,
    ) -> Result<Arc<std::sync::Mutex<()>>, Error> {
        let review = self.versions.review(comment.review_id)?;
        Ok(self.root_lock(review.root_id))
    }

    /// Add a comment stamped with a share-link attribution (issue #272
    /// AC 1) — the guest lane's entry point. Publishes like the org-lane
    /// RPC.
    pub async fn add_review_comment_via(
        &self,
        review_id: Uuid,
        comment: files_proto::NewReviewComment,
        via_link: String,
    ) -> Result<files_proto::ReviewComment, FilesError> {
        let this = self.clone();
        let added =
            blocking(move || this.add_review_comment_inner(review_id, comment, via_link)).await?;
        self.publish(FilesEvent::ReviewCommentAdded(added.clone()));
        Ok(added)
    }

    fn add_review_comment_inner(
        &self,
        review_id: Uuid,
        comment: files_proto::NewReviewComment,
        via_link: String,
    ) -> Result<files_proto::ReviewComment, Error> {
        if comment.body.trim().is_empty() && comment.annotation.is_empty() {
            return Err(Error::BadRequest(
                "a comment needs text or a drawing".into(),
            ));
        }
        if !comment.timecode_secs.is_finite() || comment.timecode_secs < 0.0 {
            return Err(Error::BadRequest(format!(
                "bad timecode: {}",
                comment.timecode_secs
            )));
        }
        let review = self.versions.review(review_id)?;
        let root = self.get_root_info(review.root_id)?;
        let lock = self.root_lock(review.root_id);
        let _guard = lock.lock().expect("root lock poisoned");
        // The recorded version must exist in this root's store — AC 2
        // hinges on the reference staying resolvable. Normalized to the
        // store's full hex spelling so two comments on one version
        // always compare equal.
        let (commit_id, _change) = self.resolve_commit(&root, &comment.commit_id)?;
        let normalized = files_proto::NewReviewComment {
            commit_id: commit_id.hex(),
            ..comment
        };
        self.versions
            .create_review_comment(&review, normalized, via_link)
    }

    /// Resolve a Named Version the way a share link must: prefer the
    /// stable `change_id` (so a rewritten change still lands on its
    /// current commit) and fall back to the recorded `commit_id`.
    /// Either way the answer is one exact change in this root's store,
    /// or [`Error::NotFound`].
    fn resolve_named_version_inner(&self, id: Uuid) -> Result<VersionRef, Error> {
        let named = self.versions.named_version(id)?;
        let root = self.get_root_info(named.root_id)?;
        let (repo, _head) = self.ensure_repo(&root)?;

        let by_change = ChangeId::try_from_hex(&named.change_id).and_then(|change_id| {
            repo.resolve_change_id(&change_id)
                .ok()
                .flatten()
                .and_then(|targets| {
                    targets
                        .visible_with_offsets()
                        .next()
                        .map(|(_, id)| id.clone())
                })
        });
        let (commit_id, change_id) = match by_change {
            Some(commit_id) if !named.change_id.is_empty() => (commit_id, named.change_id.clone()),
            // Either the change isn't in the current index (a Named
            // Version pointing at a commit no view head descends from
            // is a normal, supported shape — that's exactly what the GC
            // protect set exists for), or the page recorded no change
            // id to begin with. Both fall back to the exact commit the
            // entity recorded, validated against the store — one
            // lookup, which yields both halves of the answer.
            _ => {
                let (commit_id, change_id) = self.resolve_commit(&root, &named.commit_id)?;
                (commit_id, change_id.hex())
            }
        };
        Ok(VersionRef {
            root_id: named.root_id,
            change_id,
            commit_id: commit_id.hex(),
        })
    }

    fn start_project_version_inner(
        &self,
        root_id: Uuid,
        label: Option<String>,
    ) -> Result<ProjectVersion, Error> {
        let root = self.get_root_info(root_id)?;
        // See `name_version_inner` for why curation writes take the
        // root lock.
        let lock = self.root_lock(root_id);
        let _guard = lock.lock().expect("root lock poisoned");
        let (_repo, head) = self.ensure_repo(&root)?;
        let (commit_id, change_id) = self.resolve_commit(&root, &head.hex())?;
        let label = label
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty());
        self.versions.create_project_version(
            root_id,
            &root.name,
            label,
            change_id.hex(),
            commit_id.hex(),
        )
    }

    /// Every commit in `root_id`'s store the Vault currently
    /// references — the protect set ADR 0001 calls "Vault-referenced",
    /// resolved live from the vault pages on every pass so a name
    /// deleted (or replicated in) since the last one is honored.
    ///
    /// Three failure modes matter here and they don't all pull the same
    /// way, so each gets its own answer:
    ///
    /// - A page in **this root's own folder** that this process cannot
    ///   read, or whose `commitId` isn't hex at all, is a reference we
    ///   might be about to forfeit. It fails this root's pass
    ///   (`protect_refs` does the strict half) — GC is destructive and
    ///   unnamed content is cheap to keep one more day. Other roots
    ///   sweep normally; a page that is not identifiably this root's is
    ///   never allowed to wedge it.
    /// - A page naming a commit the store **doesn't have** protects
    ///   nothing: that content is already gone, and treating it as
    ///   fatal would wedge GC for the root forever (one stale page from
    ///   a replication reorder, and the store never gets swept again).
    ///   Logged and skipped.
    /// - A page with an **empty** `commitId` — which
    ///   `ProjectVersions::from_page` tolerates, so it exists — names
    ///   nothing at all. Same reasoning: logged and skipped, never
    ///   fatal. (`create_project_version` refuses to write one, so this
    ///   only ever arrives by hand or by replication.)
    ///
    /// Note what goes into `out`: the id `resolve_commit` **resolved**,
    /// never the one parsed off the page. A page may legitimately carry
    /// a twelve-character prefix — that is what every human-facing
    /// surface prints — and `CommitId::try_from_hex` would decode it
    /// into a six-byte id that the mark phase then chokes on. It also
    /// makes the dedup work across a page storing a prefix and another
    /// storing the full id of the same commit.
    fn protected_commits(&self, root: &FileRootInfo) -> Result<Vec<CommitId>, Error> {
        let (repo, _head) = self.ensure_repo(root)?;
        let full_hex_len = repo.store().root_commit_id().as_bytes().len() * 2;
        let mut out: Vec<CommitId> = Vec::new();
        for reference in self.versions.protect_refs(root.id, &root.name)? {
            if reference.commit_id.trim().is_empty() {
                tracing::warn!(
                    page = %reference.page,
                    "a Files version page carries no commit id; nothing to protect"
                );
                continue;
            }
            match self.resolve_commit(root, &reference.commit_id) {
                Ok((resolved, _change_id)) => {
                    if !out.contains(&resolved) {
                        out.push(resolved);
                    }
                }
                // "Not here" only means "already gone" for a full id.
                // An *abbreviation* that resolves to nothing means we
                // failed to interpret it — prefix lookup goes through
                // the index, and a Named Version's whole purpose is to
                // point at commits the index no longer reaches — so
                // treating it as stale would forfeit exactly the
                // content this set exists to keep. Fatal instead, with
                // the page named so a human can write the full id.
                Err(Error::NotFound(_)) if reference.commit_id.len() == full_hex_len => {
                    tracing::warn!(
                        page = %reference.page,
                        commit = %reference.commit_id,
                        "a Files version page references a commit this root's store doesn't have; \
                         nothing to protect"
                    );
                }
                // Not hex, or an ambiguous prefix: we cannot tell what
                // this page protects, so we refuse to sweep past it.
                Err(e) => {
                    return Err(Error::BadRequest(format!(
                        "{}: {:?} does not name a commit ({e}) — refusing to compute a GC protect \
                         set that might silently forfeit the version it references",
                        reference.page, reference.commit_id
                    )));
                }
            }
        }
        Ok(out)
    }

    fn gc_root_inner(
        &self,
        root_id: Uuid,
        keep_newer_secs: Option<u64>,
    ) -> Result<GcReport, Error> {
        let root = self.get_root_info(root_id)?;
        // A software root's objects are git's, and git collects its own
        // garbage (`git gc`, and every host runs it server-side).
        // Sweeping a colocated repository from here would mean deciding
        // reachability for a store whose other author is git itself —
        // exactly the thing issue #273's design refuses to do. Say so
        // plainly rather than failing later with a backend-type
        // mismatch, and leave the protect-set doctrine where it
        // belongs: on the store Files actually owns.
        if root.flavor == RootFlavor::Software {
            return Err(Error::BadRequest(format!(
                "root {root_id} is a software root: its objects live in a colocated git \
                 repository, which collects its own garbage (`git gc`). Files' Vault-protected \
                 sweep applies to media roots only."
            )));
        }
        // Hold the root lock for the whole pass. It blocks that root's
        // checkpoints (and curation writes) for the duration, which is
        // the deliberate trade: a sweep that raced a checkpoint could
        // read a head the checkpoint is still building on top of, or
        // miss a name that landed after the protect set was read, and
        // both of those lose data. GC is an occasional maintenance
        // verb; a checkpoint waiting on it is a delay, not a loss.
        let lock = self.root_lock(root_id);
        let _guard = lock.lock().expect("root lock poisoned");

        // Re-read the op log before deciding what is reachable: a
        // second process may have written checkpoints this handle has
        // never seen, and sweeping from a stale index would delete
        // them. See `reload_repo`.
        let (repo, _head) = self.reload_repo(&root)?;
        let protected = self.protected_commits(&root)?;
        let backend = repo
            .store()
            .backend_impl::<VersionStoreBackend>()
            .ok_or_else(|| Error::Repo("root's repo is not a VersionStoreBackend".into()))?;

        let keep_newer = std::time::SystemTime::now()
            .checked_sub(std::time::Duration::from_secs(
                keep_newer_secs.unwrap_or(DEFAULT_GC_KEEP_NEWER_SECS),
            ))
            .unwrap_or(std::time::UNIX_EPOCH);

        let stats = pollster::block_on(task_files_version_store::gc::sweep(
            backend,
            repo.readonly_index().as_index(),
            keep_newer,
            &protected,
        ))?;
        Ok(GcReport {
            objects_swept: stats.objects_swept as u64,
            manifests_swept: stats.chunks.manifests_swept as u64,
            protected_commits: protected.len() as u32,
        })
    }

    /// The one write path behind every capture (issue #260): an
    /// explicit `checkpoint_now`, a quiescence checkpoint, and a
    /// mid-session auto-snapshot all come through here. A checkpoint
    /// parents on the checkpoint head; a snapshot parents on the
    /// snapshot branch's tip (or, for a session's first, on the
    /// checkpoint head — which is what starts the branch). See
    /// [`crate::cadence`] on why snapshots branch rather than extend.
    ///
    /// Auto-snapshots are a **media-flavor** concept: a software root's
    /// history is git's, and hanging ephemeral commits off a branch a
    /// developer shares would be a surprise in `git log`. The cadence
    /// engine still checkpoints software roots at quiescence — that is
    /// an ordinary commit on the checked-out branch.
    fn capture_inner(
        &self,
        root_id: Uuid,
        kind: CaptureKind,
        description: String,
        save_points: Vec<SavePoint>,
    ) -> Result<Captured, Error> {
        let root = self.get_root_info(root_id)?;
        // Serialize captures on this root: held across the whole
        // read-diff-commit-publish sequence so two concurrent callers
        // can't both read the same head and each commit on top of it
        // (PR #280 review) — the second one now genuinely observes the
        // first's result as its parent instead of racing it.
        let lock = self.root_lock(root_id);
        let _guard = lock.lock().expect("checkpoint lock poisoned");

        // Same staleness as `gc_root_inner`, with a different symptom:
        // building on a cached head that another writer has already
        // moved past forks the chain instead of extending it.
        // `reload_repo` re-reads whichever authority this flavor has —
        // git's refs for a software root, the op log for a media one.
        let (repo, head) = self.reload_repo(&root)?;
        // Both flavors write through jj's `Backend` trait, not either
        // concrete backend (issue #273).
        let backend = repo.store().backend();

        let kind = match (kind, root.flavor) {
            // A snapshot on a software root would be a stray commit in
            // someone's git history: checkpoint instead (see the doc).
            (CaptureKind::Snapshot, RootFlavor::Software) => CaptureKind::Checkpoint,
            (kind, _) => kind,
        };
        let snapshot_head = self.snapshot_head_of(root_id);
        let parent_id = match kind {
            CaptureKind::Checkpoint => head.clone(),
            CaptureKind::Snapshot => snapshot_head.unwrap_or_else(|| head.clone()),
        };

        let parent_commit = pollster::block_on(backend.read_commit(&parent_id))?;
        let base_tree_id = parent_commit
            .root_tree
            .clone()
            .into_resolved()
            .map_err(|_| {
                Error::Repo("capturing onto a conflicted tree is unsupported (v1)".into())
            })?;
        let base_tree = pollster::block_on(backend.read_tree(RepoPath::root(), &base_tree_id))?;
        let mut base_paths: BTreeSet<RepoPathBuf> = BTreeSet::new();
        pollster::block_on(scan::walk_tree_paths(
            backend,
            &base_tree,
            RepoPath::root(),
            &mut base_paths,
        ))?;

        // The certifying full scan, with the root's Ignore set applied
        // at enumeration: an ignored *untracked* path is never offered
        // to the store, while an ignored path that is already tracked
        // keeps being versioned (see `crate::ignore`).
        let ignores = self.ignore_of(&root)?;
        let disk_files =
            scan::walk_live_tree(Path::new(&root.path), root.flavor, &ignores, &base_paths)?;
        let hook = self.hook.lock().expect("hook lock poisoned").clone();
        let result = crate::checkpoint::write_checkpoint(Capture {
            repo: &repo,
            backend,
            parent_id,
            base_tree_id,
            base_tree: &base_tree,
            disk_files: &disk_files,
            base_paths: &base_paths,
            description: description.clone(),
            attempts: self.cadence.config().certify_attempts,
            hook,
        })?;

        let at = self.cadence.now();
        let commit_hex = result.commit_id.hex();
        let store_dir = repo_open::store_dir(Path::new(&root.path));
        let mut journal = Journal::load(&store_dir)?;

        let captured = match kind {
            CaptureKind::Snapshot => {
                let checkpoint_head = head.hex();
                self.set_heads(root_id, result.repo, head, Some(result.commit_id));
                journal.record_snapshot(
                    SnapshotRecord {
                        snapshot_id: commit_hex.clone(),
                        at,
                        changed_paths: result.changed_paths.clone(),
                        save_points: save_points.clone(),
                    },
                    &checkpoint_head,
                    at,
                );
                Captured::Snapshot(SnapshotInfo {
                    root_id,
                    snapshot_id: commit_hex,
                    at,
                    changed_paths: result.changed_paths,
                    save_points,
                })
            }
            CaptureKind::Checkpoint => {
                // Software roots are colocated git: move the checked-out
                // branch and rewrite the index so the commit we just
                // wrote is what `git log` / `git status` / `git push`
                // see (issue #273).
                let repo = match root.flavor {
                    RootFlavor::Software => {
                        git_root::publish_checkpoint(result.repo, &result.commit_id)?
                    }
                    RootFlavor::Media => result.repo,
                };
                self.set_heads(root_id, repo, result.commit_id.clone(), None);
                journal.record_checkpoint(CheckpointRecord {
                    commit_id: commit_hex.clone(),
                    at,
                    save_points: save_points.clone(),
                    requeued_paths: result.requeued_paths.clone(),
                });
                Captured::Checkpoint(CheckpointInfo {
                    root_id,
                    commit_id: commit_hex,
                    description,
                    at,
                    changed_paths: result.changed_paths,
                    save_points,
                    requeued_paths: result.requeued_paths,
                })
            }
        };
        journal.save(&store_dir)?;

        self.publish(match &captured {
            Captured::Snapshot(info) => FilesEvent::Snapshotted(info.clone()),
            Captured::Checkpoint(info) => FilesEvent::Checkpointed(info.clone()),
        });

        // Checkpoint trigger for derived media (issue #269): warm up the
        // new head's rendition ladder ahead of demand. Detached and
        // best-effort — a checkpoint never waits on (or fails for)
        // transcoding, which is slow and off the critical path. Only
        // when a transcoder is configured; the spawn rides the
        // `spawn_blocking` thread's ambient runtime handle.
        if let Captured::Checkpoint(info) = &captured
            && self.transcoder_opt().is_some()
            && let Some(head) = CommitId::try_from_hex(&info.commit_id)
        {
            let this = self.clone();
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(async move { this.warm_up_head(root_id, head).await });
            }
        }
        Ok(captured)
    }

    fn checkpoint_now_inner(
        &self,
        root_id: Uuid,
        description: Option<String>,
    ) -> Result<CheckpointInfo, Error> {
        // An explicit checkpoint certifies the same live tree a
        // quiescence checkpoint would, so it ends the session: the save
        // points it collected ride onto this checkpoint, and the root
        // goes quiet until someone writes again.
        //
        // The session comes out of the engine *before* the capture that
        // needs its save points, so a failed capture has to put it back
        // — the out-of-band twin of `tick`'s `cadence.failed`. Without
        // this, a transient I/O error would silently cost the root both
        // its save points and its pending quiescence checkpoint (PR
        // #283 review).
        let ended = self.cadence.end_session(root_id);
        let save_points = ended.save_points();
        let description = description.unwrap_or_else(|| "checkpoint now".to_string());
        let captured =
            match self.capture_inner(root_id, CaptureKind::Checkpoint, description, save_points) {
                Ok(captured) => captured,
                Err(err) => {
                    self.cadence.restore_session(ended);
                    return Err(err);
                }
            };
        match captured {
            Captured::Checkpoint(info) => Ok(info),
            Captured::Snapshot(_) => unreachable!("a checkpoint capture returns a checkpoint"),
        }
    }

    /// The root's whole Ignore set, compiled on first touch and cached.
    fn ignore_of(
        &self,
        root: &FileRootInfo,
    ) -> Result<Arc<jj_lib::gitignore::GitIgnoreFile>, Error> {
        Hints::ignore_of(&self.ignores, root)
    }

    /// The registry + Ignore-set + cadence slice of this backend, as an
    /// [`ActivitySink`] a watcher can hold.
    fn hints(&self) -> Arc<Hints> {
        Arc::new(Hints {
            registry: self.registry.clone(),
            ignores: self.ignores.clone(),
            cadence: self.cadence.clone(),
        })
    }

    fn snapshots_inner(&self, root_id: Uuid) -> Result<Vec<SnapshotInfo>, Error> {
        let root = self.get_root_info(root_id)?;
        Ok(Self::journal_of(&root)?.snapshot_infos(root_id))
    }

    fn hint_activity_inner(&self, root_id: Uuid, paths: Vec<String>) -> Result<u32, Error> {
        self.hints().note(root_id, &paths)
    }

    fn ignore_set_inner(&self, root_id: Uuid) -> Result<Vec<String>, Error> {
        let root = self.get_root_info(root_id)?;
        ignore::stored_patterns(&repo_open::store_dir(Path::new(&root.path)))
    }

    fn set_ignore_set_inner(
        &self,
        root_id: Uuid,
        patterns: Vec<String>,
    ) -> Result<Vec<String>, Error> {
        let root = self.get_root_info(root_id)?;
        let stored = ignore::save_patterns(&repo_open::store_dir(Path::new(&root.path)), patterns)?;
        // Drop the compiled cache so the next capture (and the next
        // hint) matches against the edit.
        self.ignores
            .lock()
            .expect("ignore cache lock poisoned")
            .remove(&root_id);
        Ok(stored)
    }

    /// Resolve + confine one root-relative file path for the hydration
    /// ops. Same double guard as `browse_inner`: refuse absolute
    /// subpaths before `join` (std `join` replaces the base), then
    /// canonicalize-and-prefix-check the platform way. The jj repo
    /// path is parsed too, which rejects `.`/`..` components.
    fn resolve_root_file(
        &self,
        root: &FileRootInfo,
        path: &str,
    ) -> Result<(PathBuf, RepoPathBuf), Error> {
        if Path::new(path).is_absolute() {
            return Err(Error::BadRequest(format!("path escapes the root: {path}")));
        }
        let repo_path =
            RepoPathBuf::from_internal_string(&path.replace(std::path::MAIN_SEPARATOR, "/"))
                .map_err(|e| Error::BadRequest(format!("{path:?}: {e}")))?;
        let root_path = PathBuf::from(&root.path);
        let disk_path = root_path.join(repo_path.as_internal_file_string());
        if let Ok(canonical) = disk_path.canonicalize() {
            task_files_util::confine(&canonical, &root_path).map_err(confinement)?;
        }
        Ok((disk_path, repo_path))
    }

    /// The checkpoint head's `TreeValue::File` fields for `repo_path`,
    /// or `None` when the head doesn't track it.
    fn head_file(
        repo: &Arc<ReadonlyRepo>,
        head: &CommitId,
        repo_path: &RepoPath,
    ) -> Result<Option<(jj_lib::backend::FileId, bool)>, Error> {
        let backend = repo.store().backend();
        let value = pollster::block_on(async {
            let commit = backend.read_commit(head).await?;
            let tree_id =
                commit.root_tree.clone().into_resolved().map_err(|_| {
                    jj_lib::backend::BackendError::Other("conflicted root tree".into())
                })?;
            let tree = backend.read_tree(RepoPath::root(), &tree_id).await?;
            task_files_version_store::chain::lookup_dyn(backend, &tree, repo_path).await
        })
        .map_err(|e| Error::Repo(format!("reading head tree: {e}")))?;
        Ok(match value {
            Some(jj_lib::backend::TreeValue::File { id, executable, .. }) => Some((id, executable)),
            _ => None,
        })
    }

    /// One file's `BrowseEntry` as the hydration ops report it.
    fn entry_for(disk_path: &Path, name: &str) -> Result<BrowseEntry, Error> {
        let len = std::fs::metadata(disk_path)?.len();
        let stub = if stub::candidate_len(len) {
            stub::read(disk_path)?
        } else {
            None
        };
        Ok(BrowseEntry {
            name: name.to_string(),
            is_dir: false,
            size: Some(stub.as_ref().map_or(len, |s| s.size)),
            stub: stub.is_some(),
            divergent: false,
        })
    }

    /// Media-only guard shared by the hydration ops — a software root's
    /// working tree belongs to its colocated git (same split as
    /// `gc_root`): a stub there would just be a modified file to git,
    /// and every git tool would happily commit it as content.
    fn require_media(root: &FileRootInfo, what: &str) -> Result<(), Error> {
        if root.flavor != RootFlavor::Media {
            return Err(Error::BadRequest(format!(
                "{what} is media-only: a software root's working tree belongs to its colocated git"
            )));
        }
        Ok(())
    }

    fn dehydrate_inner(&self, root_id: Uuid, path: String) -> Result<BrowseEntry, Error> {
        match self.try_dehydrate_inner(root_id, &path)? {
            DehydrateOutcome::Done(entry) => Ok(entry),
            DehydrateOutcome::Dirty => Err(Error::BadRequest(format!(
                "{path}: on-disk content differs from the checkpoint head — checkpoint first, then dehydrate"
            ))),
        }
    }

    /// [`FilesBackend::dehydrate_inner`] with the dirty case as a typed
    /// outcome instead of an error, so the policy apply pass classifies
    /// it structurally rather than by matching error-message substrings
    /// (PR #289 review).
    fn try_dehydrate_inner(&self, root_id: Uuid, path: &str) -> Result<DehydrateOutcome, Error> {
        let root = self.get_root_info(root_id)?;
        Self::require_media(&root, "dehydrate")?;
        let (disk_path, repo_path) = self.resolve_root_file(&root, path)?;
        let lock = self.root_lock(root_id);
        let _guard = lock.lock().expect("root lock poisoned");

        if !disk_path.exists() {
            return Err(Error::NotFound(format!("{root_id}:{path}")));
        }
        // Idempotent: already a stub — report it, touch nothing.
        let len = std::fs::metadata(&disk_path)?.len();
        if stub::candidate_len(len) && stub::read(&disk_path)?.is_some() {
            return Ok(DehydrateOutcome::Done(Self::entry_for(&disk_path, path)?));
        }

        // Reloaded head, not the cache: dehydration compares against
        // what is genuinely committed, wherever it was written.
        let (repo, head) = self.reload_repo(&root)?;
        let Some((head_id, executable)) = Self::head_file(&repo, &head, &repo_path)? else {
            return Err(Error::BadRequest(format!(
                "{path}: not tracked by the checkpoint head — checkpoint before dehydrating"
            )));
        };

        // The one rule that makes dehydration safe: on-disk content
        // must BE the committed content. `probe` derives the id without
        // writing anything, so a refused dehydrate persists nothing —
        // the dirty bytes never enter the store as orphaned chunks (PR
        // #289 review).
        //
        // The root lock serializes THIS backend's writers, but a DAW or
        // the WebDAV bridge writes straight to disk under nobody's lock
        // — so the hash rides the same stat sandwich every checkpoint
        // read does (`crate::certify`): a file that moved while being
        // hashed, or whose timestamps are too coarse to prove anything
        // without a second matching read, is refused rather than
        // stubbed over (PR #289 review — the TOCTOU where a mid-hash
        // save was destroyed by `stub::write`).
        let backend = repo.store().backend();
        let content = crate::content::ContentStore::for_repo(&repo, backend)?;
        let guard = crate::certify::StatGuard::begin(&disk_path)?;
        let Some(disk_id) = content.probe(&disk_path)? else {
            return Err(Error::Repo(
                "this root's backend cannot derive content ids without writing".into(),
            ));
        };
        if disk_id != head_id {
            return Ok(DehydrateOutcome::Dirty);
        }
        match guard.check(&disk_path)? {
            crate::certify::Settled::Stable => {}
            crate::certify::Settled::Moved => {
                return Err(Error::BadRequest(format!(
                    "{path}: the file is being written right now — try again when the writer settles"
                )));
            }
            crate::certify::Settled::Coarse => {
                // Prove stability by content, like the checkpoint path
                // does on coarse-mtime filesystems: two independent
                // reads deriving the same id had no write between them.
                let again = content.probe(&disk_path)?;
                if again != Some(disk_id)
                    || guard.check(&disk_path)? == crate::certify::Settled::Moved
                {
                    return Err(Error::BadRequest(format!(
                        "{path}: the file is being written right now — try again when the writer settles"
                    )));
                }
            }
        }

        stub::write(&disk_path, &stub::Stub::new(&head_id, len, executable))?;
        self.publish(FilesEvent::HydrationChanged(HydrationChange {
            root_id,
            path: repo_path.as_internal_file_string().to_string(),
            stub: true,
        }));
        Ok(DehydrateOutcome::Done(Self::entry_for(&disk_path, path)?))
    }

    fn hydrate_inner(&self, root_id: Uuid, path: String) -> Result<BrowseEntry, Error> {
        let root = self.get_root_info(root_id)?;
        Self::require_media(&root, "hydrate")?;
        let (disk_path, repo_path) = self.resolve_root_file(&root, &path)?;
        let lock = self.root_lock(root_id);
        let _guard = lock.lock().expect("root lock poisoned");

        if !disk_path.exists() {
            return Err(Error::NotFound(format!("{root_id}:{path}")));
        }
        let len = std::fs::metadata(&disk_path)?.len();
        let on_disk = if stub::candidate_len(len) {
            stub::read(&disk_path)?
        } else {
            None
        };
        // Idempotent: already resident — report it, touch nothing.
        let Some(recorded) = on_disk else {
            return Self::entry_for(&disk_path, &path);
        };

        // The id to restore: the checkpoint head's when it tracks the
        // path (the head may have moved since dehydration — "the live
        // tree shows the newest save" wins over a stale stub), the
        // stub's own recorded id otherwise.
        let (repo, head) = self.reload_repo(&root)?;
        let (target_id, executable) = match Self::head_file(&repo, &head, &repo_path)? {
            Some((id, exec)) => (id, exec),
            None => (recorded.file_id()?, recorded.executable),
        };

        self.restore_content(&repo, &repo_path, &disk_path, &target_id, executable)?;
        self.publish(FilesEvent::HydrationChanged(HydrationChange {
            root_id,
            path: repo_path.as_internal_file_string().to_string(),
            stub: false,
        }));
        Self::entry_for(&disk_path, &path)
    }

    /// Stream `target_id`'s content from the store to a temp file in
    /// the same directory, verify the bytes re-derive to exactly
    /// `target_id` (the acceptance criterion's "verified by FileId" —
    /// a truncated or corrupt restore never replaces the stub), set the
    /// executable bit, and rename into place.
    fn restore_content(
        &self,
        repo: &Arc<ReadonlyRepo>,
        repo_path: &RepoPath,
        disk_path: &Path,
        target_id: &jj_lib::backend::FileId,
        executable: bool,
    ) -> Result<(), Error> {
        use futures_util::io::AsyncReadExt as _;
        use std::io::Write as _;

        let backend = repo.store().backend();
        let dir = disk_path
            .parent()
            .ok_or_else(|| Error::BadRequest(format!("{}: no parent", disk_path.display())))?;
        let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
        pollster::block_on(async {
            let mut reader = backend.read_file(repo_path, target_id).await?;
            let mut buf = vec![0u8; 128 * 1024];
            loop {
                let n = reader.read(&mut buf).await.map_err(|e| {
                    jj_lib::backend::BackendError::Other(
                        format!("reading {} from the store: {e}", target_id.hex()).into(),
                    )
                })?;
                if n == 0 {
                    break;
                }
                tmp.write_all(&buf[..n]).map_err(|e| {
                    jj_lib::backend::BackendError::Other(format!("writing restore: {e}").into())
                })?;
            }
            Ok::<(), jj_lib::backend::BackendError>(())
        })
        .map_err(Error::from)?;
        tmp.as_file().sync_all()?;

        // Verify by identity before the rename: re-derive the restored
        // bytes' id through the same content store and require it to be
        // the id we asked for.
        let content = crate::content::ContentStore::for_repo(repo, backend)?;
        let restored_id = content.probe(tmp.path())?.ok_or_else(|| {
            Error::Repo("this root's backend cannot derive content ids without writing".into())
        })?;
        if restored_id != *target_id {
            return Err(Error::Repo(format!(
                "{}: restored content re-derives to {} but the stub promised {} — store damage, stub left in place",
                disk_path.display(),
                restored_id.hex(),
                target_id.hex(),
            )));
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = if executable { 0o755 } else { 0o644 };
            std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(mode))?;
        }
        #[cfg(not(unix))]
        let _ = executable;
        tmp.persist(disk_path).map_err(|e| Error::Io(e.error))?;
        Ok(())
    }

    fn hydration_policy_inner(&self, root_id: Uuid) -> Result<Vec<String>, Error> {
        let root = self.get_root_info(root_id)?;
        hydration::stored_policy(&repo_open::store_dir(Path::new(&root.path)))
    }

    fn set_hydration_policy_inner(
        &self,
        root_id: Uuid,
        patterns: Vec<String>,
    ) -> Result<Vec<String>, Error> {
        let root = self.get_root_info(root_id)?;
        Self::require_media(&root, "hydration policy")?;
        hydration::save_policy(&repo_open::store_dir(Path::new(&root.path)), patterns)
    }

    fn apply_hydration_policy_inner(&self, root_id: Uuid) -> Result<HydrationReport, Error> {
        let root = self.get_root_info(root_id)?;
        Self::require_media(&root, "hydration policy")?;
        let store_dir = repo_open::store_dir(Path::new(&root.path));
        let Some(policy) = hydration::matcher(&store_dir)? else {
            // Empty policy: opt-in means touch nothing.
            return Ok(HydrationReport::default());
        };

        // One live-tree walk decides the whole pass; the per-file ops
        // then re-take the root lock each, so a checkpoint landing
        // mid-pass serializes between files rather than deadlocking
        // against a pass-wide lock.
        let ignores = self.ignore_of(&root)?;
        let (_, head) = self.reload_repo(&root)?;
        let tracked = self.tracked_paths(&root, &head)?;
        let files = scan::walk_live_tree(Path::new(&root.path), root.flavor, &ignores, &tracked)?;

        let mut report = HydrationReport::default();
        // Per-file fault tolerance: one unhydratable stub (a partial
        // replica missing chunks — hydrate's own docs call that
        // normal) or one racing writer must not abort the pass and
        // discard the report of mutations already performed (PR #289
        // review). Every per-file failure lands in `failed` with its
        // path; the pass itself only errors on setup.
        for file in files {
            let rel = file.repo_path.as_internal_file_string().to_string();
            let keep = hydration::keeps_hydrated(&policy, &rel);
            if file.stub.is_some() {
                if keep {
                    match self.hydrate_inner(root_id, rel.clone()) {
                        Ok(_) => report.hydrated.push(rel),
                        Err(err) => {
                            tracing::warn!(%root_id, path = %rel, %err, "policy hydrate failed");
                            report.failed.push(rel);
                        }
                    }
                }
            } else if !keep && !file.ignored && tracked.contains(&file.repo_path) {
                match self.try_dehydrate_inner(root_id, &rel) {
                    Ok(DehydrateOutcome::Done(_)) => report.dehydrated.push(rel),
                    Ok(DehydrateOutcome::Dirty) => report.skipped_dirty.push(rel),
                    Err(err) => {
                        tracing::warn!(%root_id, path = %rel, %err, "policy dehydrate failed");
                        report.failed.push(rel);
                    }
                }
            }
        }
        report.hydrated.sort();
        report.dehydrated.sort();
        report.skipped_dirty.sort();
        report.failed.sort();
        Ok(report)
    }

    /// The checkpoint head's full tracked-path set (the scan walker's
    /// second input).
    fn tracked_paths(
        &self,
        root: &FileRootInfo,
        head: &CommitId,
    ) -> Result<std::collections::BTreeSet<RepoPathBuf>, Error> {
        let (repo, _) = self.ensure_repo(root)?;
        let backend = repo.store().backend();
        let mut out = std::collections::BTreeSet::new();
        pollster::block_on(async {
            let commit = backend.read_commit(head).await?;
            let tree_id =
                commit.root_tree.clone().into_resolved().map_err(|_| {
                    jj_lib::backend::BackendError::Other("conflicted root tree".into())
                })?;
            let tree = backend.read_tree(RepoPath::root(), &tree_id).await?;
            scan::walk_tree_paths(backend, &tree, RepoPath::root(), &mut out)
                .await
                .map_err(|e| jj_lib::backend::BackendError::Other(e.to_string().into()))
        })
        .map_err(|e| Error::Repo(format!("walking head tree: {e}")))?;
        Ok(out)
    }

    /// Run one cadence pass: perform every capture that has fallen due
    /// as of the engine's clock. This is what the driver task calls on
    /// a timer in production, and what a test calls after advancing its
    /// [`crate::cadence::TestClock`] — the same code path either way.
    pub async fn tick(&self) -> Vec<Captured> {
        let mut performed = Vec::new();
        for due in self.cadence.take_due() {
            match self.perform_due(&due).await {
                Ok(captured) => {
                    self.cadence.completed(&due);
                    performed.push(captured);
                }
                Err(err) => {
                    // Nothing is consumed on failure: the same capture
                    // falls due again next tick, so a transient I/O
                    // error costs a tick, not a session.
                    tracing::warn!(root_id = %due.root_id, kind = ?due.kind, %err, "files cadence capture failed");
                    self.cadence.failed(&due);
                }
            }
        }
        performed
    }

    async fn perform_due(&self, due: &Due) -> Result<Captured, FilesError> {
        let this = self.clone();
        let due = due.clone();
        let description = match due.kind {
            DueKind::Snapshot => "auto-snapshot".to_string(),
            DueKind::Checkpoint => "session checkpoint".to_string(),
        };
        blocking(move || {
            this.capture_inner(due.root_id, due.kind.into(), description, due.save_points)
        })
        .await
    }

    /// Drive the cadence forever on `interval` — one background task
    /// per backend, the production counterpart of a test calling
    /// [`FilesBackend::tick`] by hand. The interval only bounds how
    /// promptly a due capture happens; the cadence itself is the
    /// engine's, so a coarse interval is cheap.
    ///
    /// The handle is kept on the backend (and aborted by
    /// [`FilesBackend::shutdown`], or by a second call to this) rather
    /// than left to the caller: two drivers ticking one on-disk store
    /// would resurrect exactly the dual-capture race PR #280 closed, and
    /// a driver nobody holds is a driver nobody can stop (PR #283
    /// review).
    pub fn spawn_cadence_driver(&self, interval: std::time::Duration) {
        let this = self.clone();
        let handle = tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                let _ = this.tick().await;
            }
        });
        if let Some(previous) = self
            .driver
            .lock()
            .expect("driver lock poisoned")
            .replace(handle)
        {
            previous.abort();
        }
    }

    /// Start the server-side watcher for `root_id` — activity hints
    /// into the cadence engine (see [`crate::cadence::watcher`]).
    /// Idempotent: watching an already-watched root is a no-op.
    ///
    /// Blocking: establishing a recursive watch walks the whole tree
    /// (inotify is per-directory, so one watch per directory), which on
    /// a multi-GB media root with thousands of directories is real
    /// filesystem work. Callers on an async runtime must reach it
    /// through [`FilesBackend::enable_watching`] or their own
    /// `spawn_blocking` (PR #283 review). The watchers map is locked
    /// only around the lookup and the insert, never across that walk.
    pub fn watch_root(&self, root_id: Uuid) -> Result<(), FilesError> {
        let root = self.get_root_info(root_id).map_err(to_files_error)?;
        if self
            .watchers
            .lock()
            .expect("watcher map poisoned")
            .contains_key(&root_id)
        {
            return Ok(());
        }
        let watcher = RootWatcher::spawn(root_id, Path::new(&root.path), self.hints())
            .map_err(to_files_error)?;
        // Another caller may have won the race while the walk ran; the
        // first watch installed wins and ours is dropped (which stops
        // it), so a root never ends up with two.
        self.watchers
            .lock()
            .expect("watcher map poisoned")
            .entry(root_id)
            .or_insert(watcher);
        Ok(())
    }

    /// Watch every root this backend already knows about, and every
    /// root created from here on — what a server does at startup so
    /// sessions are detected without anyone having to call
    /// `hint_activity`. A root whose watch can't be established (an
    /// offline removable location, a platform limit) is logged and
    /// skipped: it still checkpoints on an explicit trigger, which is
    /// the whole reason watchers are hints.
    ///
    /// Async because [`FilesBackend::watch_root`] is blocking work: the
    /// whole sweep runs on `spawn_blocking` so establishing watches over
    /// a NAS full of media roots cannot stall an async worker during org
    /// startup (PR #283 review).
    pub async fn enable_watching(&self) {
        let this = self.clone();
        let _ = tokio::task::spawn_blocking(move || {
            this.watch_new_roots
                .store(true, std::sync::atomic::Ordering::SeqCst);
            for root in this.registry.list() {
                if let Err(err) = this.watch_root(root.id) {
                    tracing::warn!(root_id = %root.id, path = %root.path, %err, "files: root not watched");
                }
            }
        })
        .await;
    }

    /// Stop watching `root_id`.
    pub fn unwatch_root(&self, root_id: Uuid) {
        self.watchers
            .lock()
            .expect("watcher map poisoned")
            .remove(&root_id);
    }
}

/// The Project Version restart flow (issue #268) and its two read
/// verbs. The spec's sequence, literally: **checkpoint** (the old
/// iteration's terminal state, taken through the ordinary certified
/// capture), **reshape** (the disk half — clear per mode, with every
/// removal re-verified against the checkpoint so a mid-flip save is
/// never destroyed), **flip** (the new lineage's first commit, again
/// through the ordinary scan machinery, so the flip is just data other
/// subscribers — and replicas, #264 — receive as events).
impl FilesBackend {
    fn restart_inner(
        &self,
        root_id: Uuid,
        mode: files_proto::RestartMode,
        label: Option<String>,
    ) -> Result<ProjectVersion, Error> {
        use files_proto::RestartMode;
        let root = self.get_root_info(root_id)?;
        Self::require_media(&root, "restart")?;
        let root_path = PathBuf::from(&root.path);

        // Validate the mode's inputs BEFORE the checkpoint mutates
        // anything — a restart that fails validation must be a no-op.
        let template = match &mode {
            RestartMode::Template { source_path } => {
                let source = self.confine(Path::new(source_path))?;
                crate::restart::validate_template(&source)?;
                // Disjoint from the root, both directions: a template
                // inside the restarting root would be gutted by the
                // clear before it seeds (a half-completed destructive
                // restart), and a template containing the root would
                // copy the tree into itself (PR #290 review).
                if source.starts_with(&root_path) || root_path.starts_with(&source) {
                    return Err(Error::BadRequest(format!(
                        "{source_path}: the template must be outside the root being restarted"
                    )));
                }
                Some(source)
            }
            _ => None,
        };
        let carry: Option<BTreeSet<RepoPathBuf>> = match &mode {
            RestartMode::CarryForward { paths } if !paths.is_empty() => {
                let mut set = BTreeSet::new();
                for p in paths {
                    let (_, repo_path) = self.resolve_root_file(&root, p)?;
                    set.insert(repo_path);
                }
                Some(set)
            }
            // Empty carry list = the picker default: everything minus
            // the Ignore set — which is every tracked path, since the
            // Ignore set governs what enters tracking in the first
            // place. A pure lineage cut.
            RestartMode::CarryForward { .. } => None,
            _ => Some(BTreeSet::new()), // Empty / Template carry nothing
        };

        // 1. The old iteration's terminal checkpoint — ordinary,
        // certified, session-ending.
        self.checkpoint_now_inner(
            root_id,
            Some("final checkpoint of the old iteration".to_string()),
        )?;
        if let Some(hook) = self
            .flip_hook
            .lock()
            .expect("flip hook lock poisoned")
            .clone()
        {
            hook(&root_path);
        }

        // 2. Everything else under the root lock.
        let lock = self.root_lock(root_id);
        let _guard = lock.lock().expect("root lock poisoned");
        let (repo, head) = self.reload_repo(&root)?;
        let backend = repo.store().backend();
        let head_commit = pollster::block_on(backend.read_commit(&head))?;
        let head_tree_id =
            head_commit.root_tree.clone().into_resolved().map_err(|_| {
                Error::Repo("restarting a conflicted tree is unsupported (v1)".into())
            })?;
        let head_tree = pollster::block_on(backend.read_tree(RepoPath::root(), &head_tree_id))?;
        let mut head_paths: BTreeSet<RepoPathBuf> = BTreeSet::new();
        pollster::block_on(scan::walk_tree_paths(
            backend,
            &head_tree,
            RepoPath::root(),
            &mut head_paths,
        ))?;

        // 3. Clear phase. Every candidate removal is re-verified
        // against the checkpoint we just took: a file whose content
        // moved in the window is a mid-flip save — it is never
        // deleted from under the writer; it is committed as a sibling
        // of the old head below, surviving as flagged divergence
        // (spec AC 3).
        let content = crate::content::ContentStore::for_repo(&repo, backend)?;
        let keeps = |path: &RepoPathBuf| match &carry {
            Some(set) => {
                set.contains(path)
                    || set.iter().any(|kept| {
                        // A carried directory carries its subtree.
                        path.as_internal_file_string()
                            .starts_with(&format!("{}/", kept.as_internal_file_string()))
                    })
            }
            None => true,
        };
        // A carry-forward path that names nothing tracked is almost
        // certainly a typo, and the cost of honoring it is clearing
        // the whole tree (PR #290 review): refuse before removing
        // anything. Matching mirrors `keeps`: exact path or directory
        // prefix of something tracked.
        if let Some(set) = &carry {
            for kept in set {
                let prefix = format!("{}/", kept.as_internal_file_string());
                let hits = head_paths.contains(kept)
                    || head_paths
                        .iter()
                        .any(|p| p.as_internal_file_string().starts_with(&prefix));
                if !hits {
                    return Err(Error::BadRequest(format!(
                        "carry-forward path {:?} matches nothing tracked — nothing was cleared",
                        kept.as_internal_file_string()
                    )));
                }
            }
        }

        struct Mover {
            repo_path: RepoPathBuf,
            disk: PathBuf,
            copy_id: jj_lib::backend::CopyId,
        }
        let mut removed: Vec<PathBuf> = Vec::new();
        let mut movers: Vec<Mover> = Vec::new();
        for repo_path in &head_paths {
            if keeps(repo_path) {
                continue;
            }
            let disk = root_path.join(repo_path.as_internal_file_string());
            if !disk.exists() {
                continue;
            }
            let Some(existing) = pollster::block_on(task_files_version_store::chain::lookup_dyn(
                backend, &head_tree, repo_path,
            ))?
            else {
                continue;
            };
            let (head_id, copy_id) = match existing {
                jj_lib::backend::TreeValue::File { id, copy_id, .. } => (id, copy_id),
                _ => continue,
            };
            let as_mover = |disk: &Path| Mover {
                repo_path: repo_path.clone(),
                disk: disk.to_path_buf(),
                copy_id: copy_id.clone(),
            };
            // A stub clears without a content verify: its content is
            // in the store by construction and its bytes are a
            // placeholder — but the stat sandwich still runs, because
            // a save can replace the stub with real content mid-flip.
            let is_stub = stub::probe(&disk).is_some();
            // Every removal rides the same certify sandwich as a
            // checkpoint read (PR #290 review): an external writer —
            // DAW, WebDAV — holds none of our locks, so a file that
            // moved between the verify and the delete is a mid-flip
            // save. It is never deleted: it becomes a mover, kept on
            // the old iteration as flagged divergence.
            let guard = crate::certify::StatGuard::begin(&disk)?;
            if !is_stub {
                let disk_id = content.probe(&disk)?.ok_or_else(|| {
                    Error::Repo(
                        "this root's backend cannot derive content ids without writing".into(),
                    )
                })?;
                if disk_id != head_id {
                    movers.push(as_mover(&disk));
                    continue;
                }
            }
            match guard.check(&disk)? {
                crate::certify::Settled::Stable => {}
                // Moved, or timestamps too coarse to prove otherwise:
                // treat as a mid-flip save rather than re-reading —
                // the mover path re-verifies by content anyway.
                _ => {
                    movers.push(as_mover(&disk));
                    continue;
                }
            }
            match std::fs::remove_file(&disk) {
                Ok(()) => removed.push(disk),
                // Vanished concurrently: exactly the outcome a removal
                // wants; aborting a restart half-done over it would be
                // worse than the race (PR #290 review).
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        crate::restart::prune_empty_dirs(&root_path, &removed);

        // 4. Mid-flip saves: one sibling commit of the old head
        // carrying every mover at its saved content. Two heads —
        // the new lineage's start below and this — is exactly the
        // Divergent-versions shape the badges flag and #267 resolves.
        // The flip commits through whichever repo handle is newest, so
        // its op parents chain rather than diverge.
        let mut flip_repo = repo.clone();
        if !movers.is_empty() {
            let store = repo.store().clone();
            let mut builder =
                jj_lib::tree_builder::TreeBuilder::new(store.clone(), head_tree_id.clone());
            let mut settled: Vec<bool> = Vec::with_capacity(movers.len());
            for mover in &movers {
                // Sandwich the ingest too: a THIRD save landing while
                // the mover streams into the store means the on-disk
                // bytes are newer than what the divergence commit will
                // hold — such a file is left on disk for the flip scan
                // to capture instead of being deleted below.
                let guard = crate::certify::StatGuard::begin(&mover.disk)?;
                let probed = content.probe(&mover.disk)?;
                let id = pollster::block_on(content.write(&mover.repo_path, &mover.disk, probed))?;
                builder.set(
                    mover.repo_path.clone(),
                    jj_lib::backend::TreeValue::File {
                        id,
                        executable: false,
                        copy_id: mover.copy_id.clone(),
                    },
                );
                settled.push(guard.check(&mover.disk)? == crate::certify::Settled::Stable);
            }
            let div_tree_id = pollster::block_on(builder.write_tree())
                .map_err(|e| Error::Repo(format!("mid-flip tree: {e}")))?;
            let merged = jj_lib::merged_tree::MergedTree::resolved(store, div_tree_id);
            let mut tx = repo.start_transaction();
            pollster::block_on(async {
                tx.repo_mut()
                    .new_commit(vec![head.clone()], merged)
                    .set_description("save landed mid-restart — kept on the old iteration")
                    .write()
                    .await
                    .map(|_| ())
            })
            .map_err(|e| Error::Repo(format!("mid-flip commit: {e}")))?;
            let committed = pollster::block_on(tx.commit("mid-flip divergence"))
                .map_err(|e| Error::Repo(e.to_string()))?;
            for (mover, settled) in movers.iter().zip(&settled) {
                if !settled {
                    // Still being written when it was ingested: the
                    // disk bytes may be newer than the divergence
                    // commit. Leave the file — the flip scan captures
                    // it into the new lineage; nothing is lost.
                    continue;
                }
                // Durably in the store (and flagged); the live tree
                // belongs to the new lineage now.
                if let Err(e) = std::fs::remove_file(&mover.disk)
                    && e.kind() != std::io::ErrorKind::NotFound
                {
                    return Err(e.into());
                }
            }
            flip_repo = committed;
        }

        // 5. Template seed, after the clear so the template's own
        // names never collide with removals.
        if let Some(source) = &template {
            crate::restart::seed_template(&root_path, source)?;
        }

        // 6. The flip: the new lineage's first checkpoint, through the
        // ordinary certified scan — parented on the OLD HEAD this
        // function has held since step 2, never on a reloaded pick:
        // `heads_of` deliberately follows descendants, so after the
        // mid-flip divergence commit it would choose that sibling as
        // the parent and silently absorb the save into the new lineage
        // — the exact opposite of "survives as flagged divergence".
        let backend = flip_repo.store().backend();
        let ignores = self.ignore_of(&root)?;
        let disk_files = scan::walk_live_tree(&root_path, root.flavor, &ignores, &head_paths)?;
        let next_number = self.versions.next_project_version_number(root_id)?;
        let result = crate::checkpoint::write_checkpoint(Capture {
            repo: &flip_repo,
            backend,
            parent_id: head.clone(),
            base_tree_id: head_tree_id.clone(),
            base_tree: &head_tree,
            disk_files: &disk_files,
            base_paths: &head_paths,
            description: format!("restart: Project Version v{next_number}"),
            attempts: self.cadence.config().certify_attempts,
            hook: None,
        })?;
        let at = self.cadence.now();
        let commit_hex = result.commit_id.hex();
        let store_dir = repo_open::store_dir(&root_path);
        let mut journal = Journal::load(&store_dir)?;
        journal.record_checkpoint(CheckpointRecord {
            commit_id: commit_hex.clone(),
            at,
            save_points: Vec::new(),
            requeued_paths: result.requeued_paths.clone(),
        });
        journal.save(&store_dir)?;
        self.set_heads(root_id, result.repo, result.commit_id.clone(), None);

        // 7. The entity, minted on the new lineage's first commit —
        // and the events replicas fold in like any other sync signal
        // (spec AC 4): the checkpoint that IS the flip, then the
        // Project Version that names it.
        let (commit_id, change_id) = self.resolve_commit(&root, &commit_hex)?;
        let pv = self.versions.create_project_version(
            root_id,
            &root.name,
            label
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty()),
            change_id.hex(),
            commit_id.hex(),
        )?;
        self.publish(FilesEvent::Checkpointed(CheckpointInfo {
            root_id,
            commit_id: commit_hex,
            description: format!("restart: Project Version v{}", pv.number),
            at,
            changed_paths: result.changed_paths,
            save_points: Vec::new(),
            requeued_paths: result.requeued_paths,
        }));
        self.publish(FilesEvent::ProjectVersionStarted(pv.clone()));
        Ok(pv)
    }

    fn browse_at_inner(
        &self,
        root_id: Uuid,
        commit_ref: String,
        subpath: String,
    ) -> Result<Vec<BrowseEntry>, Error> {
        let root = self.get_root_info(root_id)?;
        // Read path: open-only, reloaded to head — time-travel browsing
        // must neither initialize a store on a bare mountpoint nor
        // answer from a snapshot frozen at this process's last write
        // (PR #288's browse rule, applied here per PR #290's review).
        let Some((repo, _head)) = self.reload_existing_repo(&root)? else {
            return Err(Error::NotFound(format!(
                "{root_id}: no version store (never checkpointed, or its volume is not mounted)"
            )));
        };
        let (commit_id, _) = Self::resolve_commit_in(&repo, &root, &commit_ref)?;
        let dir = badges::repo_dir(&subpath)?;
        let backend = repo.store().backend();
        let listed = pollster::block_on(badges::listing(backend, &commit_id, &dir))?;
        if listed.is_empty() && !subpath.is_empty() {
            // Distinguish "empty directory / not a directory here" from
            // a real listing the same way browse does: absent is a 404.
            return Err(Error::NotFound(format!("{root_id}@{commit_ref}:{subpath}")));
        }
        let mut out: Vec<BrowseEntry> = listed
            .into_iter()
            .map(|(name, (is_dir, _state))| BrowseEntry {
                name,
                is_dir,
                // Tree entries carry identities, not lengths; nothing
                // is opened to answer a time-travel listing.
                size: None,
                stub: false,
                divergent: false,
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    fn copy_forward_inner(
        &self,
        root_id: Uuid,
        commit_ref: String,
        paths: Vec<String>,
    ) -> Result<Vec<String>, Error> {
        let root = self.get_root_info(root_id)?;
        // Media-only like every other verb that mutates the live tree
        // from Files' side: rewriting a Software root's colocated git
        // working tree behind git's back would hand git users surprise
        // modifications (PR #290 review) — `git checkout <commit> -- p`
        // is that flavor's copy-forward.
        Self::require_media(&root, "copy_forward")?;
        if paths.is_empty() {
            return Err(Error::BadRequest("copy_forward: no paths given".into()));
        }
        let lock = self.root_lock(root_id);
        let _guard = lock.lock().expect("root lock poisoned");
        let (repo, head) = self.reload_repo(&root)?;
        let backend = repo.store().backend();
        let (commit_id, _) = self.resolve_commit(&root, &commit_ref)?;
        let source_commit = pollster::block_on(backend.read_commit(&commit_id))?;
        let source_tree_id = source_commit
            .root_tree
            .clone()
            .into_resolved()
            .map_err(|_| {
                Error::Repo("copying forward from a conflicted tree is unsupported (v1)".into())
            })?;
        let source_tree = pollster::block_on(backend.read_tree(RepoPath::root(), &source_tree_id))?;
        let head_commit = pollster::block_on(backend.read_commit(&head))?;
        let head_tree_id = head_commit
            .root_tree
            .clone()
            .into_resolved()
            .map_err(|_| Error::Repo("conflicted head tree".into()))?;
        let head_tree = pollster::block_on(backend.read_tree(RepoPath::root(), &head_tree_id))?;
        let content = crate::content::ContentStore::for_repo(&repo, backend)?;

        // Two phases: validate everything, then write everything — a
        // multi-file copy-forward either starts cleanly or not at all.
        struct Planned {
            repo_path: RepoPathBuf,
            disk: PathBuf,
            id: jj_lib::backend::FileId,
            executable: bool,
        }
        let mut planned = Vec::new();
        let mut dirty = Vec::new();
        for p in &paths {
            let (disk, repo_path) = self.resolve_root_file(&root, p)?;
            let Some(jj_lib::backend::TreeValue::File { id, executable, .. }) = pollster::block_on(
                task_files_version_store::chain::lookup_dyn(backend, &source_tree, &repo_path),
            )?
            else {
                return Err(Error::NotFound(format!(
                    "{commit_ref}:{p}: not a file there"
                )));
            };
            if disk.exists() && stub::probe(&disk).is_none() {
                // Overwriting live content is the verb's point — but
                // only content that is already versioned. Unversioned
                // work is refused, never clobbered.
                let head_id = match pollster::block_on(
                    task_files_version_store::chain::lookup_dyn(backend, &head_tree, &repo_path),
                )? {
                    Some(jj_lib::backend::TreeValue::File { id, .. }) => Some(id),
                    _ => None,
                };
                let disk_id = content.probe(&disk)?.ok_or_else(|| {
                    Error::Repo(
                        "this root's backend cannot derive content ids without writing".into(),
                    )
                })?;
                if head_id.as_ref() != Some(&disk_id) {
                    dirty.push(p.clone());
                    continue;
                }
            }
            planned.push(Planned {
                repo_path,
                disk,
                id,
                executable,
            });
        }
        if !dirty.is_empty() {
            return Err(Error::BadRequest(format!(
                "unversioned changes at {} — checkpoint first, then copy forward",
                dirty.join(", ")
            )));
        }

        let mut written = Vec::new();
        for plan in planned {
            if let Some(parent) = plan.disk.parent() {
                std::fs::create_dir_all(parent)?;
            }
            self.restore_content(
                &repo,
                &plan.repo_path,
                &plan.disk,
                &plan.id,
                plan.executable,
            )?;
            written.push(plan.repo_path.as_internal_file_string().to_string());
        }
        written.sort();
        Ok(written)
    }
}

/// Divergent versions (issue #264, ADR 0001): listing the paths whose
/// state differs between the root's visible heads, and settling them.
/// Resolution is a **merge checkpoint** — every visible head becomes a
/// parent, so both sides stay in history and the root returns to one
/// head. jj's own concurrency model is what makes the listing correct:
/// concurrent writers leave multiple visible heads, never lost data.
impl FilesBackend {
    /// Every `(path, per-head state)` where the visible heads disagree,
    /// journal-line head first. Empty for a single-head root.
    fn divergences_inner(&self, root_id: Uuid) -> Result<Vec<files_proto::DivergenceInfo>, Error> {
        let root = self.get_root_info(root_id)?;
        let Some((repo, head)) = self.reload_existing_repo(&root)? else {
            return Ok(Vec::new());
        };
        let heads = self.ordered_heads(&repo, &root, &head);
        if heads.len() < 2 {
            return Ok(Vec::new());
        }
        let backend = repo.store().backend();
        let mut per_head: Vec<BTreeMap<RepoPathBuf, jj_lib::backend::FileId>> = Vec::new();
        for h in &heads {
            per_head.push(Self::tree_files_of(backend, h)?);
        }
        let mut all_paths: BTreeSet<RepoPathBuf> = BTreeSet::new();
        for map in &per_head {
            all_paths.extend(map.keys().cloned());
        }
        let mut out = Vec::new();
        for path in all_paths {
            let states: BTreeSet<Option<String>> = per_head
                .iter()
                .map(|m| m.get(&path).map(|id| id.hex()))
                .collect();
            if states.len() > 1 {
                out.push(files_proto::DivergenceInfo {
                    root_id,
                    path: path.as_internal_file_string().to_string(),
                    sides: heads
                        .iter()
                        .zip(&per_head)
                        .map(|(h, m)| files_proto::DivergenceSide {
                            commit_id: h.hex(),
                            file_id: m.get(&path).map(|id| id.hex()),
                        })
                        .collect(),
                });
            }
        }
        Ok(out)
    }

    /// The root's visible **checkpoint** heads with the journal-line
    /// head first — the stable "side A" every divergence surface
    /// reports. Ephemeral auto-snapshot tips are excluded (PR #291
    /// review): a snapshot branch is not a divergent version, and
    /// counting one would fold ephemeral captures into the chain and
    /// leak them to replicas via `sync_heads`.
    fn ordered_heads(
        &self,
        repo: &Arc<ReadonlyRepo>,
        root: &FileRootInfo,
        head: &CommitId,
    ) -> Vec<CommitId> {
        let known_snapshots: std::collections::HashSet<String> = Self::journal_of(root)
            .map(|j| {
                j.snapshots
                    .iter()
                    .map(|s| s.snapshot_id.clone())
                    .chain(j.snapshot_head.clone())
                    .collect()
            })
            .unwrap_or_default();
        let mut heads: Vec<CommitId> = vec![head.clone()];
        heads.extend(
            repo.view()
                .heads()
                .iter()
                .filter(|h| *h != head && !known_snapshots.contains(&h.hex()))
                .cloned()
                .collect::<BTreeSet<_>>(),
        );
        heads
    }

    /// Every file `path → FileId` in `commit`'s tree.
    fn tree_files_of(
        backend: &dyn Backend,
        commit_id: &CommitId,
    ) -> Result<BTreeMap<RepoPathBuf, jj_lib::backend::FileId>, Error> {
        let mut out = BTreeMap::new();
        pollster::block_on(async {
            let commit = backend.read_commit(commit_id).await?;
            let tree_id =
                commit.root_tree.clone().into_resolved().map_err(|_| {
                    jj_lib::backend::BackendError::Other("conflicted root tree".into())
                })?;
            let tree = backend.read_tree(RepoPath::root(), &tree_id).await?;
            Self::walk_tree_files(backend, &tree, RepoPath::root(), &mut out).await
        })
        .map_err(|e| Error::Repo(format!("walking {}: {e}", commit_id.hex())))?;
        Ok(out)
    }

    async fn walk_tree_files(
        backend: &dyn Backend,
        tree: &jj_lib::backend::Tree,
        prefix: &RepoPath,
        out: &mut BTreeMap<RepoPathBuf, jj_lib::backend::FileId>,
    ) -> std::result::Result<(), jj_lib::backend::BackendError> {
        for name in tree.names() {
            let Some(value) = tree.value(name) else {
                continue;
            };
            let path = prefix.join(name);
            match value {
                jj_lib::backend::TreeValue::Tree(id) => {
                    let sub = backend.read_tree(&path, id).await?;
                    Box::pin(Self::walk_tree_files(backend, &sub, &path, out)).await?;
                }
                jj_lib::backend::TreeValue::File { id, .. } => {
                    out.insert(path, id.clone());
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// The `<stem> (divergent n).<ext>` name a KeepBoth side lands
    /// under, beside the journal-line side's file. The extension split
    /// applies to the FILE NAME only, never a dot in a parent
    /// directory (`a.b/c` must not become `a (divergent 1).b/c`, which
    /// would relocate the sibling into a stray directory — PR #291
    /// review).
    fn divergent_name(path: &str, n: usize) -> String {
        let (dir, name) = match path.rsplit_once('/') {
            Some((dir, name)) => (Some(dir), name),
            None => (None, path),
        };
        let renamed = match name.rsplit_once('.') {
            Some((stem, ext)) if !stem.is_empty() => {
                format!("{stem} (divergent {n}).{ext}")
            }
            _ => format!("{name} (divergent {n})"),
        };
        match dir {
            Some(dir) => format!("{dir}/{renamed}"),
            None => renamed,
        }
    }

    fn resolve_divergence_inner(
        &self,
        root_id: Uuid,
        path: String,
        choice: files_proto::DivergenceChoice,
    ) -> Result<CheckpointInfo, Error> {
        use files_proto::DivergenceChoice;
        let root = self.get_root_info(root_id)?;
        Self::require_media(&root, "resolve_divergence")?;
        let (_, repo_path) = self.resolve_root_file(&root, &path)?;
        let lock = self.root_lock(root_id);
        let _guard = lock.lock().expect("root lock poisoned");

        let (repo, head) = self.reload_repo(&root)?;
        let heads = self.ordered_heads(&repo, &root, &head);
        if heads.len() < 2 {
            return Err(Error::BadRequest(format!(
                "{root_id}: no divergence to resolve"
            )));
        }
        let backend = repo.store().backend();
        let mut per_head: Vec<BTreeMap<RepoPathBuf, jj_lib::backend::FileId>> = Vec::new();
        for h in &heads {
            per_head.push(Self::tree_files_of(backend, h)?);
        }
        let named_states: BTreeSet<Option<String>> = per_head
            .iter()
            .map(|m| m.get(&repo_path).map(|id| id.hex()))
            .collect();
        if named_states.len() < 2 {
            return Err(Error::BadRequest(format!("{path}: not divergent")));
        }

        // The resolved tree starts from the journal-line head (the live
        // tree's own line) and takes the decided state for the named
        // path. Every OTHER divergent path defaults to KeepBoth — one
        // resolution returns the root to a single head, and "nothing is
        // lost" outranks tidiness for the paths nobody named: their
        // other sides land as `(divergent n)` files to keep or delete
        // as ordinary content. Resolve the paths you care about first.
        let head_commit = pollster::block_on(backend.read_commit(&head))?;
        let base_tree_id = head_commit
            .root_tree
            .clone()
            .into_resolved()
            .map_err(|_| Error::Repo("conflicted head tree".into()))?;
        let store = repo.store().clone();
        let mut builder = jj_lib::tree_builder::TreeBuilder::new(store.clone(), base_tree_id);
        let mut changed_paths: Vec<String> = Vec::new();
        // Disk materialization plan: (root-relative, Some(id, exec) to
        // restore | None to delete).
        let mut materialize: Vec<(RepoPathBuf, Option<jj_lib::backend::FileId>)> = Vec::new();

        let value_in = |head_idx: usize, p: &RepoPathBuf| per_head[head_idx].get(p).cloned();
        let tree_value_of = |commit_id: &CommitId,
                             p: &RepoPathBuf|
         -> Result<Option<jj_lib::backend::TreeValue>, Error> {
            let commit = pollster::block_on(backend.read_commit(commit_id))?;
            let tree_id = commit
                .root_tree
                .clone()
                .into_resolved()
                .map_err(|_| Error::Repo("conflicted tree".into()))?;
            let tree = pollster::block_on(backend.read_tree(RepoPath::root(), &tree_id))?;
            Ok(pollster::block_on(
                task_files_version_store::chain::lookup_dyn(backend, &tree, p),
            )?)
        };

        // A KeepBoth sibling name that is already tracked or on disk
        // must not be clobbered (PR #291 review): bump the counter past
        // any collision. `tracked` is every path any head knows; the
        // disk check catches an untracked file sitting there too.
        let tracked_names: BTreeSet<String> = per_head
            .iter()
            .flat_map(|m| m.keys().map(|p| p.as_internal_file_string().to_string()))
            .collect();
        let sibling_name = |base: &str, start: usize| -> Result<(String, RepoPathBuf), Error> {
            let mut n = start;
            loop {
                let candidate = Self::divergent_name(base, n);
                let (disk, rp) = self.resolve_root_file(&root, &candidate)?;
                if !tracked_names.contains(rp.as_internal_file_string()) && !disk.exists() {
                    return Ok((candidate, rp));
                }
                n += 1;
            }
        };

        // 1. The named path, per the choice.
        match &choice {
            DivergenceChoice::Pick { commit_id } => {
                // An empty or ambiguous id must never silently resolve
                // to side A (PR #291 review): require an exact match or
                // a non-empty unambiguous prefix.
                if commit_id.trim().is_empty() {
                    return Err(Error::BadRequest("pick: empty commit id".into()));
                }
                let matches: Vec<&CommitId> = heads
                    .iter()
                    .filter(|h| h.hex() == *commit_id || h.hex().starts_with(commit_id.as_str()))
                    .collect();
                let picked = match matches.as_slice() {
                    [one] => (*one).clone(),
                    [] => {
                        return Err(Error::BadRequest(format!(
                            "{commit_id}: not one of this root's divergent heads"
                        )));
                    }
                    _ => {
                        return Err(Error::BadRequest(format!(
                            "{commit_id}: ambiguous — names more than one head"
                        )));
                    }
                };
                match tree_value_of(&picked, &repo_path)? {
                    Some(value) => {
                        let id = match &value {
                            jj_lib::backend::TreeValue::File { id, .. } => id.clone(),
                            _ => return Err(Error::BadRequest(format!("{path}: not a file"))),
                        };
                        builder.set(repo_path.clone(), value);
                        materialize.push((repo_path.clone(), Some(id)));
                    }
                    None => {
                        builder.remove(repo_path.clone());
                        materialize.push((repo_path.clone(), None));
                    }
                }
                changed_paths.push(path.clone());
            }
            DivergenceChoice::KeepBoth => {
                // Side A already sits in the base tree; each other
                // side's distinct content lands beside it under a
                // collision-free `(divergent n)` name.
                let a_state = value_in(0, &repo_path).map(|id| id.hex());
                let mut n = 1;
                for (idx, h) in heads.iter().enumerate().skip(1) {
                    let side = value_in(idx, &repo_path);
                    if side.as_ref().map(|id| id.hex()) == a_state {
                        continue;
                    }
                    if let Some(id) = side {
                        let (sibling, sibling_path) = sibling_name(&path, n)?;
                        n += 1;
                        let value = tree_value_of(h, &repo_path)?.ok_or_else(|| {
                            Error::Repo(format!("{path}: side vanished mid-resolve"))
                        })?;
                        builder.set(sibling_path.clone(), value);
                        materialize.push((sibling_path, Some(id)));
                        changed_paths.push(sibling);
                    }
                }
            }
        }

        // 2. Every other divergent path: KeepBoth by default.
        let mut all_paths: BTreeSet<RepoPathBuf> = BTreeSet::new();
        for m in &per_head {
            all_paths.extend(m.keys().cloned());
        }
        for other in all_paths {
            if other == repo_path {
                continue;
            }
            let states: BTreeSet<Option<String>> = per_head
                .iter()
                .map(|m| m.get(&other).map(|id| id.hex()))
                .collect();
            if states.len() < 2 {
                continue;
            }
            let a_state = value_in(0, &other).map(|id| id.hex());
            let mut n = 1;
            for (idx, h) in heads.iter().enumerate().skip(1) {
                let side = value_in(idx, &other);
                let side_hex = side.as_ref().map(|id| id.hex());
                if side_hex == a_state {
                    continue;
                }
                if let Some(id) = side {
                    let other_str = other.as_internal_file_string().to_string();
                    let (sibling, sibling_path) = sibling_name(&other_str, n)?;
                    n += 1;
                    let value = tree_value_of(h, &other)?.ok_or_else(|| {
                        Error::Repo(format!("{other_str}: side vanished mid-resolve"))
                    })?;
                    builder.set(sibling_path.clone(), value);
                    materialize.push((sibling_path, Some(id)));
                    changed_paths.push(sibling);
                }
            }
        }

        // Never destroy unversioned work when materializing the decided
        // state (PR #291 review — the same rule `materialize_head`
        // enforces): a materialize target that exists on disk with
        // content the store does not hold is an unversioned edit;
        // refuse rather than overwrite or delete it.
        let content = crate::content::ContentStore::for_repo(&repo, backend)?;
        let root_path = PathBuf::from(&root.path);
        let mut dirty = Vec::new();
        for (p, _want) in &materialize {
            let disk = root_path.join(p.as_internal_file_string());
            if !disk.exists() || stub::probe(&disk).is_some() {
                continue;
            }
            if let Some(disk_id) = content.probe(&disk)? {
                let known = match task_files_chunk_store::FileId::from_hex(&disk_id.hex()) {
                    Ok(fid) => self
                        .with_version_store(root_id, |vs| pollster::block_on(vs.chunks().has(fid)))
                        .map_err(|e| Error::Repo(e.to_string()))?,
                    Err(_) => false,
                };
                if !known {
                    dirty.push(p.as_internal_file_string().to_string());
                }
            }
        }
        if !dirty.is_empty() {
            dirty.sort();
            return Err(Error::BadRequest(format!(
                "unversioned changes at {} — checkpoint first, then resolve",
                dirty.join(", ")
            )));
        }

        // 3. One merge checkpoint over every head: the decided state is
        // the single new head; every side stays reachable in history.
        let new_tree_id = pollster::block_on(builder.write_tree())
            .map_err(|e| Error::Repo(format!("resolution tree: {e}")))?;
        let merged = jj_lib::merged_tree::MergedTree::resolved(store, new_tree_id);
        let mut tx = repo.start_transaction();
        let commit = pollster::block_on(async {
            tx.repo_mut()
                .new_commit(heads.clone(), merged)
                .set_description(format!("resolve divergence: {path}"))
                .write()
                .await
        })
        .map_err(|e| Error::Repo(format!("resolution commit: {e}")))?;
        let commit_id = commit.id().clone();
        let new_repo = pollster::block_on(tx.commit("resolve divergence"))
            .map_err(|e| Error::Repo(e.to_string()))?;

        // 4. Materialize the decided state into the live tree.
        for (p, want) in &materialize {
            let disk = root_path.join(p.as_internal_file_string());
            match want {
                Some(id) => {
                    if let Some(parent) = disk.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    self.restore_content(&new_repo, p, &disk, id, false)?;
                }
                None => {
                    if let Err(e) = std::fs::remove_file(&disk)
                        && e.kind() != std::io::ErrorKind::NotFound
                    {
                        return Err(e.into());
                    }
                }
            }
        }

        let at = self.cadence.now();
        let commit_hex = commit_id.hex();
        let store_dir = repo_open::store_dir(&root_path);
        let mut journal = Journal::load(&store_dir)?;
        journal.record_checkpoint(CheckpointRecord {
            commit_id: commit_hex.clone(),
            at,
            save_points: Vec::new(),
            requeued_paths: Vec::new(),
        });
        journal.save(&store_dir)?;
        self.set_heads(root_id, new_repo, commit_id, None);
        changed_paths.sort();
        let info = CheckpointInfo {
            root_id,
            commit_id: commit_hex,
            description: format!("resolve divergence: {path}"),
            at,
            changed_paths,
            save_points: Vec::new(),
            requeued_paths: Vec::new(),
        };
        self.publish(FilesEvent::Checkpointed(info.clone()));
        Ok(info)
    }

    /// Adopt a directory as a **replica** of an existing root (issue
    /// #264): same root id, same name, fresh local store. This is the
    /// local half of replica creation — the content arrives by
    /// reconcile (the `files-sync` crate). Not an RPC: the sync daemon
    /// (#265) drives it on its own machine.
    /// `flavor` is the flavor of the root being replicated. Only
    /// [`RootFlavor::Media`] is supported (PR #291 review): this engine
    /// is the CAS chunk store's reconcile, and a media replica's whole
    /// materialize path assumes it. A software root is replicated by
    /// cloning its colocated git — a different, git-native path — so a
    /// non-media flavor is refused here rather than silently adopted as
    /// media (which would give it wrong semantics and a half-applied,
    /// then-erroring reconcile).
    pub fn adopt_replica(
        &self,
        root_id: Uuid,
        name: &str,
        path: &str,
        flavor: RootFlavor,
    ) -> Result<FileRootInfo, FilesError> {
        self.adopt_replica_inner(root_id, name, path, flavor)
            .map_err(to_files_error)
    }

    fn adopt_replica_inner(
        &self,
        root_id: Uuid,
        name: &str,
        path: &str,
        flavor: RootFlavor,
    ) -> Result<FileRootInfo, Error> {
        if flavor != RootFlavor::Media {
            return Err(Error::BadRequest(
                "replica sync is media-only; a software root is replicated by cloning its git"
                    .into(),
            ));
        }
        let requested = PathBuf::from(path);
        std::fs::create_dir_all(&requested)?;
        let canonical = self.confine(&requested)?;
        let canonical_str = canonical
            .to_str()
            .ok_or_else(|| Error::BadRequest(format!("{path}: not valid UTF-8")))?
            .to_string();
        if canonical.join(MARKER_FILE).exists() {
            return Err(Error::AlreadyExists(canonical_str));
        }
        if let Some(existing) = self.registry.conflicting_root(&canonical) {
            return Err(Error::AlreadyExists(format!(
                "{canonical_str} overlaps existing root {} ({})",
                existing.id, existing.path
            )));
        }
        let repo = repo_open::open_or_init_repo(&canonical, RootFlavor::Media)?;
        let head = Self::head_of(&repo, RootFlavor::Media)?;
        let marker = serde_json::json!({ "id": root_id, "name": name });
        std::fs::write(
            canonical.join(MARKER_FILE),
            serde_json::to_vec_pretty(&marker)?,
        )?;
        let root = FileRootInfo {
            id: root_id,
            name: name.to_string(),
            path: canonical_str,
            flavor: RootFlavor::Media,
            created_at: Utc::now(),
            project_version: None,
        };
        self.registry.insert(root.clone())?;
        self.set_heads(root_id, repo, head, None);
        self.ignore_of(&root)?;
        self.publish(FilesEvent::RootCreated(root.clone()));
        Ok(root)
    }
}

/// The sync seam (issue #264): what the `files-sync` crate's reconcile
/// engine needs from a backend on either side of a replica
/// relationship. Serve-side accessors are thin and verified; the two
/// mutating entry points — importing a remote head and materializing
/// the live tree — carry the safety rules (parents-present,
/// never-clobber-unversioned-work).
impl FilesBackend {
    /// The root's visible heads, hex, journal-line first.
    pub fn sync_heads(&self, root_id: Uuid) -> Result<Vec<String>, FilesError> {
        let root = self.get_root_info(root_id).map_err(to_files_error)?;
        let Some((repo, head)) = self.reload_existing_repo(&root).map_err(to_files_error)? else {
            return Ok(Vec::new());
        };
        Ok(self
            .ordered_heads(&repo, &root, &head)
            .iter()
            .map(|h| h.hex())
            .collect())
    }

    /// Raw structural-object bytes (commit/tree/copy-history) by hex id.
    pub fn sync_object(&self, root_id: Uuid, id_hex: &str) -> Result<Vec<u8>, FilesError> {
        let bytes = hex_bytes(id_hex)?;
        self.with_version_store(root_id, |vs| pollster::block_on(vs.read_raw_object(&bytes)))?
            .map_err(|e| to_files_error(Error::VersionStore(e)))
    }

    /// Does this root's store hold the object? (The root commit's
    /// all-zero id is virtual on every store and reports `true`.)
    pub fn sync_has_object(&self, root_id: Uuid, id_hex: &str) -> Result<bool, FilesError> {
        if id_hex.chars().all(|c| c == '0') {
            return Ok(true);
        }
        let bytes = hex_bytes(id_hex)?;
        self.with_version_store(root_id, |vs| {
            pollster::block_on(vs.read_raw_object(&bytes)).is_ok()
        })
    }

    /// Decode a fetched commit's `(parent hex ids, tree hex id)`
    /// **without storing it** — reconcile imports a commit's closure
    /// before the commit object itself (issue #264 / PR #291 review:
    /// a commit's presence must mean its whole closure is present, so
    /// an interrupted pull is re-runnable).
    pub fn sync_decode_commit(&self, bytes: &[u8]) -> Result<(Vec<String>, String), FilesError> {
        let (parents, tree) = VersionStoreBackend::decode_commit_meta(bytes)
            .map_err(|e| to_files_error(Error::VersionStore(e)))?;
        Ok((
            parents.iter().map(|p| bytes_to_hex(p)).collect(),
            bytes_to_hex(&tree),
        ))
    }

    /// Import one structural object received from a peer (hash-verified
    /// inside the store).
    pub fn sync_import_object(
        &self,
        root_id: Uuid,
        id_hex: &str,
        bytes: Vec<u8>,
    ) -> Result<(), FilesError> {
        let id = hex_bytes(id_hex)?;
        self.with_version_store(root_id, |vs| {
            pollster::block_on(vs.import_raw_object(&id, bytes))
        })?
        .map_err(|e| to_files_error(Error::VersionStore(e)))
    }

    /// A file's chunk manifest as `(chunk hash hex, len)` pairs.
    pub fn sync_manifest(
        &self,
        root_id: Uuid,
        file_id_hex: &str,
    ) -> Result<Vec<(String, u64)>, FilesError> {
        let file_id = chunk_file_id_from_hex(file_id_hex)?;
        self.with_version_store(root_id, |vs| {
            pollster::block_on(vs.chunks().manifest(file_id))
        })?
        .map(|m| {
            m.chunks
                .iter()
                .map(|c| (c.hash.to_hex().to_string(), c.len))
                .collect()
        })
        .map_err(|e| to_files_error(Error::VersionStore(e.into())))
    }

    /// Does this root's chunk store hold a manifest for the file?
    pub fn sync_has_manifest(&self, root_id: Uuid, file_id_hex: &str) -> Result<bool, FilesError> {
        let file_id = chunk_file_id_from_hex(file_id_hex)?;
        self.with_version_store(root_id, |vs| pollster::block_on(vs.chunks().has(file_id)))
    }

    /// Does this root's chunk store hold the chunk?
    pub fn sync_has_chunk(&self, root_id: Uuid, hash_hex: &str) -> Result<bool, FilesError> {
        let hash = chunk_hash_from_hex(hash_hex)?;
        self.with_version_store(root_id, |vs| {
            pollster::block_on(vs.chunks().has_chunk(hash))
        })?
        .map_err(|e| to_files_error(Error::VersionStore(e.into())))
    }

    /// One chunk's bytes.
    pub fn sync_read_chunk(&self, root_id: Uuid, hash_hex: &str) -> Result<Vec<u8>, FilesError> {
        let hash = chunk_hash_from_hex(hash_hex)?;
        self.with_version_store(root_id, |vs| {
            pollster::block_on(vs.chunks().read_chunk(hash))
        })?
        .map_err(|e| to_files_error(Error::VersionStore(e.into())))
    }

    /// Hold GC quiescent for a file's whole chunk+manifest import
    /// (issue #264 / PR #291 review): synced chunks have no manifest
    /// protecting them until the manifest lands, so the caller holds
    /// this across the import to stop a sweep in that window.
    pub fn sync_gc_quiesce(
        &self,
        root_id: Uuid,
    ) -> Result<tokio::sync::OwnedRwLockReadGuard<()>, FilesError> {
        self.with_version_store(root_id, |vs| {
            pollster::block_on(vs.chunks().gc_quiesce_guard())
        })
    }

    /// Import one chunk (hash-verified inside the store).
    pub fn sync_import_chunk(
        &self,
        root_id: Uuid,
        hash_hex: &str,
        bytes: Vec<u8>,
    ) -> Result<(), FilesError> {
        let hash = chunk_hash_from_hex(hash_hex)?;
        self.with_version_store(root_id, |vs| {
            pollster::block_on(vs.chunks().import_chunk(hash, bytes))
        })?
        .map_err(|e| to_files_error(Error::VersionStore(e.into())))
    }

    /// Import a manifest once every chunk it references is present.
    pub fn sync_import_manifest(
        &self,
        root_id: Uuid,
        file_id_hex: &str,
        chunks: Vec<(String, u64)>,
    ) -> Result<(), FilesError> {
        let expected = chunk_file_id_from_hex(file_id_hex)?;
        let mut refs = Vec::with_capacity(chunks.len());
        for (hash_hex, len) in chunks {
            refs.push(task_files_chunk_store::ChunkRef {
                hash: chunk_hash_from_hex(&hash_hex)?,
                len,
            });
        }
        let manifest = task_files_chunk_store::Manifest::new(refs);
        if manifest.file_id() != expected {
            return Err(FilesError::BadRequest(format!(
                "manifest re-derives to {}, peer claimed {file_id_hex}",
                manifest.file_id().to_hex()
            )));
        }
        self.with_version_store(root_id, |vs| {
            pollster::block_on(vs.chunks().import_manifest(&manifest)).map(|_| ())
        })?
        .map_err(|e| to_files_error(Error::VersionStore(e.into())))
    }

    /// A commit's `(parents, root tree id)` read from the LOCAL store —
    /// the puller decodes objects only after importing them, so the
    /// wire never needs a structured commit format.
    pub fn sync_commit_meta(
        &self,
        root_id: Uuid,
        commit_hex: &str,
    ) -> Result<(Vec<String>, String), FilesError> {
        let id = CommitId::try_from_hex(commit_hex)
            .ok_or_else(|| FilesError::BadRequest(format!("{commit_hex}: not a commit id")))?;
        self.with_repo(root_id, |repo| {
            let backend = repo.store().backend();
            let commit = pollster::block_on(backend.read_commit(&id))
                .map_err(|e| to_files_error(Error::JjBackend(e)))?;
            let tree = commit
                .root_tree
                .clone()
                .into_resolved()
                .map_err(|_| FilesError::Io("conflicted root tree".into()))?;
            Ok((commit.parents.iter().map(|p| p.hex()).collect(), tree.hex()))
        })?
    }

    /// One tree level's `(subtree ids, (file id, copy id) pairs)`, read
    /// from the LOCAL store after import.
    #[allow(clippy::type_complexity)]
    pub fn sync_tree_meta(
        &self,
        root_id: Uuid,
        tree_hex: &str,
    ) -> Result<SyncTreeMeta, FilesError> {
        let id = jj_lib::backend::TreeId::try_from_hex(tree_hex)
            .ok_or_else(|| FilesError::BadRequest(format!("{tree_hex}: not a tree id")))?;
        self.with_repo(root_id, |repo| {
            let backend = repo.store().backend();
            let tree = pollster::block_on(backend.read_tree(RepoPath::root(), &id))
                .map_err(|e| to_files_error(Error::JjBackend(e)))?;
            let mut subtrees = Vec::new();
            let mut files = Vec::new();
            for name in tree.names() {
                let name_str = name.as_internal_str().to_string();
                match tree.value(name) {
                    Some(jj_lib::backend::TreeValue::Tree(t)) => subtrees.push((name_str, t.hex())),
                    Some(jj_lib::backend::TreeValue::File { id, copy_id, .. }) => {
                        let copy = (!copy_id.as_bytes().is_empty()).then(|| copy_id.hex());
                        files.push((name_str, id.hex(), copy));
                    }
                    _ => {}
                }
            }
            Ok(SyncTreeMeta { subtrees, files })
        })?
    }

    /// Make a fully-imported remote commit a visible head of this
    /// root's repo. jj's own view semantics take it from there: a head
    /// descending from the local line fast-forwards it (the journal
    /// follows via `heads_of`); anything else stays a sibling —
    /// Divergent versions, flagged until resolved. Every ancestor
    /// object must already be imported.
    pub fn import_remote_head(&self, root_id: Uuid, commit_hex: &str) -> Result<(), FilesError> {
        let root = self.get_root_info(root_id).map_err(to_files_error)?;
        let id = CommitId::try_from_hex(commit_hex)
            .ok_or_else(|| FilesError::BadRequest(format!("{commit_hex}: not a commit id")))?;
        let lock = self.root_lock(root_id);
        let _guard = lock.lock().expect("root lock poisoned");
        let (repo, _head) = self.reload_repo(&root).map_err(to_files_error)?;
        if repo.view().heads().contains(&id) {
            return Ok(());
        }
        let commit = pollster::block_on(repo.store().get_commit_async(&id))
            .map_err(|e| FilesError::Io(format!("reading imported head: {e}")))?;
        let mut tx = repo.start_transaction();
        pollster::block_on(tx.repo_mut().add_head(&commit))
            .map_err(|e| FilesError::Io(format!("adding head: {e}")))?;
        let new_repo = pollster::block_on(tx.commit("sync: import remote head"))
            .map_err(|e| FilesError::Io(e.to_string()))?;
        let (head, snapshot_head) = Self::heads_of(&new_repo, &root).map_err(to_files_error)?;
        self.set_heads(root_id, new_repo, head, snapshot_head);
        Ok(())
    }

    /// Bring the live tree in line with the (journal-line) head —
    /// replica checkout, hydration-policy aware (issue #263: matching
    /// paths hydrate, the rest become stubs), and **safe**: a disk file
    /// whose content the store does not hold anywhere is unversioned
    /// work — it is never overwritten or deleted, only reported.
    pub fn materialize_head(&self, root_id: Uuid) -> Result<MaterializeReport, FilesError> {
        self.materialize_head_inner(root_id).map_err(to_files_error)
    }

    fn materialize_head_inner(&self, root_id: Uuid) -> Result<MaterializeReport, Error> {
        let root = self.get_root_info(root_id)?;
        Self::require_media(&root, "materialize")?;
        let root_path = PathBuf::from(&root.path);
        let lock = self.root_lock(root_id);
        let _guard = lock.lock().expect("root lock poisoned");
        let (repo, head) = self.reload_repo(&root)?;
        let backend = repo.store().backend();
        let target = Self::tree_files_of(backend, &head)?;
        let store_dir = repo_open::store_dir(&root_path);
        let policy = hydration::matcher(&store_dir)?;
        let content = crate::content::ContentStore::for_repo(&repo, backend)?;
        let mut report = MaterializeReport::default();

        // Disk side: anything tracked-on-disk that the head no longer
        // has, or holds at different content.
        let ignores = self.ignore_of(&root)?;
        let tracked_paths: BTreeSet<RepoPathBuf> = target.keys().cloned().collect();
        let disk_files = scan::walk_live_tree(&root_path, root.flavor, &ignores, &tracked_paths)?;
        let mut on_disk: BTreeSet<RepoPathBuf> = BTreeSet::new();
        for file in &disk_files {
            if file.ignored {
                continue;
            }
            on_disk.insert(file.repo_path.clone());
            let want = target.get(&file.repo_path);
            let current_id = match &file.stub {
                Some(s) => Some(s.file_id()?),
                None => content.probe(&file.disk_path)?,
            };
            match (want, current_id) {
                (Some(want_id), Some(cur)) if *want_id == cur => {
                    // Content agrees; hydration state may still change
                    // below via the policy pass.
                }
                (want, Some(cur)) => {
                    // Disk differs. Known content (the store holds a
                    // manifest for it) is versioned SOMEWHERE — safe to
                    // move aside; unknown content is unversioned work
                    // and stays untouched.
                    let known = match task_files_chunk_store::FileId::from_hex(&cur.hex()) {
                        Ok(cur_id) => self
                            .with_version_store(root_id, |vs| {
                                pollster::block_on(vs.chunks().has(cur_id))
                            })
                            .map_err(|e| Error::Repo(e.to_string()))?,
                        Err(_) => false,
                    };
                    if !known {
                        report
                            .kept_dirty
                            .push(file.repo_path.as_internal_file_string().to_string());
                        continue;
                    }
                    match want {
                        Some(want_id) => {
                            self.write_materialized(
                                &root,
                                &repo,
                                &file.repo_path,
                                want_id,
                                &policy,
                                &mut report,
                            )?;
                        }
                        None => {
                            if let Err(e) = std::fs::remove_file(&file.disk_path)
                                && e.kind() != std::io::ErrorKind::NotFound
                            {
                                return Err(e.into());
                            }
                            report
                                .removed
                                .push(file.repo_path.as_internal_file_string().to_string());
                        }
                    }
                }
                (None, None) => {}
                (Some(_), None) => {
                    // Unprobeable backend — already errored in probe.
                }
            }
        }

        // Store side: every tracked path absent from disk materializes
        // per the hydration policy.
        for (path, want_id) in &target {
            if on_disk.contains(path) {
                // Present: reconcile hydration STATE with the policy
                // (hydrate a stub the policy wants resident; a resident
                // file the policy wants dehydrated is left for
                // apply_hydration_policy — materialize never dehydrates).
                let disk = root_path.join(path.as_internal_file_string());
                let is_stub = stub::probe(&disk).is_some();
                let keep = policy
                    .as_deref()
                    .is_some_and(|p| hydration::keeps_hydrated(p, path.as_internal_file_string()));
                if is_stub && keep {
                    self.write_materialized(&root, &repo, path, want_id, &policy, &mut report)?;
                }
                continue;
            }
            self.write_materialized(&root, &repo, path, want_id, &policy, &mut report)?;
        }
        report.written.sort();
        report.stubbed.sort();
        report.removed.sort();
        report.kept_dirty.sort();
        Ok(report)
    }

    /// Write one path's materialized form: resident content when the
    /// hydration policy keeps it (or there is no policy), a stub
    /// otherwise (partial replica, issue #264's AC 4).
    fn write_materialized(
        &self,
        root: &FileRootInfo,
        repo: &Arc<ReadonlyRepo>,
        path: &RepoPath,
        want_id: &jj_lib::backend::FileId,
        policy: &Option<Arc<jj_lib::gitignore::GitIgnoreFile>>,
        report: &mut MaterializeReport,
    ) -> Result<(), Error> {
        let root_path = PathBuf::from(&root.path);
        let disk = root_path.join(path.as_internal_file_string());
        if let Some(parent) = disk.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let hydrate = match policy.as_deref() {
            // No policy: a full replica — everything resident.
            None => true,
            Some(p) => hydration::keeps_hydrated(p, path.as_internal_file_string()),
        };
        let rel = path.as_internal_file_string().to_string();
        if hydrate {
            self.restore_content(repo, path, &disk, want_id, false)?;
            report.written.push(rel);
        } else {
            let file_id = task_files_chunk_store::FileId::from_hex(&want_id.hex())
                .map_err(|e| Error::VersionStore(e.into()))?;
            let size = self
                .with_version_store(root.id, |vs| {
                    pollster::block_on(vs.chunks().manifest(file_id))
                })
                .map_err(|e| Error::Repo(e.to_string()))?
                .map(|m| m.total_len())
                .map_err(|e| Error::VersionStore(e.into()))?;
            stub::write(&disk, &stub::Stub::new(want_id, size, false))?;
            report.stubbed.push(rel);
        }
        Ok(())
    }
}

/// What one [`FilesBackend::materialize_head`] pass did, root-relative
/// paths, sorted.
#[derive(Debug, Default, Clone)]
pub struct MaterializeReport {
    /// Hydrated to resident content.
    pub written: Vec<String>,
    /// Materialized as pointer stubs (partial replica).
    pub stubbed: Vec<String>,
    /// Removed (their content is store-held; the head no longer has
    /// them).
    pub removed: Vec<String>,
    /// Left untouched: on-disk content the store holds nowhere —
    /// unversioned work materialization must never destroy.
    pub kept_dirty: Vec<String>,
}

/// One tree level, decoded for the replica-sync walk (issue #264/#265):
/// child subtrees and files by NAME, so the reconcile can build each
/// file's full root-relative path for per-file progress reporting.
#[derive(Debug, Clone)]
pub struct SyncTreeMeta {
    /// `(entry name, subtree id hex)`.
    pub subtrees: Vec<(String, String)>,
    /// `(entry name, file id hex, copy id hex)`.
    pub files: Vec<(String, String, Option<String>)>,
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn hex_bytes(hex: &str) -> Result<Vec<u8>, FilesError> {
    if !hex.len().is_multiple_of(2) {
        return Err(FilesError::BadRequest(format!("{hex}: odd-length hex")));
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&hex[i..i + 2], 16)
                .map_err(|_| FilesError::BadRequest(format!("{hex}: not hex")))
        })
        .collect()
}

fn chunk_hash_from_hex(hex: &str) -> Result<task_files_chunk_store::blake3::Hash, FilesError> {
    task_files_chunk_store::blake3::Hash::from_hex(hex)
        .map_err(|e| FilesError::BadRequest(format!("{hex}: {e}")))
}

fn chunk_file_id_from_hex(hex: &str) -> Result<task_files_chunk_store::FileId, FilesError> {
    task_files_chunk_store::FileId::from_hex(hex)
        .map_err(|e| FilesError::BadRequest(format!("{hex}: {e}")))
}

/// Map a rendition-store read failure: a CAS id the store doesn't hold
/// is `NotFound` (the streaming route's 404), anything else is `Io`.
fn rendition_read_err(file_id_hex: &str, e: files_transcode::Error) -> FilesError {
    match e {
        files_transcode::Error::ChunkStore(task_files_chunk_store::Error::UnknownFileId(_)) => {
            FilesError::NotFound(format!("rendition {file_id_hex}"))
        }
        other => FilesError::Io(format!("rendition {file_id_hex}: {other}")),
    }
}

/// Derived media (issue #269): the `rendition` RPC, the checkpoint
/// warm-up trigger, and the source-tied rendition GC.
impl FilesBackend {
    /// The transcoder if one is configured.
    fn transcoder_opt(&self) -> Option<Arc<dyn files_transcode::Transcoder>> {
        self.transcoder.lock().expect("transcoder lock").clone()
    }

    /// The root's rendition store, opened once and cached (a private
    /// `FsStore` — opening a second on one dir hangs).
    async fn rendition_store(
        &self,
        root_id: Uuid,
        root_path: &Path,
    ) -> Result<Arc<files_transcode::RenditionStore>, Error> {
        if let Some(store) = self
            .rendition_stores
            .lock()
            .expect("rendition store cache")
            .get(&root_id)
        {
            return Ok(store.clone());
        }
        // Serialize the open: two concurrent misses must not both open
        // an `FsStore` on one dir (that hangs). Re-check the cache once
        // the lock is held — the winner populated it.
        let _open = self.rendition_open_lock.lock().await;
        if let Some(store) = self
            .rendition_stores
            .lock()
            .expect("rendition store cache")
            .get(&root_id)
        {
            return Ok(store.clone());
        }
        let store = Arc::new(crate::transcode::open_store(root_path).await?);
        self.rendition_stores
            .lock()
            .expect("rendition store cache")
            .insert(root_id, store.clone());
        Ok(store)
    }

    /// Resolve a media file at `path` to its source CAS `FileId` — at
    /// the checkpoint head, or at `at` (a commit reference) for the
    /// version switcher (issue #270 AC 4) — plus the root's chunk
    /// store: the sync prep a rendition needs before the async
    /// generate.
    fn rendition_prep(
        &self,
        root_id: Uuid,
        path: &str,
        at: Option<&str>,
    ) -> Result<
        (
            Arc<task_files_chunk_store::ChunkStore>,
            task_files_chunk_store::FileId,
            PathBuf,
        ),
        Error,
    > {
        let root = self.get_root_info(root_id)?;
        Self::require_media(&root, "rendition")?;
        let (_disk, repo_path) = self.resolve_root_file(&root, path)?;
        let (repo, head) = self.reload_repo(&root)?;
        let commit = match at {
            Some(reference) => self.resolve_commit(&root, reference)?.0,
            None => head,
        };
        let Some((source_id, _exec)) = Self::head_file(&repo, &commit, &repo_path)? else {
            return Err(Error::NotFound(format!(
                "{path}: not tracked by that version"
            )));
        };
        let source_fid = task_files_chunk_store::FileId::from_hex(&source_id.hex())
            .map_err(|e| Error::Repo(format!("source file id: {e}")))?;
        let chunks = self
            .with_version_store(root_id, |vs| vs.chunks().clone())
            .map_err(from_files_error)?;
        Ok((chunks, source_fid, PathBuf::from(&root.path)))
    }

    async fn rendition_inner(
        &self,
        root_id: Uuid,
        path: String,
        at: Option<String>,
        kind: files_proto::RenditionKind,
    ) -> Result<files_proto::RenditionInfo, Error> {
        let Some(transcoder) = self.transcoder_opt() else {
            return Err(Error::NotFound(
                "no transcoder configured on this server".into(),
            ));
        };
        let this = self.clone();
        let p = path.clone();
        let (chunks, source_fid, root_path) =
            blocking(move || this.rendition_prep(root_id, &p, at.as_deref()))
                .await
                .map_err(from_files_error)?;
        let store = self.rendition_store(root_id, &root_path).await?;
        let ekind = crate::transcode::engine_kind(kind);
        // Generate-once (AC 2): hold a per-`(root, source, kind)` lock
        // across the whole generate so a second concurrent request for
        // the same uncached rendition waits and then hits the cache
        // rather than running ffmpeg a second time.
        let lock_key = format!("{root_id}:{}:{}", source_fid.to_hex(), ekind.tag());
        let keyed = {
            let mut locks = self
                .rendition_gen_locks
                .lock()
                .expect("rendition gen locks");
            locks.entry(lock_key.clone()).or_default().clone()
        };
        let _gen = keyed.lock().await;
        let pipe = files_transcode::TranscodePipeline::new(chunks, store, transcoder);
        let result = pipe.rendition(&source_fid, ekind).await;
        // Drop the entry once we're its last holder (only our `keyed`
        // clone and the map's own), so the lock map stays bounded.
        {
            let mut locks = self
                .rendition_gen_locks
                .lock()
                .expect("rendition gen locks");
            if let Some(entry) = locks.get(&lock_key)
                && Arc::strong_count(entry) <= 2
            {
                locks.remove(&lock_key);
            }
        }
        let rendition = result.map_err(|e| match e {
            files_transcode::Error::NotMedia(m) => Error::BadRequest(m),
            other => Error::Repo(format!("transcode: {other}")),
        })?;
        Ok(files_proto::RenditionInfo {
            file_id: rendition.file_id.to_hex(),
            len: rendition.len,
            mime: rendition.kind.mime().to_string(),
            kind,
        })
    }

    /// Warm up (pre-generate) every media file's rendition ladder in
    /// `head`'s tree — the checkpoint trigger (AC 1). Best-effort: a
    /// failed rendition is logged, never fatal to the checkpoint that
    /// spawned it. Skips when no transcoder is configured.
    async fn warm_up_head(&self, root_id: Uuid, head: CommitId) {
        let Some(transcoder) = self.transcoder_opt() else {
            return;
        };
        let this = self.clone();
        let sources = match blocking(move || this.head_source_ids(root_id, &head)).await {
            Ok(s) => s,
            Err(err) => {
                tracing::warn!(%root_id, %err, "transcode warm-up: reading head failed");
                return;
            }
        };
        let root_path = match self.get_root_info(root_id) {
            Ok(r) => PathBuf::from(&r.path),
            Err(_) => return,
        };
        let chunks = match self.with_version_store(root_id, |vs| vs.chunks().clone()) {
            Ok(c) => c,
            Err(_) => return,
        };
        let store = match self.rendition_store(root_id, &root_path).await {
            Ok(s) => s,
            Err(err) => {
                tracing::warn!(%root_id, %err, "transcode warm-up: store failed");
                return;
            }
        };
        let pipe = files_transcode::TranscodePipeline::new(chunks, store, transcoder);
        for source in sources {
            if let Err(err) = pipe.warm_up(&source).await {
                tracing::warn!(%root_id, source = %source.to_hex(), %err, "transcode warm-up failed");
            }
        }
    }

    /// Every file's source CAS `FileId` in `head`'s tree.
    fn head_source_ids(
        &self,
        root_id: Uuid,
        head: &CommitId,
    ) -> Result<Vec<task_files_chunk_store::FileId>, Error> {
        let (repo, _) = self.ensure_repo(&self.get_root_info(root_id)?)?;
        let backend = repo.store().backend();
        let files = Self::tree_files_of(backend, head)?;
        let mut out = Vec::new();
        for (_path, id) in files {
            if let Ok(fid) = task_files_chunk_store::FileId::from_hex(&id.hex()) {
                out.push(fid);
            }
        }
        Ok(out)
    }

    /// Whether a rendition's content (by hex CAS id) is present in the
    /// root's rendition store — test/introspection observability for the
    /// GC (a swept rendition's content is gone from here).
    pub async fn rendition_content_present(
        &self,
        root_id: Uuid,
        file_id_hex: &str,
    ) -> Result<bool, FilesError> {
        let root = self.get_root_info(root_id).map_err(to_files_error)?;
        let store = self
            .rendition_store(root_id, Path::new(&root.path))
            .await
            .map_err(to_files_error)?;
        let fid = task_files_chunk_store::FileId::from_hex(file_id_hex)
            .map_err(|e| FilesError::BadRequest(format!("{file_id_hex}: {e}")))?;
        Ok(store.has_content(fid).await)
    }

    /// Stream a rendition's bytes (by hex CAS id, from the `rendition`
    /// RPC's [`files_proto::RenditionInfo`]) to `dest`. Renditions live
    /// in a *private* CAS, so the source-content read paths can't reach
    /// them — this is how the Review page's streaming route (issue #270)
    /// serves a proxy or filmstrip.
    pub async fn read_rendition<W>(
        &self,
        root_id: Uuid,
        file_id_hex: &str,
        dest: &mut W,
    ) -> Result<(), FilesError>
    where
        W: tokio::io::AsyncWrite + Unpin,
    {
        let root = self.get_root_info(root_id).map_err(to_files_error)?;
        let store = self
            .rendition_store(root_id, Path::new(&root.path))
            .await
            .map_err(to_files_error)?;
        let fid = task_files_chunk_store::FileId::from_hex(file_id_hex)
            .map_err(|e| FilesError::BadRequest(format!("{file_id_hex}: {e}")))?;
        store
            .read_to(fid, dest)
            .await
            .map_err(|e| rendition_read_err(file_id_hex, e))
    }

    /// A rendition's total byte length — the Review page's streaming
    /// route (issue #270) needs it to build a `Content-Range`.
    pub async fn rendition_len(&self, root_id: Uuid, file_id_hex: &str) -> Result<u64, FilesError> {
        let root = self.get_root_info(root_id).map_err(to_files_error)?;
        let store = self
            .rendition_store(root_id, Path::new(&root.path))
            .await
            .map_err(to_files_error)?;
        let fid = task_files_chunk_store::FileId::from_hex(file_id_hex)
            .map_err(|e| FilesError::BadRequest(format!("{file_id_hex}: {e}")))?;
        store
            .content_len(fid)
            .await
            .map_err(|e| rendition_read_err(file_id_hex, e))
    }

    /// Stream a byte range of a rendition (the `<video>`-seek path, issue
    /// #270) — reads only the overlapping chunks, so a seek doesn't pull
    /// the whole proxy.
    pub async fn read_rendition_range<W>(
        &self,
        root_id: Uuid,
        file_id_hex: &str,
        start: u64,
        len: u64,
        dest: &mut W,
    ) -> Result<(), FilesError>
    where
        W: tokio::io::AsyncWrite + Unpin,
    {
        let root = self.get_root_info(root_id).map_err(to_files_error)?;
        let store = self
            .rendition_store(root_id, Path::new(&root.path))
            .await
            .map_err(to_files_error)?;
        let fid = task_files_chunk_store::FileId::from_hex(file_id_hex)
            .map_err(|e| FilesError::BadRequest(format!("{file_id_hex}: {e}")))?;
        store
            .read_range(fid, start, len, dest)
            .await
            .map_err(|e| rendition_read_err(file_id_hex, e))
    }

    /// One review by id — the share routes' per-hit lookup (issue
    /// #272): one targeted vault read, never an org-wide listing on a
    /// byte-serving path.
    pub async fn get_review(&self, id: Uuid) -> Result<files_proto::Review, FilesError> {
        let this = self.clone();
        blocking(move || this.versions.review(id)).await
    }

    /// Place an outside file into the root's live tree — the share
    /// promotion path (issue #272 AC 3). Uses the same safeguards as
    /// every other live-tree write: path confinement (including
    /// symlinked parents — the deepest existing ancestor must
    /// canonicalize inside the root), the root lock (a cadence capture
    /// must not snapshot a half-copied file), and never-overwrite.
    pub async fn place_file(
        &self,
        root_id: Uuid,
        dest_rel: String,
        src: PathBuf,
    ) -> Result<(), FilesError> {
        let this = self.clone();
        blocking(move || {
            let root = this.get_root_info(root_id)?;
            let (disk, _repo_path) = this.resolve_root_file(&root, &dest_rel)?;
            let root_canon = std::fs::canonicalize(&root.path).map_err(Error::Io)?;
            let mut probe = disk.parent().map(std::path::Path::to_path_buf);
            while let Some(p) = probe {
                if p.exists() {
                    let canon = std::fs::canonicalize(&p).map_err(Error::Io)?;
                    if !canon.starts_with(&root_canon) {
                        return Err(Error::BadRequest(format!(
                            "{dest_rel}: destination escapes the root"
                        )));
                    }
                    break;
                }
                probe = p.parent().map(std::path::Path::to_path_buf);
            }
            let lock = this.root_lock(root_id);
            let _guard = lock.lock().expect("root lock poisoned");
            if disk.exists() {
                return Err(Error::AlreadyExists(format!(
                    "{dest_rel}: already exists in the live tree"
                )));
            }
            if let Some(parent) = disk.parent() {
                std::fs::create_dir_all(parent).map_err(Error::Io)?;
            }
            std::fs::copy(&src, &disk).map_err(Error::Io)?;
            Ok(())
        })
        .await
    }

    /// The root's current checkpoint-head commit (hex) — what share
    /// serving pins a slice link's whole surface to (issue #271):
    /// listing and bytes must describe the same tree.
    pub async fn head_commit_hex(&self, root_id: Uuid) -> Result<String, FilesError> {
        let this = self.clone();
        blocking(move || {
            let root = this.get_root_info(root_id)?;
            let (_repo, head) = this.reload_repo(&root)?;
            Ok(head.hex())
        })
        .await
    }

    /// Pin a source file as of `at` (`None` = the checkpoint head) to
    /// its immutable CAS identity: `(byte length, hex FileId)` — the
    /// share-link download path (issue #271) resolves ONCE, then
    /// streams by id, so a checkpoint landing mid-request can't produce
    /// a `Content-Length`/body mismatch. Media roots only: their
    /// content lives in the chunk CAS; a software root's history is
    /// git's.
    pub async fn resolve_source(
        &self,
        root_id: Uuid,
        path: String,
        at: Option<String>,
    ) -> Result<(u64, String), FilesError> {
        let this = self.clone();
        let p = path.clone();
        let (chunks, fid, _root) =
            blocking(move || this.rendition_prep(root_id, &p, at.as_deref())).await?;
        let len = chunks
            .content_len(fid)
            .await
            .map_err(|e| FilesError::Io(format!("{path}: {e}")))?;
        Ok((len, fid.to_hex()))
    }

    /// Stream a source file's bytes by the pinned CAS id
    /// [`FilesBackend::resolve_source`] returned — content-addressed,
    /// so the bytes are exactly the resolved version's, whatever has
    /// happened to the root since.
    pub async fn read_source_content<W>(
        &self,
        root_id: Uuid,
        file_id_hex: &str,
        dest: &mut W,
    ) -> Result<(), FilesError>
    where
        W: tokio::io::AsyncWrite + Unpin,
    {
        let chunks = self.with_version_store(root_id, |vs| vs.chunks().clone())?;
        let fid = chunk_file_id_from_hex(file_id_hex)?;
        chunks
            .read_to(fid, dest)
            .await
            .map_err(|e| FilesError::Io(format!("source {file_id_hex}: {e}")))
    }

    /// Prune a root's renditions whose source content the store no
    /// longer holds, or whose recipe is superseded — the source-tied
    /// half of GC (AC 3/4). Called after `gc_root`'s version-store
    /// sweep, with the chunk store as the liveness oracle. Returns the
    /// index entries removed.
    fn gc_renditions(&self, root_id: Uuid) -> Result<u64, Error> {
        let root = self.get_root_info(root_id)?;
        if root.flavor != RootFlavor::Media {
            return Ok(0);
        }
        let root_path = PathBuf::from(&root.path);
        // Nothing to sweep — and don't create a rendition store (its
        // dirs, a private `FsStore`) on a server that never rendered
        // anything (no transcoder configured, or nothing warmed yet).
        if !crate::transcode::rendition_dir(&root_path).exists() {
            return Ok(0);
        }
        let chunks = self
            .with_version_store(root_id, |vs| vs.chunks().clone())
            .map_err(from_files_error)?;
        let store = pollster::block_on(self.rendition_store(root_id, &root_path))?;
        pollster::block_on(async {
            // Resolve liveness for every referenced source up front
            // (async), then hand `gc` a plain sync predicate — a nested
            // `block_on` inside the gc scan deadlocks. Live = the
            // SOURCE's manifest is still in the source chunk store (the
            // version-store sweep just ran, so a dead source's manifest
            // is already gone).
            let mut live = std::collections::HashSet::new();
            for src_hex in store
                .source_ids()
                .await
                .map_err(|e| Error::Repo(format!("rendition sources: {e}")))?
            {
                if let Ok(fid) = task_files_chunk_store::FileId::from_hex(&src_hex)
                    && chunks.has(fid).await
                {
                    live.insert(src_hex);
                }
            }
            store
                .gc(|src_hex| live.contains(src_hex))
                .await
                .map_err(|e| Error::Repo(format!("rendition gc: {e}")))
        })
    }
}

/// `FilesError` back to the crate error, for the few call sites that
/// bridge a `blocking()`-wrapped inner call inside another async method.
fn from_files_error(err: FilesError) -> Error {
    match err {
        FilesError::NotFound(m) => Error::NotFound(m),
        FilesError::AlreadyExists(m) => Error::AlreadyExists(m),
        FilesError::BadRequest(m) => Error::BadRequest(m),
        FilesError::Io(m) => Error::Repo(m),
    }
}

impl FilesService for FilesBackend {
    async fn create_root(
        &self,
        path: String,
        name: String,
        flavor: RootFlavor,
    ) -> Result<FileRootInfo, FilesError> {
        let this = self.clone();
        blocking(move || this.create_root_inner(path, name, flavor)).await
    }

    async fn list_roots(&self) -> Result<Vec<FileRootInfo>, FilesError> {
        // On the blocking pool like every other method here: the
        // lineage overlay scans the vault, and one root on a sleeping
        // drive must not stall a runtime worker for every org.
        let this = self.clone();
        blocking(move || Ok(this.with_project_version(this.registry.list()))).await
    }

    async fn get_root(&self, id: Uuid) -> Result<FileRootInfo, FilesError> {
        let this = self.clone();
        blocking(move || {
            let root = this.get_root_info(id)?;
            Ok(this
                .with_project_version(vec![root])
                .pop()
                .expect("one root in, one root out"))
        })
        .await
    }

    async fn browse(&self, root_id: Uuid, subpath: String) -> Result<Vec<BrowseEntry>, FilesError> {
        let this = self.clone();
        blocking(move || this.browse_inner(root_id, subpath)).await
    }

    async fn dehydrate(&self, root_id: Uuid, path: String) -> Result<BrowseEntry, FilesError> {
        let this = self.clone();
        blocking(move || this.dehydrate_inner(root_id, path)).await
    }

    async fn hydrate(&self, root_id: Uuid, path: String) -> Result<BrowseEntry, FilesError> {
        let this = self.clone();
        blocking(move || this.hydrate_inner(root_id, path)).await
    }

    async fn hydration_policy(&self, root_id: Uuid) -> Result<Vec<String>, FilesError> {
        let this = self.clone();
        blocking(move || this.hydration_policy_inner(root_id)).await
    }

    async fn set_hydration_policy(
        &self,
        root_id: Uuid,
        patterns: Vec<String>,
    ) -> Result<Vec<String>, FilesError> {
        let this = self.clone();
        blocking(move || this.set_hydration_policy_inner(root_id, patterns)).await
    }

    async fn apply_hydration_policy(&self, root_id: Uuid) -> Result<HydrationReport, FilesError> {
        let this = self.clone();
        blocking(move || this.apply_hydration_policy_inner(root_id)).await
    }

    async fn drive_browse(&self, path: String) -> Result<Vec<BrowseEntry>, FilesError> {
        let this = self.clone();
        blocking(move || this.drive_browse_inner(path)).await
    }

    async fn tree_browse(&self, path: String) -> Result<files_proto::TreeNode, FilesError> {
        let this = self.clone();
        blocking(move || this.tree_browse_inner(path)).await
    }

    async fn chain(&self, root_id: Uuid, path: String) -> Result<Vec<ChainEntry>, FilesError> {
        let this = self.clone();
        blocking(move || this.chain_inner(root_id, path)).await
    }

    async fn checkpoint_now(
        &self,
        root_id: Uuid,
        description: Option<String>,
    ) -> Result<CheckpointInfo, FilesError> {
        let this = self.clone();
        blocking(move || this.checkpoint_now_inner(root_id, description)).await
    }

    async fn hint_activity(&self, root_id: Uuid, paths: Vec<String>) -> Result<u32, FilesError> {
        // On the blocking pool like its neighbours: the first hint for a
        // root compiles (and may seed) its Ignore set off disk.
        let this = self.clone();
        blocking(move || this.hint_activity_inner(root_id, paths)).await
    }

    async fn snapshots(&self, root_id: Uuid) -> Result<Vec<SnapshotInfo>, FilesError> {
        let this = self.clone();
        blocking(move || this.snapshots_inner(root_id)).await
    }

    async fn ignore_set(&self, root_id: Uuid) -> Result<Vec<String>, FilesError> {
        let this = self.clone();
        blocking(move || this.ignore_set_inner(root_id)).await
    }

    async fn set_ignore_set(
        &self,
        root_id: Uuid,
        patterns: Vec<String>,
    ) -> Result<Vec<String>, FilesError> {
        let this = self.clone();
        blocking(move || this.set_ignore_set_inner(root_id, patterns)).await
    }

    async fn name_version(
        &self,
        root_id: Uuid,
        commit_id: String,
        name: String,
    ) -> Result<NamedVersion, FilesError> {
        let this = self.clone();
        let named = blocking(move || this.name_version_inner(root_id, commit_id, name)).await?;
        self.publish(FilesEvent::VersionNamed(named.clone()));
        Ok(named)
    }

    async fn list_named_versions(
        &self,
        root_id: Option<Uuid>,
    ) -> Result<Vec<NamedVersion>, FilesError> {
        let this = self.clone();
        blocking(move || this.versions.named_versions(root_id)).await
    }

    async fn resolve_named_version(&self, id: Uuid) -> Result<VersionRef, FilesError> {
        let this = self.clone();
        blocking(move || this.resolve_named_version_inner(id)).await
    }

    async fn unname_version(&self, id: Uuid) -> Result<(), FilesError> {
        let this = self.clone();
        let removed = blocking(move || this.unname_version_inner(id)).await?;
        self.publish(FilesEvent::VersionUnnamed(removed));
        Ok(())
    }

    async fn start_project_version(
        &self,
        root_id: Uuid,
        label: Option<String>,
    ) -> Result<ProjectVersion, FilesError> {
        let this = self.clone();
        let pv = blocking(move || this.start_project_version_inner(root_id, label)).await?;
        self.publish(FilesEvent::ProjectVersionStarted(pv.clone()));
        Ok(pv)
    }

    async fn list_project_versions(
        &self,
        root_id: Uuid,
    ) -> Result<Vec<ProjectVersion>, FilesError> {
        let this = self.clone();
        blocking(move || this.versions.project_versions(root_id)).await
    }

    async fn find_review(
        &self,
        root_id: Uuid,
        file_path: String,
    ) -> Result<Option<files_proto::Review>, FilesError> {
        let this = self.clone();
        blocking(move || this.find_review_inner(root_id, &file_path)).await
    }

    async fn review_for_file(
        &self,
        root_id: Uuid,
        file_path: String,
    ) -> Result<files_proto::Review, FilesError> {
        let this = self.clone();
        let (review, created) =
            blocking(move || this.review_for_file_inner(root_id, file_path)).await?;
        if created {
            self.publish(FilesEvent::ReviewCreated(review.clone()));
        }
        Ok(review)
    }

    async fn list_reviews(
        &self,
        root_id: Option<Uuid>,
    ) -> Result<Vec<files_proto::Review>, FilesError> {
        let this = self.clone();
        blocking(move || this.versions.reviews(root_id)).await
    }

    async fn review_comments(
        &self,
        review_id: Uuid,
    ) -> Result<Vec<files_proto::ReviewComment>, FilesError> {
        let this = self.clone();
        blocking(move || this.versions.review_comments(review_id)).await
    }

    async fn add_review_comment(
        &self,
        review_id: Uuid,
        comment: files_proto::NewReviewComment,
    ) -> Result<files_proto::ReviewComment, FilesError> {
        let this = self.clone();
        let added =
            blocking(move || this.add_review_comment_inner(review_id, comment, String::new()))
                .await?;
        self.publish(FilesEvent::ReviewCommentAdded(added.clone()));
        Ok(added)
    }

    async fn delete_review_comment(&self, id: Uuid) -> Result<(), FilesError> {
        let this = self.clone();
        let removed = blocking(move || {
            let comment = this.versions.review_comment(id)?;
            let lock = this.root_lock_for_review(&comment)?;
            let _guard = lock.lock().expect("root lock poisoned");
            this.versions.delete_review_comment(id)
        })
        .await?;
        self.publish(FilesEvent::ReviewCommentDeleted(removed));
        Ok(())
    }

    async fn restart_project_version(
        &self,
        root_id: Uuid,
        mode: files_proto::RestartMode,
        label: Option<String>,
    ) -> Result<ProjectVersion, FilesError> {
        let this = self.clone();
        blocking(move || this.restart_inner(root_id, mode, label)).await
    }

    async fn browse_at(
        &self,
        root_id: Uuid,
        commit_id: String,
        subpath: String,
    ) -> Result<Vec<BrowseEntry>, FilesError> {
        let this = self.clone();
        blocking(move || this.browse_at_inner(root_id, commit_id, subpath)).await
    }

    async fn copy_forward(
        &self,
        root_id: Uuid,
        commit_id: String,
        paths: Vec<String>,
    ) -> Result<Vec<String>, FilesError> {
        let this = self.clone();
        blocking(move || this.copy_forward_inner(root_id, commit_id, paths)).await
    }

    async fn divergences(
        &self,
        root_id: Uuid,
    ) -> Result<Vec<files_proto::DivergenceInfo>, FilesError> {
        let this = self.clone();
        blocking(move || this.divergences_inner(root_id)).await
    }

    async fn resolve_divergence(
        &self,
        root_id: Uuid,
        path: String,
        choice: files_proto::DivergenceChoice,
    ) -> Result<CheckpointInfo, FilesError> {
        let this = self.clone();
        blocking(move || this.resolve_divergence_inner(root_id, path, choice)).await
    }

    async fn gc_root(
        &self,
        root_id: Uuid,
        keep_newer_secs: Option<u64>,
    ) -> Result<GcReport, FilesError> {
        let this = self.clone();
        blocking(move || {
            let report = this.gc_root_inner(root_id, keep_newer_secs)?;
            // Fold in the source-tied rendition sweep (issue #269): a
            // rendition whose source content the version-store sweep
            // just removed is now dead too. Best-effort — a rendition
            // GC hiccup must not fail the main sweep's report.
            if let Err(err) = this.gc_renditions(root_id) {
                tracing::warn!(%root_id, %err, "rendition gc after gc_root failed");
            }
            Ok(report)
        })
        .await
    }

    async fn rendition(
        &self,
        root_id: Uuid,
        path: String,
        kind: files_proto::RenditionKind,
    ) -> Result<files_proto::RenditionInfo, FilesError> {
        self.rendition_inner(root_id, path, None, kind)
            .await
            .map_err(to_files_error)
    }

    async fn rendition_at(
        &self,
        root_id: Uuid,
        path: String,
        commit_id: String,
        kind: files_proto::RenditionKind,
    ) -> Result<files_proto::RenditionInfo, FilesError> {
        self.rendition_inner(root_id, path, Some(commit_id), kind)
            .await
            .map_err(to_files_error)
    }
}

/// The `#[subscribe]` backend contract: hand the emitted stream host
/// the hub it attaches subscriber sinks to. Publishing happens in the
/// `*_inner` methods above, on every successful mutation.
impl files_proto::service::legacy::FilesServiceStreamSource for FilesBackend {
    fn events_hub(&self) -> &architect::PubSub<FilesEvent> {
        &self.events
    }
}
