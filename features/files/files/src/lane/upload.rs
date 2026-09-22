//! `UploadService` — getting content in, `files.write.upload`.
//!
//! The lane exists to enforce two rules the spec states plainly and that
//! every naive upload implementation breaks.
//!
//! **Dedup happens before the transfer, not after.** A client that has
//! computed its own content address puts it in
//! [`UploadSpec::content`], and [`begin`](UploadService::begin) answers
//! from the chunk store — the FastCDC + BLAKE3 CAS under
//! `files_store::chunk` — rather than from the bytes. Content the store
//! already holds yields an empty `needed` list, which is the whole of
//! the answer and costs nothing. Content it holds *partially* (a
//! manifest is present, some chunks are not) yields exactly the missing
//! chunks' byte ranges, so an interrupted upload resumes rather than
//! restarts. Resumption is therefore a fact read out of the store, not a
//! guess held in a session: it survives anything that does not delete
//! chunks, including this process.
//!
//! **A collision asks.** [`complete`](UploadService::complete) takes the
//! [`OnConflict`] the caller chose and never infers one, and
//! [`UploadPlan::conflict`] reports the occupant *and* the name
//! `KeepBoth` would land under before the choice is made, so a human
//! decides knowing what they would displace. `Replace` checkpoints the
//! outgoing content first, so replacing records a new version instead of
//! destroying the old one.
//!
//! **A save can be safe.** [`UploadSpec::expect`] names what the caller
//! believes occupies the destination — nothing, or the content address a
//! previous read or save reported — and the landing checks it under the
//! root's lock. Two machines saving one session file therefore cannot
//! overwrite each other silently: the second gets `FilesFault::Stale`.
//! The address `complete` returns is the landed file's own, read back
//! from where it sits, because placement decides the address and the
//! next save's precondition must compare like with like.
//!
//! ## How bytes arrive
//!
//! [`send_bytes`](UploadService::send_bytes) writes frames into a
//! staging file outside the live tree and records which ranges arrived.
//! Only once every declared byte is there is the file ingested into the
//! root's chunk store — ingesting a file with holes would record zeros
//! as content — and `complete` then renames that staging file into
//! place. Content the store already held needs no transfer at all: it is
//! materialised out of the CAS at `complete`.
//!
//! **Sessions are per-process.** There is no upload store anywhere in
//! this codebase, so open sessions live in [`uploads`], a
//! process-lifetime `Mutex<Uploads>`. A restart forgets every open
//! session; it forgets no *progress*, because progress is the chunk
//! store's. A client whose session id is gone calls `begin` again with
//! the same spec and gets back a plan naming only the chunks still
//! missing. Nothing degrades to a wrong answer: a forgotten session
//! reads as `UploadNotFound`, never as a completed one.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use crate::error::FilesError;
use chrono::{DateTime, Duration, Utc};
use files_proto::error::FilesFault;
use files_proto::id::{ContentId, RootId, UploadId};
use files_proto::model::{FileRootInfo, RootFlavor};
use files_proto::path::RootPath;
use files_proto::service::access::Capability;
use files_proto::service::tree::{CatalogueEntry, EntryKind, Hydration};
use files_proto::service::upload::{
    ChunkRange, Conflict, Expect, Received, UploadFrame, UploadPlan, UploadProgress, UploadService,
    UploadSpec,
};
use files_proto::service::write::OnConflict;

use crate::backend::FilesBackend;
use crate::error::Error;

/// Where bytes are sent: this lane's own
/// [`send_bytes`](UploadService::send_bytes), over the connection the
/// plan arrived on. Named so a client reading [`UploadPlan::lane`] learns
/// the method, not a URL — there is no second endpoint.
pub const BYTE_LANE: &str = "files-upload/send_bytes";

/// How long an unfinished upload stays addressable.
///
/// Long enough to span an overnight transfer on a bad line, short
/// enough that abandoned sessions cannot accumulate without bound. An
/// expiry costs nothing but the session record: no bytes are ever
/// written outside `complete`, so an expired upload has nothing to
/// collect.
const SESSION_TTL_HOURS: i64 = 24;

/// The most `KeepBoth` siblings we will try before giving up.
///
/// A bound rather than an unbounded search: a directory holding a
/// thousand collisions of one name is a bug somewhere else, and looping
/// forever in a lock is a worse answer than a refusal.
const KEEP_BOTH_LIMIT: usize = 1000;

// ── In-memory sessions ─────────────────────────────────────────────

/// One upload the server has planned and not yet landed.
///
/// The spec is all a session needs to hold, because the *progress* is
/// not here — it is derived from the chunk store on every question. A
/// session caching `needed` would go stale the moment another upload,
/// a sync pull or a checkpoint brought the same chunks in.
#[derive(Debug, Clone)]
struct Session {
    spec: UploadSpec,
    expires_at: DateTime<Utc>,
    /// Byte ranges that have arrived over [`UploadService::send_bytes`]
    /// into this session's staging file, sorted and coalesced.
    ///
    /// The one piece of progress that *is* held here rather than read
    /// out of the store, because it has to be: until every byte has
    /// arrived the staging file holds holes, and ingesting a file with
    /// holes would record zeros as content. The store only hears about
    /// an upload once this covers the whole file.
    received: Vec<ChunkRange>,
    /// Who began it. `pending` answers per caller, so one person's
    /// half-finished uploads are not another's to list.
    owner: Option<files_proto::id::PrincipalId>,
}

impl Session {
    /// Every declared byte has arrived and sits in the staging file.
    fn staged_whole(&self) -> bool {
        missing(&self.received, self.spec.size).is_empty()
    }
}

#[derive(Debug, Default)]
struct Uploads(HashMap<UploadId, Session>);

impl Uploads {
    /// Drop what has expired. Called on every access rather than by a
    /// timer, so there is no sweeper to fail silently.
    fn sweep(&mut self, now: DateTime<Utc>) {
        self.0.retain(|_, s| s.expires_at > now);
    }
}

fn uploads() -> &'static Mutex<Uploads> {
    static UPLOADS: OnceLock<Mutex<Uploads>> = OnceLock::new();
    UPLOADS.get_or_init(Mutex::default)
}

fn with_uploads<T>(f: impl FnOnce(&mut Uploads) -> T) -> T {
    let mut guard = uploads().lock().expect("upload state lock poisoned");
    guard.sweep(Utc::now());
    f(&mut guard)
}

/// A live session, or the typed fault naming the id.
///
/// Expired and never-existed are the same answer on purpose: both mean
/// "there is nothing here to finish", and a client's recovery from
/// either is identical — call `begin` again, and pay only for what the
/// store still lacks.
fn session_of(upload_id: UploadId) -> Result<Session, FilesFault> {
    with_uploads(|u| u.0.get(&upload_id).cloned()).ok_or(FilesFault::UploadNotFound(upload_id))
}

fn forget(upload_id: UploadId) {
    with_uploads(|u| u.0.remove(&upload_id));
}

// ── Ranges ─────────────────────────────────────────────────────────

/// The whole file, or nothing at all for an empty one.
///
/// An empty file is genuinely complete before a byte moves, so it must
/// not be reported as a zero-length range a client would then try to
/// transfer.
fn whole(size: u64) -> Vec<ChunkRange> {
    if size == 0 {
        Vec::new()
    } else {
        vec![ChunkRange {
            start: 0,
            end: size,
        }]
    }
}

/// Append a range, coalescing with the previous one when they touch.
///
/// Consecutive missing chunks are one gap in the file, and reporting
/// them separately would make a resumed transfer issue one request per
/// chunk where one request would do.
fn push_range(out: &mut Vec<ChunkRange>, start: u64, end: u64) {
    match out.last_mut() {
        Some(last) if last.end == start => last.end = end,
        _ => out.push(ChunkRange { start, end }),
    }
}

/// Fold `[start, end)` into a sorted, coalesced range set.
fn merge_range(set: &mut Vec<ChunkRange>, start: u64, end: u64) {
    if start >= end {
        return;
    }
    set.push(ChunkRange { start, end });
    set.sort_by_key(|r| r.start);
    let mut out: Vec<ChunkRange> = Vec::with_capacity(set.len());
    for r in set.drain(..) {
        match out.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => out.push(r),
        }
    }
    *set = out;
}

/// The complement of a sorted, coalesced range set within `[0, size)`.
fn missing(have: &[ChunkRange], size: u64) -> Vec<ChunkRange> {
    let mut out = Vec::new();
    let mut at = 0u64;
    for r in have {
        if r.start > at {
            push_range(&mut out, at, r.start.min(size));
        }
        at = at.max(r.end);
        if at >= size {
            break;
        }
    }
    if at < size {
        push_range(&mut out, at, size);
    }
    out
}

fn outstanding_bytes(ranges: &[ChunkRange]) -> u64 {
    ranges.iter().map(|r| r.end.saturating_sub(r.start)).sum()
}

fn progress_of(upload_id: UploadId, spec: &UploadSpec, needed: Vec<ChunkRange>) -> UploadProgress {
    UploadProgress {
        upload_id,
        // Derived rather than counted: what the server holds is the
        // truth, and a counter incremented by the transport would
        // disagree with it the moment a chunk arrived by another route.
        received: spec.size.saturating_sub(outstanding_bytes(&needed)),
        total: spec.size,
        needed,
    }
}

/// The `<stem> (n).<ext>` name a keep-both landing uses.
///
/// The extension split applies to the **file name only** — a dot in a
/// parent directory must not be treated as one, or `a.b/c` would land
/// as `a (2).b/c` and quietly relocate the file into a stray directory.
/// (`backend.rs` enforces the same rule for divergence siblings; its
/// helper is private to that file, which is why the rule is restated
/// rather than shared.)
fn sibling(path: &RootPath, n: usize) -> Result<RootPath, FilesFault> {
    let raw = path.as_str();
    let (dir, name) = raw
        .rsplit_once('/')
        .map_or((None, raw), |(dir, name)| (Some(dir), name));
    let renamed = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => format!("{stem} ({n}).{ext}"),
        _ => format!("{name} ({n})"),
    };
    let joined = dir.map_or_else(|| renamed.clone(), |dir| format!("{dir}/{renamed}"));
    Ok(RootPath::parse(joined)?)
}

// ── Conversions ────────────────────────────────────────────────────

/// The legacy four-`String` surface onto the v2 one, matching what the
/// other lanes do so a caller sees one story whichever path a call took.
fn fault(err: FilesError) -> FilesFault {
    match err {
        FilesError::NotFound(m) | FilesError::BadRequest(m) => FilesFault::Invalid(m),
        FilesError::AlreadyExists(m) => FilesFault::AlreadyRoot(m),
        FilesError::Io(m) => FilesFault::Io(m),
    }
}

/// The same, for use inside a [`crate::lane::blocking`] closure, whose
/// error type is this crate's own.
fn as_error(err: FilesError) -> Error {
    match err {
        FilesError::NotFound(m) => Error::NotFound(m),
        FilesError::AlreadyExists(m) => Error::AlreadyExists(m),
        FilesError::BadRequest(m) => Error::BadRequest(m),
        FilesError::Io(m) => Error::Repo(m),
    }
}

/// The etag of a file in a root with no chunk store: `blake3:<hex>` over
/// its bytes, read in bounded memory.
fn plain_address(disk: &std::path::Path) -> Result<ContentId, Error> {
    let mut hasher = blake3::Hasher::new();
    hasher.update_reader(std::fs::File::open(disk)?)?;
    Ok(ContentId::new(format!(
        "blake3:{}",
        hasher.finalize().to_hex()
    )))
}

/// What [`FilesBackend::land`] made of a landing.
enum Landing {
    /// In place, with the landed file's content address.
    Landed(Option<ContentId>),
    /// The destination was taken and replacing was not chosen.
    Occupied,
    /// The caller's [`Expect`] did not hold; this is what is there.
    Stale(Option<ContentId>),
}

/// Refuse unless the caller may put content at `path`: `Write`, or
/// `Deposit` — the drop-folder capability, which is upload without read
/// and exists for exactly this lane.
async fn may_upload(
    backend: &FilesBackend,
    root_id: RootId,
    path: &RootPath,
) -> Result<(), FilesFault> {
    match crate::lane::caller::authorise(backend, root_id, path, Capability::Write).await {
        Ok(()) => Ok(()),
        Err(write) => crate::lane::caller::authorise(backend, root_id, path, Capability::Deposit)
            .await
            .map_err(|_| write),
    }
}

// ── The store questions ────────────────────────────────────────────

impl FilesBackend {
    /// The content address of what sits at `disk` right now, or `None`
    /// when nothing does. A directory has no address and reads as
    /// occupied-by-something-else, which no `Expect::Content` matches.
    ///
    /// A stub answers from itself — the stub *is* the address — so a
    /// dehydrated file can be safely replaced without hydrating it
    /// first. Anything else is probed through the root's chunk store:
    /// the address a write of these bytes would get, derived without
    /// writing, which is the same answer the catalogue reports.
    pub(crate) fn sync_content_at(
        &self,
        root: &FileRootInfo,
        disk: &std::path::Path,
    ) -> Result<Option<ContentId>, Error> {
        let Ok(meta) = std::fs::symlink_metadata(disk) else {
            return Ok(None);
        };
        if meta.is_dir() {
            return Ok(Some(ContentId::new("directory")));
        }
        if root.flavor != RootFlavor::Media {
            // A software root's history is its colocated git, and it has
            // no chunk store to ask. The etag is the bytes' own BLAKE3,
            // tagged so it can never be mistaken for a chunk address.
            return plain_address(disk).map(Some);
        }
        if crate::stub::candidate_len(meta.len())
            && let Some(stub) = crate::stub::probe(disk)
        {
            return Ok(Some(ContentId::new(stub.file_id)));
        }
        self.sync_probe_path(root.id, disk)
            .map(|hex| Some(ContentId::new(hex)))
            .map_err(as_error)
    }
    /// The byte ranges this server does not already hold.
    ///
    /// Three answers, in order of how much they cost:
    ///
    /// 1. **The manifest and every chunk are present** — an empty list.
    ///    Nothing transfers, whatever the file's size. This is the
    ///    dedup requirement, and it is answered by two store lookups.
    /// 2. **The manifest is present, some chunks are not** — exactly
    ///    the missing chunks' ranges, coalesced. This is resumption:
    ///    the store remembers what arrived, so an interrupted upload
    ///    picks up rather than starting over.
    /// 3. **No content address, or the store has never seen it** — the
    ///    whole file. A server cannot derive a content address without
    ///    the bytes it is trying not to ask for, so an upload that
    ///    supplies none pays in full; that is the client's choice, and
    ///    it is reported rather than worked around.
    ///
    /// Bytes that arrived over the wire count before any of that: a
    /// session whose staging file is whole needs nothing more, whatever
    /// the store says, and one that is part-way needs exactly its holes
    /// — (3) minus what was received.
    ///
    /// A software root skips (1) and (2): its content lives in its
    /// colocated git, not in the chunk CAS, so the CAS has nothing
    /// useful to say about it.
    async fn outstanding(
        &self,
        root: &FileRootInfo,
        spec: &UploadSpec,
        received: &[ChunkRange],
    ) -> Result<Vec<ChunkRange>, FilesFault> {
        let from_wire = missing(received, spec.size);
        if from_wire.is_empty() {
            return Ok(from_wire);
        }
        let (Some(content), RootFlavor::Media) = (spec.content.clone(), root.flavor) else {
            return Ok(from_wire);
        };
        // A partly-sent file whose address the store also knows: the
        // store's answer covers what the wire has not, but mixing the
        // two sources into one staging file is not something `stage`
        // does. The wire's holes are the honest answer.
        if !received.is_empty() {
            return Ok(from_wire);
        }
        let this = self.clone();
        let root_id = root.id;
        let hex = content.0;
        let size = spec.size;
        crate::lane::blocking(move || {
            if !this.sync_has_manifest(root_id, &hex).map_err(as_error)? {
                return Ok(whole(size));
            }
            let chunks = this.sync_manifest(root_id, &hex).map_err(as_error)?;
            let mut needed = Vec::new();
            let mut offset = 0u64;
            for (hash, len) in chunks {
                if !this.sync_has_chunk(root_id, &hash).map_err(as_error)? {
                    push_range(&mut needed, offset, offset + len);
                }
                offset += len;
            }
            // The address and the declared size disagree, so one of them
            // is wrong and we cannot tell which. Refusing is the only
            // honest move: accepting would land a file whose recorded
            // length is a lie.
            if offset != size {
                return Err(Error::BadRequest(format!(
                    "content {hex} describes {offset} bytes, the upload declares {size}"
                )));
            }
            Ok(needed)
        })
        .await
    }

    /// What is at `path` in the live tree right now, as a catalogue
    /// record — or `None` when the destination is free.
    ///
    /// Read from disk rather than from the catalogue lane: the
    /// catalogue is built once per process and nothing invalidates it,
    /// so a file created since would not be there, and a collision we
    /// failed to see is precisely the failure this lane exists to
    /// prevent.
    async fn occupant(
        &self,
        root: &FileRootInfo,
        path: &RootPath,
    ) -> Result<Option<CatalogueEntry>, FilesFault> {
        let this = self.clone();
        let root = root.clone();
        let path = path.clone();
        crate::lane::blocking(move || {
            let (disk, _) = this.resolve_root_file(&root, path.as_str())?;
            let Ok(meta) = std::fs::symlink_metadata(&disk) else {
                return Ok(None);
            };
            let now = Utc::now();
            // A stub is the one case where the address is known without
            // hashing — the stub file *is* the address. A resident file
            // is reported unverified rather than given an invented one.
            let stub = (!meta.is_dir()
                && root.flavor == RootFlavor::Media
                && crate::stub::candidate_len(meta.len()))
            .then(|| crate::stub::probe(&disk))
            .flatten();
            Ok(Some(CatalogueEntry {
                root_id: RootId::new(root.id),
                path,
                kind: if meta.is_dir() {
                    EntryKind::Directory
                } else {
                    EntryKind::File
                },
                size: stub.as_ref().map_or(meta.len(), |s| s.size),
                content: stub.as_ref().map(|s| ContentId(s.file_id.clone())),
                hydration: if stub.is_some() {
                    Hydration::Stub
                } else {
                    Hydration::Resident
                },
                locations: if meta.is_dir() {
                    Vec::new()
                } else {
                    // Empty on a host holding structure only: the
                    // entry is real, and no location here answers for it.
                    root.path.clone().into_iter().collect()
                },
                modified_at: meta.modified().map_or(now, DateTime::<Utc>::from),
                confirmed_at: now,
            }))
        })
        .await
    }

    /// The first free `<stem> (n).<ext>` beside `path`.
    ///
    /// Recomputed at the moment of landing as well as at plan time, so
    /// a name that filled up while a human was deciding is skipped
    /// rather than clobbered. The plan's `keep_both_as` is therefore a
    /// faithful preview, not a reservation.
    async fn keep_both_name(
        &self,
        root: &FileRootInfo,
        path: &RootPath,
    ) -> Result<RootPath, FilesFault> {
        for n in 2..=KEEP_BOTH_LIMIT {
            let candidate = sibling(path, n)?;
            if self.occupant(root, &candidate).await?.is_none() {
                return Ok(candidate);
            }
        }
        Err(FilesFault::invalid(format!(
            "{path}: no free keep-both name within {KEEP_BOTH_LIMIT} attempts"
        )))
    }

    /// Materialise the upload's content into a staging file outside the
    /// live tree, and hand back its path.
    ///
    /// Outside the tree is the point: `files.write.upload` says no
    /// partial file is ever visible, so nothing appears under the root
    /// until a completed, verified file can be renamed into place in
    /// one step. Staging under the backend's own data directory also
    /// keeps the rename on one filesystem in the ordinary case.
    async fn stage(
        &self,
        upload_id: UploadId,
        root_id: RootId,
        spec: &UploadSpec,
    ) -> Result<PathBuf, FilesFault> {
        let dir = self.data_dir().join("uploads");
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(FilesFault::io)?;
        let staged = dir.join(format!("{upload_id}.part"));
        let mut file = tokio::fs::File::create(&staged)
            .await
            .map_err(FilesFault::io)?;
        if let Some(content) = &spec.content {
            // Straight out of the CAS: the bytes are the ones that
            // address resolves to, so a dedup landing is byte-identical
            // to the content the client would have sent.
            self.read_source_content(root_id.get(), content.as_str(), &mut file)
                .await
                .map_err(fault)?;
        }
        file.sync_all().await.map_err(FilesFault::io)?;
        Ok(staged)
    }

    /// Move the staged file into the live tree in one step.
    ///
    /// Returns `false` when the destination was occupied and `replace`
    /// was not chosen — checked *inside* the root lock, because the
    /// gap between the plan and the landing is exactly where a
    /// concurrent writer gets to take the name.
    async fn land(
        &self,
        root: &FileRootInfo,
        dest: &RootPath,
        staged: PathBuf,
        replace: bool,
        expect: Option<Expect>,
    ) -> Result<Landing, FilesFault> {
        let this = self.clone();
        let root = root.clone();
        let dest = dest.clone();
        crate::lane::blocking(move || {
            let (disk, _) = this.resolve_root_file(&root, dest.as_str())?;
            let lock = this.root_lock(root.id);
            let _guard = lock.lock().expect("root lock poisoned");
            // t[impl files.write.safe-save] — checked under the root lock
            // The precondition, inside the lock: between checking and
            // renaming nobody else may land here, or a safe save is
            // only safe against writers slower than itself.
            if let Some(expect) = &expect {
                let now = this.sync_content_at(&root, &disk)?;
                let holds = match (expect, &now) {
                    (Expect::Absent, None) => true,
                    (Expect::Content(want), Some(have)) => want == have,
                    _ => false,
                };
                if !holds {
                    return Ok(Landing::Stale(now));
                }
            }
            if disk.exists() && !replace {
                return Ok(Landing::Occupied);
            }
            if let Some(parent) = disk.parent() {
                std::fs::create_dir_all(parent)?;
            }
            // Rename is the atomic case and the usual one. A staging
            // directory on another filesystem falls back to copying
            // into the destination's *own* directory and renaming from
            // there, so the tree still never sees a half-written file
            // under the destination's name.
            if std::fs::rename(&staged, &disk).is_err() {
                let parent = disk.parent().unwrap_or(&disk);
                let tmp = tempfile::NamedTempFile::new_in(parent)?;
                std::fs::copy(&staged, tmp.path())?;
                tmp.persist(&disk).map_err(|e| Error::Io(e.error))?;
                let _ = std::fs::remove_file(&staged);
            }
            // The landed file's own address, read back from where it now
            // sits — not the staging file's. Placement decides the
            // address (a whole-file link and a chunked copy of the same
            // bytes differ), and the etag a caller hands back as
            // `Expect::Content` must be the one this lock will compute.
            let content = this.sync_content_at(&root, &disk)?;
            Ok(Landing::Landed(content))
        })
        .await
    }

    /// Certify the root's current live tree as a checkpoint.
    ///
    /// Used twice by `complete`: once *before* a replace, so the
    /// outgoing content becomes a version rather than a casualty, and
    /// once after, so the landed file is itself recoverable.
    async fn checkpoint(&self, root_id: RootId, why: String) -> Result<(), FilesFault> {
        self.checkpoint_now(root_id.get(), Some(why))
            .await
            .map(|_| ())
            .map_err(fault)
    }
}

// ── The lane ───────────────────────────────────────────────────────

impl UploadService for FilesBackend {
    // t[impl files.write.upload] — dedup is decided here, before any transfer
    async fn begin(&self, spec: UploadSpec) -> Result<UploadPlan, FilesFault> {
        let root = crate::lane::root_or_fault(self, spec.root_id)?;
        // Re-validate: `RootPath` is transparent on the wire, so a
        // hostile peer's `..` arrives having never seen `parse`.
        let path = spec.path.validate()?;
        if path.is_root() {
            return Err(FilesFault::invalid(
                "an upload needs a destination path, not the root itself",
            ));
        }
        may_upload(self, spec.root_id, &path).await?;

        let spec = UploadSpec {
            path: path.clone(),
            ..spec
        };
        let needed = self.outstanding(&root, &spec, &[]).await?;

        // The occupant is reported, never acted on. `complete` carries
        // the choice, so a client may finish transferring while a human
        // is still deciding — which is why this is a field on the plan
        // rather than a fault out of `begin`.
        let conflict = match self.occupant(&root, &path).await? {
            Some(existing) => Some(Conflict {
                keep_both_as: self.keep_both_name(&root, &path).await?,
                existing,
            }),
            None => None,
        };

        let upload_id = UploadId::generate();
        let expires_at = Utc::now() + Duration::hours(SESSION_TTL_HOURS);
        with_uploads(|u| {
            u.0.insert(
                upload_id,
                Session {
                    spec: spec.clone(),
                    expires_at,
                    received: Vec::new(),
                    owner: crate::lane::caller::principal(),
                },
            );
        });

        Ok(UploadPlan {
            upload_id,
            needed,
            lane: BYTE_LANE.to_string(),
            conflict,
            expires_at,
        })
    }

    // t[impl files.write.upload] — outstanding ranges, read from the store
    async fn progress(&self, upload_id: UploadId) -> Result<UploadProgress, FilesFault> {
        let session = session_of(upload_id)?;
        let root = crate::lane::root_or_fault(self, session.spec.root_id)?;
        // Re-derived, not remembered: chunks that arrived by any route
        // since the plan — another upload, a sync pull — count, so
        // resuming never re-sends what the server already has.
        let needed = self
            .outstanding(&root, &session.spec, &session.received)
            .await?;
        Ok(progress_of(upload_id, &session.spec, needed))
    }

    // t[impl files.write.upload] — the collision asks; replace keeps the old
    async fn complete(
        &self,
        upload_id: UploadId,
        on_conflict: OnConflict,
    ) -> Result<CatalogueEntry, FilesFault> {
        let session = session_of(upload_id)?;
        let staged_whole = session.staged_whole() && session.spec.size > 0;
        let spec = session.spec.clone();
        let root_id = spec.root_id;
        let root = crate::lane::root_or_fault(self, root_id)?;
        let path = spec.path.validate()?;
        may_upload(self, root_id, &path).await?;

        // An expectation is about the path the caller named. Keep-both
        // lands somewhere else and keep-existing lands nowhere, so
        // pairing either with one asks two contradictory things.
        if spec.expect.is_some()
            && matches!(on_conflict, OnConflict::KeepBoth | OnConflict::KeepExisting)
        {
            return Err(FilesFault::invalid(
                "an expectation about the destination needs `Fail` or `Replace`",
            ));
        }

        let needed = self.outstanding(&root, &spec, &session.received).await?;
        if !needed.is_empty() {
            // Not a silent truncation: bytes are still owed. The session
            // stays open, so the client sends what `needed` names and
            // tries again.
            return Err(FilesFault::invalid(format!(
                "{path}: {} bytes still outstanding — send them with `send_bytes` first",
                outstanding_bytes(&needed)
            )));
        }

        let occupant = self.occupant(&root, &path).await?;
        let dest = match (&occupant, on_conflict) {
            (None, _) => path.clone(),
            // The session survives a refusal on purpose: `Fail` is a
            // question bounced back to a human, and making them re-upload
            // to answer it would be the coercion this lane forbids.
            (Some(_), OnConflict::Fail) => return Err(FilesFault::Exists { path }),
            (Some(existing), OnConflict::KeepExisting) => {
                let existing = existing.clone();
                forget(upload_id);
                return Ok(existing);
            }
            (Some(_), OnConflict::KeepBoth) => self.keep_both_name(&root, &path).await?,
            (Some(_), OnConflict::Replace) => path.clone(),
        };
        let replacing = occupant.is_some() && on_conflict == OnConflict::Replace;

        // Bytes that came over the wire are already staged, whole; bytes
        // the store already held are materialised out of it now.
        let staged = if staged_whole {
            self.staging_path(upload_id)
        } else {
            self.stage(upload_id, root_id, &spec).await?
        };
        let landed = self
            .finish(&root, &dest, staged.clone(), replacing, &spec)
            .await;
        if landed.is_err() {
            // Nothing partial is left anywhere: the staging file lives
            // outside the tree and goes with the failure. The session
            // forgets what arrived with it, so its next answer comes
            // from the store — which, for a media root, already holds
            // every chunk, and a retry transfers nothing.
            let _ = std::fs::remove_file(&staged);
            with_uploads(|u| {
                if let Some(s) = u.0.get_mut(&upload_id) {
                    s.received.clear();
                }
            });
        }
        let entry = landed?;
        forget(upload_id);
        // An upload is a write, and the catalogue hears about writes from
        // nobody else — see `lane::tree::note_write`.
        if let Ok(root) = crate::lane::root_or_fault(self, spec.root_id) {
            crate::lane::tree::note_write(self, &root, std::slice::from_ref(&entry.path), &[]);
        }
        crate::lane::events::publish(
            self,
            files_proto::service::FilesEvent::Upload(
                files_proto::service::upload::UploadEvent::Completed(entry.clone()),
            ),
        );
        Ok(entry)
    }

    // t[impl files.write.upload] — an abandoned upload leaves nothing behind
    async fn abort(&self, upload_id: UploadId) -> Result<(), FilesFault> {
        session_of(upload_id)?;
        forget(upload_id);
        // There is deliberately nothing else to undo. Content only ever
        // reaches the live tree inside `complete`, so aborting is the
        // absence of an action rather than the reversal of one. Chunks
        // a transfer had already delivered stay in the CAS unreferenced
        // — which is what makes the next attempt cheap, and what the
        // store's own GC collects.
        Ok(())
    }

    async fn pending(&self) -> Result<Vec<UploadProgress>, FilesFault> {
        // The caller's open sessions, whichever device began them — the
        // session is keyed to the person, not the connection, which is
        // what lets a phone finish what a laptop began. Sessions are
        // still process-lifetime (see the module doc).
        let me = crate::lane::caller::principal();
        let open = with_uploads(|u| {
            let mut v: Vec<_> =
                u.0.iter()
                    .filter(|(_, s)| s.owner == me)
                    .map(|(id, s)| (*id, s.clone()))
                    .collect();
            v.sort_by_key(|(id, _)| *id);
            v
        });

        let mut out = Vec::with_capacity(open.len());
        for (id, session) in open {
            // A root released while an upload was open cannot be
            // resumed and is not reported as if it could be. It expires
            // on its own rather than being deleted here, so a released
            // root that is re-adopted mid-session still finds it.
            let Ok(root) = crate::lane::root_or_fault(self, session.spec.root_id) else {
                continue;
            };
            let needed = self
                .outstanding(&root, &session.spec, &session.received)
                .await?;
            out.push(progress_of(id, &session.spec, needed));
        }
        Ok(out)
    }
    // t[impl files.write.upload] — bytes arrive over the same transport
    // t[impl files.scale.transport] — ingress rides vox, with vox's credit
    async fn send_bytes(
        &self,
        upload_id: UploadId,
        mut frames: architect::vox::Rx<UploadFrame>,
    ) -> Result<Received, FilesFault> {
        use tokio::io::{AsyncSeekExt as _, AsyncWriteExt as _};

        let session = session_of(upload_id)?;
        let spec = session.spec.clone();
        let root = crate::lane::root_or_fault(self, spec.root_id)?;
        may_upload(self, spec.root_id, &spec.path).await?;

        // Writing into the staging file at an offset is what makes a
        // resumed upload cheap: the client sends the ranges still
        // outstanding and nothing else.
        let staged = self.staging_path(upload_id);
        tokio::fs::create_dir_all(staged.parent().expect("uploads dir"))
            .await
            .map_err(FilesFault::io)?;
        let file = tokio::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&staged)
            .await
            .map_err(FilesFault::io)?;
        // Size it up front so a client may send its ranges in any order:
        // seeking past the end of a shorter file would otherwise decide
        // the order for them.
        file.set_len(spec.size).await.map_err(FilesFault::io)?;
        let mut file = file;

        let mut written = 0u64;
        let mut arrived: Vec<ChunkRange> = Vec::new();
        // Each `recv` awaits, so the channel's credit is what paces the
        // sender: a client faster than this disk waits on us rather than
        // filling our memory. That is the whole reason a 244 GB upload is
        // safe here and a `Vec<u8>` parameter would not be.
        while let Ok(Some(frame)) = frames.recv().await {
            let mut copied = None;
            let _ = frame.map(|f| copied = Some(f));
            match copied {
                Some(UploadFrame::Chunk { offset, bytes }) => {
                    if offset + bytes.len() as u64 > spec.size {
                        // Refuse rather than grow the file past what the
                        // spec declared: the size is what the collision
                        // and dedup decisions were made against.
                        return Err(FilesFault::invalid(
                            "an upload frame lies outside the declared size",
                        ));
                    }
                    file.seek(std::io::SeekFrom::Start(offset))
                        .await
                        .map_err(FilesFault::io)?;
                    file.write_all(&bytes).await.map_err(FilesFault::io)?;
                    written += bytes.len() as u64;
                    merge_range(&mut arrived, offset, offset + bytes.len() as u64);
                }
                Some(UploadFrame::Finished) => break,
                None => break,
            }
        }

        // Durable before we report progress: a client told its bytes
        // landed and then losing them to a crash would resume from a
        // position that never existed.
        file.flush().await.map_err(FilesFault::io)?;
        file.sync_all().await.map_err(FilesFault::io)?;
        drop(file);

        // Only now, with the bytes durable, does the session count them.
        let received = with_uploads(|u| {
            u.0.get_mut(&upload_id).map(|s| {
                for r in &arrived {
                    merge_range(&mut s.received, r.start, r.end);
                }
                s.received.clone()
            })
        })
        .ok_or(FilesFault::UploadNotFound(upload_id))?;
        let holes = missing(&received, spec.size);
        if !holes.is_empty() {
            // Part-way. Nothing reaches the store yet: the staging file
            // still has holes, and ingesting it would record zeros as
            // content under an address the client never sent.
            return Ok(Received {
                upload_id,
                written,
                needed: holes,
            });
        }

        // Whole. Chunk the staged file into the root's store — this is
        // what makes the bytes *held* rather than merely written, and
        // where the upload dedups against everything already there: an
        // identical chunk costs nothing to add.
        //
        // A software root has no chunk store — its history is git's — so
        // its staging file is simply whole, and lands as it is.
        if root.flavor != RootFlavor::Media {
            return Ok(Received {
                upload_id,
                written,
                needed: Vec::new(),
            });
        }
        let this = self.clone();
        let root_id = spec.root_id.get();
        let staged_for_ingest = staged.clone();
        let landed = crate::lane::blocking(move || {
            this.sync_ingest_path(root_id, &staged_for_ingest)
                .map_err(|e| crate::error::Error::BadRequest(e.to_string()))
        })
        .await?;

        // Record it on the session as the content this upload is for.
        //
        // From here the session is indistinguishable from one whose
        // client declared its address up front — which is the point:
        // `stage`, `outstanding` and the dedup check all key off the
        // content address, and none of them should need to know whether
        // the bytes arrived over the wire or were already held.
        let resolved = ContentId::new(landed);
        with_uploads(|u| {
            if let Some(session) = u.0.get_mut(&upload_id) {
                session.spec.content = Some(resolved.clone());
            }
        });
        let spec = UploadSpec {
            content: Some(resolved),
            ..spec
        };

        let needed = self.outstanding(&root, &spec, &received).await?;
        Ok(Received {
            upload_id,
            written,
            needed,
        })
    }
}

impl FilesBackend {
    /// Where an upload's bytes wait, outside the live tree, until
    /// `complete` renames them into it.
    fn staging_path(&self, upload_id: UploadId) -> PathBuf {
        self.data_dir()
            .join("uploads")
            .join(format!("{upload_id}.part"))
    }

    /// The landing half of `complete`, split out so its failure can be
    /// caught in one place and the staging file cleaned up.
    async fn finish(
        &self,
        root: &FileRootInfo,
        dest: &RootPath,
        staged: PathBuf,
        replacing: bool,
        spec: &UploadSpec,
    ) -> Result<CatalogueEntry, FilesFault> {
        let root_id = RootId::new(root.id);
        if replacing {
            // Before, not after: once the rename has happened the old
            // bytes are gone, and a checkpoint taken then would record
            // the replacement as if nothing had been displaced.
            self.checkpoint(root_id, format!("before replacing {dest}"))
                .await?;
        }
        let content = match self
            .land(root, dest, staged, replacing, spec.expect.clone())
            .await?
        {
            Landing::Landed(content) => content,
            Landing::Occupied => return Err(FilesFault::Exists { path: dest.clone() }),
            Landing::Stale(now) => {
                architect_telemetry::wide::set(
                    "files.upload.stale_found",
                    now.map_or_else(|| "nothing".to_string(), |c| c.0),
                );
                return Err(FilesFault::Stale {
                    path: dest.clone(),
                    expected: match &spec.expect {
                        Some(Expect::Content(c)) => c.to_string(),
                        _ => "nothing".to_string(),
                    },
                });
            }
        };
        self.checkpoint(root_id, format!("upload: {dest}")).await?;

        // FUTURE: `spec.modified_at` is not applied to the landed file —
        // setting an mtime needs a `filetime`-style dependency this
        // crate does not carry. The entry reports the filesystem's own
        // mtime rather than the client's claim, so it is honest about
        // what happened; it is not yet faithful to what was asked.
        let _ = spec.modified_at;
        let mut entry = self
            .occupant(root, dest)
            .await?
            .ok_or_else(|| FilesFault::Io(format!("{dest}: landed and then vanished")))?;
        // The etag the caller passes back as `Expect::Content` on its
        // next save — computed under the landing's lock, so it names
        // exactly the bytes this call put there.
        entry.content = content;
        Ok(entry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> RootPath {
        RootPath::parse(s).expect("path")
    }

    #[test]
    fn received_ranges_merge_and_their_holes_are_what_is_missing() {
        let mut have = Vec::new();
        merge_range(&mut have, 10, 20);
        merge_range(&mut have, 0, 5);
        merge_range(&mut have, 18, 30);
        merge_range(&mut have, 5, 5);
        assert_eq!(
            have,
            vec![
                ChunkRange { start: 0, end: 5 },
                ChunkRange { start: 10, end: 30 }
            ]
        );
        assert_eq!(
            missing(&have, 40),
            vec![
                ChunkRange { start: 5, end: 10 },
                ChunkRange { start: 30, end: 40 }
            ]
        );
        merge_range(&mut have, 5, 10);
        merge_range(&mut have, 30, 40);
        assert!(missing(&have, 40).is_empty(), "every byte arrived");
        assert!(missing(&[], 0).is_empty(), "an empty file needs nothing");
    }

    fn spec(size: u64) -> UploadSpec {
        UploadSpec {
            root_id: RootId::generate(),
            path: p("Audio Files/vox.wav"),
            size,
            content: None,
            modified_at: None,
            expect: None,
        }
    }

    // t[verify files.write.upload]
    // t[verify scenario.album.collide] — the resumption half: an upload
    // that dropped mid-transfer asks for the ranges it is missing and
    // transfers only chunks not already held
    #[test]
    fn consecutive_missing_chunks_are_one_gap() {
        // A resumed transfer should ask for a range, not for a chunk at
        // a time — the coalescing is what makes resumption cheap.
        let mut out = Vec::new();
        push_range(&mut out, 0, 10);
        push_range(&mut out, 10, 25);
        push_range(&mut out, 40, 50);
        assert_eq!(
            out,
            vec![
                ChunkRange { start: 0, end: 25 },
                ChunkRange { start: 40, end: 50 }
            ]
        );
    }

    // t[verify files.write.upload]
    #[test]
    fn an_empty_file_needs_no_transfer() {
        assert!(
            whole(0).is_empty(),
            "a zero-length range is not something a client can send"
        );
        assert_eq!(whole(9), vec![ChunkRange { start: 0, end: 9 }]);
    }

    // t[verify files.write.upload]
    #[test]
    fn progress_counts_what_the_store_holds() {
        let spec = spec(1_000);
        let needed = vec![
            ChunkRange {
                start: 100,
                end: 200,
            },
            ChunkRange {
                start: 900,
                end: 1_000,
            },
        ];
        let progress = progress_of(UploadId::generate(), &spec, needed);
        assert_eq!(progress.received, 800);
        assert_eq!(progress.total, 1_000);
        assert_eq!(progress.needed.len(), 2);

        let done = progress_of(UploadId::generate(), &spec, Vec::new());
        assert_eq!(
            done.received, 1_000,
            "an empty needed list means the content is held in full"
        );
    }

    // t[verify files.write.upload]
    #[test]
    fn a_keep_both_name_never_escapes_its_directory() {
        assert_eq!(sibling(&p("mix.wav"), 2).unwrap().as_str(), "mix (2).wav");
        assert_eq!(
            sibling(&p("Audio Files/vox.wav"), 3).unwrap().as_str(),
            "Audio Files/vox (3).wav"
        );
        assert_eq!(
            sibling(&p("stems"), 2).unwrap().as_str(),
            "stems (2)",
            "a name with no extension keeps not having one"
        );
        // The dot belongs to the directory, not the file: splitting on
        // it would relocate the sibling into a directory of its own.
        assert_eq!(
            sibling(&p("a.b/c"), 2).unwrap().as_str(),
            "a.b/c (2)",
            "a dot in a parent is not an extension"
        );
        assert_eq!(
            sibling(&p(".gitignore"), 2).unwrap().as_str(),
            ".gitignore (2)",
            "a leading dot is not an extension either"
        );
    }

    // t[verify files.write.upload]
    #[test]
    fn an_expired_session_is_dropped_rather_than_resumable() {
        // An upload that is never finished must not pin a session
        // record forever, and must not come back to life either.
        let now = Utc::now();
        let mut uploads = Uploads::default();
        let live = UploadId::generate();
        let stale = UploadId::generate();
        uploads.0.insert(
            live,
            Session {
                spec: spec(1),
                expires_at: now + Duration::hours(1),
                received: Vec::new(),
                owner: None,
            },
        );
        uploads.0.insert(
            stale,
            Session {
                spec: spec(1),
                expires_at: now - Duration::seconds(1),
                received: Vec::new(),
                owner: None,
            },
        );

        uploads.sweep(now);
        assert!(uploads.0.contains_key(&live));
        assert!(
            !uploads.0.contains_key(&stale),
            "an expired upload is gone; nothing partial was ever written for it"
        );
    }
}
