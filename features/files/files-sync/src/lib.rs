// architect's rpc derives emit cfg-gated blocks; allow at crate scope
// (same convention as the sibling files crates).
#![allow(unexpected_cfgs)]

//! Replica sync engine (issue #264, spec: "Reconcile = commit graphs +
//! missing chunks ... resumable at chunk level; divergence surfaces per
//! ADR 0001 — the UI resolves; sync never merges content").
//!
//! Two halves:
//!
//! - [`SyncService`] — the wire surface a backend serves for each of
//!   its roots: visible heads, raw structural objects, chunk manifests,
//!   and chunk streaming. Every payload is **verified on receipt** by
//!   the importing store (objects and chunks are content-addressed;
//!   the id IS the checksum), so a peer is never trusted about content.
//! - [`reconcile`] — the pull one backend runs against another's
//!   `SyncService`: walk the remote head's commit closure, import every
//!   missing object, fetch only the chunks the local store lacks
//!   (**that presence check is what makes an interrupted transfer
//!   resumable** — held chunks are never re-sent), then make the remote
//!   head visible. jj's view semantics take it from there: a head
//!   descending from the local line fast-forwards it; concurrent lines
//!   stay sibling heads — Divergent versions, flagged until resolved
//!   through [`files_proto::FilesService::resolve_divergence`].
//!
//! "Edits flow both ways" is two pulls: each side serves and each side
//! reconciles. There is no push — a replica that wants its offline
//! checkpoints on the server is *pulled from*, which keeps every
//! mutation of a store in that store's own process.
//!
//! On "siblings under one change id" (the ticket's phrasing): the
//! store-level representation of concurrent saves here is **sibling
//! visible heads** — jj's own concurrent-writer shape, which its
//! glossary defines divergence over. Each checkpoint commit carries the
//! change id it was captured under; reconcile imports commits verbatim
//! (content-addressed, id-stable) and never rewrites them, so the two
//! sides keep their captured identities and BOTH stay visible — the
//! "nothing is lost" half is structural. A capture-layer scheme that
//! reuses one change id per path-session (making jj's divergent-change
//! machinery light up too) is deliberately out of scope for v1.

use uuid::Uuid;

pub use files::{FilesBackend, MaterializeReport};

/// Run one synchronous [`FilesBackend`] call off the async worker.
///
/// The backend's `sync_*` / import / materialize methods are
/// synchronous and `pollster::block_on` internally (the whole crate's
/// idiom — a sync jj-lib `Backend` under an async surface). Calling one
/// directly from [`reconcile`]'s async task parks the tokio worker
/// inside `block_on` while the RPC futures it is racing still need that
/// worker — a deadlock. Every local backend touch therefore goes
/// through `spawn_blocking`, exactly as `FilesService`'s own `blocking`
/// helper does for the RPC methods.
async fn off_thread<T, F>(f: F) -> Result<T, SyncError>
where
    F: FnOnce() -> Result<T, SyncError> + Send + 'static,
    T: Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(r) => r,
        Err(e) => Err(SyncError::Io(format!("blocking task panicked: {e}"))),
    }
}

/// One streamed chunk: its blake3 hash (hex) and its bytes. The
/// receiver re-hashes and refuses a mismatch — the wire is untrusted.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, facet::Facet)]
#[repr(C)]
pub struct WireChunk {
    pub hash: String,
    pub bytes: Vec<u8>,
}

/// A file's chunk manifest on the wire: `(chunk hash hex, len)` in
/// file order. The importer re-derives the manifest hash and requires
/// it to equal the claimed `FileId`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, facet::Facet)]
#[repr(C)]
pub struct WireManifest {
    pub file_id: String,
    pub chunks: Vec<WireChunkEntry>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, facet::Facet)]
#[repr(C)]
pub struct WireChunkEntry {
    pub hash: String,
    pub len: u64,
}

#[derive(
    Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, facet::Facet, thiserror::Error,
)]
#[repr(u8)]
pub enum SyncError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("io: {0}")]
    Io(String),
}

fn from_files(err: files_proto::FilesError) -> SyncError {
    match err {
        files_proto::FilesError::NotFound(m) => SyncError::NotFound(m),
        files_proto::FilesError::AlreadyExists(m) | files_proto::FilesError::BadRequest(m) => {
            SyncError::BadRequest(m)
        }
        files_proto::FilesError::Io(m) => SyncError::Io(m),
    }
}

/// The per-root sync surface one backend serves to its peers.
#[architect::rpc]
pub trait SyncService {
    /// The root's visible heads, hex, this side's own line first.
    async fn heads(&self, root_id: Uuid) -> Result<Vec<String>, SyncError>;

    /// One structural object's raw bytes (commit / tree / copy-history)
    /// by hex id. Content-addressed: the bytes hash to the id, and the
    /// importer verifies exactly that.
    async fn object(&self, root_id: Uuid, id: String) -> Result<Vec<u8>, SyncError>;

    /// One file's chunk manifest.
    async fn manifest(&self, root_id: Uuid, file_id: String) -> Result<WireManifest, SyncError>;

    /// The named chunks' bytes, in request order. The caller asks only
    /// for chunks it lacks and in bounded batches (see [`CHUNK_BATCH`]),
    /// which is the resumability contract *and* the bounded-memory one:
    /// a re-run after an interrupted transfer requests strictly the
    /// remainder, and no single response holds more than a batch.
    ///
    /// (A `Tx<WireChunk>` streaming variant is the eventual shape over a
    /// real iroh transport; over the in-process memory link a unary
    /// batch is what actually works, and the batch bound keeps memory
    /// flat either way.)
    async fn chunks(&self, root_id: Uuid, hashes: Vec<String>)
    -> Result<Vec<WireChunk>, SyncError>;

    /// A window of one chunk, bao-encoded: the bytes, plus the proof they
    /// belong to that hash.
    ///
    /// [`Self::chunks`] moves whole chunks, which is fine while a chunk
    /// is a chunk and wrong the moment it is a file: the store links
    /// large files whole (a link costs nothing at any size), so their
    /// manifest has a single chunk whose length is the file's. Asking for
    /// that chunk means asking for 800 GB in one response — not merely
    /// unresumable but unable to complete at all.
    ///
    /// So the unit is a range of BLAKE3 chunks. Two properties follow,
    /// and the second is why this is bao-encoded rather than raw bytes:
    ///
    /// - **Resumable.** A receiver asks its own store which ranges it
    ///   lacks and requests from there, so an interrupted transfer moves
    ///   the gap however large the file is.
    /// - **Verified in flight.** BLAKE3 is a Merkle tree, so a range can
    ///   carry the hashes on its path to the root. A receiver rejects a
    ///   corrupt window as it arrives. Verifying only at the end is not a
    ///   smaller version of this on an 800 GB take — it is a day of
    ///   transfer to learn something a proof states immediately.
    ///
    /// `chunks` is clamped to [`WINDOW_CHUNKS`]; a caller asking for more
    /// gets a window rather than an error, because the bound exists to
    /// keep the server's memory flat and so cannot be a request
    /// parameter.
    async fn chunk_ranges(
        &self,
        root_id: Uuid,
        hash: String,
        from_chunk: u64,
        chunks: u64,
    ) -> Result<Vec<u8>, SyncError>;
}

/// How many chunks one [`SyncService::chunks`] request carries. With a
/// ~1 MiB average chunk this bounds a response near a batch's worth of
/// memory while amortizing per-call overhead across many chunks.
pub const CHUNK_BATCH: usize = 16;

/// A BLAKE3 chunk, in bytes — the unit bao ranges are counted in.
///
/// Not the FastCDC chunk the manifest names, which is a different thing
/// with the same word attached to it: one is where the content-defined
/// chunker chose to split, this is the fixed leaf size of the hash tree.
/// A range of *these* is what a verified window is expressed in.
pub const BAO_CHUNK: u64 = 1024;

/// How much one [`SyncService::chunk_ranges`] response carries.
///
/// 1 MiB, matching `ByteRange::MAX_LEN` on the federation relay — the
/// other place in this codebase that moves someone else's bytes a window
/// at a time. One number for both is worth more than a separately
/// optimal one for each: it is the bound a reviewer has to hold in mind
/// when asking whether a transfer can exhaust memory.
///
/// Large enough that per-call overhead is noise against a multi-gigabyte
/// file, small enough that a failed window costs almost nothing to
/// re-request.
pub const WINDOW_CHUNKS: u64 = (1 << 20) / BAO_CHUNK;

/// Above this, a chunk is pulled as verified windows rather than whole.
///
/// Both paths are kept because both are right somewhere. A session
/// folder is thousands of sub-megabyte chunks, and windowing each would
/// multiply the round trips to move the same bytes; one 800 GB blob is
/// one chunk, and asking for it whole cannot work. The threshold is what
/// tells them apart.
pub const WINDOW_ABOVE: u64 = WINDOW_CHUNKS * BAO_CHUNK;

/// Serve the replica lane to admitted peers on a **device's** endpoint.
///
/// This is what makes a client a peer rather than only a puller. A laptop
/// serving this can be dialled by another laptop, and
/// `files.topology.multi-server`'s "where two peers can reach each other,
/// bytes move directly over iroh/QUIC" stops requiring one of those peers
/// to be a server.
///
/// The gate is [`files::peer::device_gate`]: no sessions, no roles, no
/// accounts — which endpoints this device admits, and nothing else. A
/// smaller model than a server's, and the whole model here, because a
/// laptop has nobody to authenticate.
///
/// Only the replica lane is mounted. A device is not an org and must not
/// answer as one: browsing its tree, reading its grants, writing to it are
/// questions for the server that holds the org, and a device answering
/// them would be a second authority nobody registered.
///
/// Runs until the endpoint closes.
#[cfg(feature = "vox")]
pub async fn serve_peer(
    backend: FilesBackend,
    whose: String,
    endpoint: &architect::iroh_link::iroh::Endpoint,
) {
    // The one table a device has. Its gate refuses anything unlisted, so
    // without this it would refuse the lane it exists to serve — which is
    // what it did, loudly, the first time this ran.
    let gate = std::sync::Arc::new(
        files::peer::device_gate(&backend, &whose)
            .permit(sync_service_service_descriptor(), files::peer::REPLICA_PERMITS),
    );
    let router = architect::LayerRouter::new().merge(layer(SyncHost::new(backend)));
    files::peer::serve_over_iroh(endpoint, move |bearer| {
        architect::permissions_gate::PermissionsGate::wrap_shared_with_bearer(
            gate.clone(),
            router.clone(),
            bearer,
        )
    })
    .await;
}

/// [`SyncService`] served straight off a [`FilesBackend`] — the shape
/// both the in-server hosting and the sync daemon (#265) mount.
#[derive(Clone, architect::HasDispatcher)]
pub struct SyncHost {
    backend: FilesBackend,
}

impl std::fmt::Debug for SyncHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyncHost").finish_non_exhaustive()
    }
}

impl SyncHost {
    #[must_use]
    pub fn new(backend: FilesBackend) -> Self {
        Self { backend }
    }
}

// The serve side runs every backend call through `off_thread` for the
// same reason the pull side does: `sync_*` are synchronous and
// `pollster::block_on` inside, so calling one on the RPC handler's
// async worker parks it. Over an in-process `LocalServer` the client
// and server share one runtime, so a blocked server worker also stalls
// the client — the deadlock that wedged the first test run.
impl SyncService for SyncHost {
    async fn heads(&self, root_id: Uuid) -> Result<Vec<String>, SyncError> {
        let b = self.backend.clone();
        off_thread(move || b.sync_heads(root_id).map_err(from_files)).await
    }

    async fn object(&self, root_id: Uuid, id: String) -> Result<Vec<u8>, SyncError> {
        let b = self.backend.clone();
        off_thread(move || b.sync_object(root_id, &id).map_err(from_files)).await
    }

    async fn manifest(&self, root_id: Uuid, file_id: String) -> Result<WireManifest, SyncError> {
        let b = self.backend.clone();
        let key = file_id.clone();
        let chunks = off_thread(move || b.sync_manifest(root_id, &key).map_err(from_files)).await?;
        Ok(WireManifest {
            file_id,
            chunks: chunks
                .into_iter()
                .map(|(hash, len)| WireChunkEntry { hash, len })
                .collect(),
        })
    }

    async fn chunks(
        &self,
        root_id: Uuid,
        hashes: Vec<String>,
    ) -> Result<Vec<WireChunk>, SyncError> {
        let b = self.backend.clone();
        off_thread(move || {
            hashes
                .into_iter()
                .map(|hash| {
                    let bytes = b.sync_read_chunk(root_id, &hash).map_err(from_files)?;
                    Ok(WireChunk { hash, bytes })
                })
                .collect()
        })
        .await
    }

    async fn chunk_ranges(
        &self,
        root_id: Uuid,
        hash: String,
        from_chunk: u64,
        chunks: u64,
    ) -> Result<Vec<u8>, SyncError> {
        // Clamped here rather than trusted: the bound exists to keep this
        // server's memory flat, so it cannot be a request parameter.
        let chunks = chunks.min(WINDOW_CHUNKS);
        let b = self.backend.clone();
        off_thread(move || {
            b.sync_export_ranges(root_id, &hash, from_chunk, chunks)
                .map_err(from_files)
        })
        .await
    }
}

/// Live progress from a running [`reconcile_with_progress`] pull —
/// what the sync daemon (issue #265) turns into per-file status so a
/// user sees "big.wav: 340/512 chunks" rather than a bare "syncing".
/// Callbacks fire from the reconcile task; keep them cheap
/// (a lock + map update), never blocking.
pub trait SyncObserver: Send + Sync {
    /// The pull began scanning `root_id`'s remote heads.
    fn scan_started(&self, root_id: Uuid) {
        let _ = root_id;
    }
    /// A file's content transfer began — `total_chunks` to move,
    /// `resident_chunks` already local (a resumed transfer starts
    /// partway), `logical_bytes` the file's full size.
    fn file_started(
        &self,
        root_id: Uuid,
        path: &str,
        total_chunks: usize,
        resident_chunks: usize,
        logical_bytes: u64,
    ) {
        let _ = (root_id, path, total_chunks, resident_chunks, logical_bytes);
    }
    /// `chunks_done` of the file's chunks are now local (fetched or
    /// already-resident), carrying `bytes_done` logical bytes.
    fn file_progress(&self, root_id: Uuid, path: &str, chunks_done: usize, bytes_done: u64) {
        let _ = (root_id, path, chunks_done, bytes_done);
    }
    /// A file's content is fully local.
    fn file_finished(&self, root_id: Uuid, path: &str) {
        let _ = (root_id, path);
    }
    /// The pull finished (or failed — `error` set).
    fn pull_finished(&self, root_id: Uuid, error: Option<&str>) {
        let _ = (root_id, error);
    }
}

/// A [`SyncObserver`] that does nothing — the default for a plain
/// [`reconcile`] with no daemon watching.
pub struct NoObserver;
impl SyncObserver for NoObserver {}

/// What one [`reconcile`] pull did.
#[derive(Debug, Default, Clone)]
pub struct ReconcileReport {
    /// Remote heads that were new to this store and are now visible.
    pub heads_imported: u32,
    /// Structural objects (commits/trees/copy records) imported.
    pub objects_imported: u32,
    /// File manifests imported.
    pub manifests_imported: u32,
    /// Chunks actually transferred this pull.
    pub chunks_fetched: u32,
    /// Chunks the manifest referenced that this store already held —
    /// the resumability counter: a resumed transfer's fetched+skipped
    /// always sums to the manifest total, with skipped covering
    /// everything the interrupted run had landed.
    pub chunks_skipped: u32,
    /// The live-tree materialization that followed the import.
    pub materialized: MaterializeReport,
}

/// Pull everything `remote` has for `root_id` that `local` lacks, make
/// the remote heads visible, and materialize the live tree (hydration-
/// policy aware). Safe to re-run at any time — every step is
/// presence-checked first, which is also what makes an interrupted
/// pull resumable at chunk level.
pub async fn reconcile(
    local: &FilesBackend,
    remote: &SyncServiceClient,
    root_id: Uuid,
) -> Result<ReconcileReport, SyncError> {
    reconcile_with_progress(local, remote, root_id, &NoObserver).await
}

/// How much of a root to pull.
///
/// `files.peering.replication` in one type: structure converges across
/// every host, content follows placement. The commit graph *is* the
/// structure — commits, trees and manifests say what exists, how big it
/// is and what it hashes to — so a host can hold a complete and correct
/// account of a 244 GB project for the size of its metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    /// Commits, trees and manifests. No chunks, and no working copy
    /// written — a host with no tree has nowhere to write one, and a
    /// host that wanted the bytes would have asked for them.
    ///
    /// Manifests are deliberately *in*: they carry each file's size and
    /// chunk hashes, which is what lets this host answer "how big is
    /// this" and later tell which chunks it would need. Stopping short
    /// of them would leave a structure that cannot describe itself.
    Structure,
    /// Everything, and materialise the head. What a replica holding the
    /// project pulls.
    Content,
}

impl Depth {
    const fn wants_chunks(self) -> bool {
        matches!(self, Self::Content)
    }
}

/// Pull a root's structure and none of its bytes.
///
/// The receiving half of `files.peering.replication`, paired with
/// [`files_proto::service::roots::RootsService::host_structure`] which
/// gives the root a presence here first.
pub async fn reconcile_structure(
    local: &FilesBackend,
    remote: &SyncServiceClient,
    root_id: Uuid,
) -> Result<ReconcileReport, SyncError> {
    reconcile_at(local, remote, root_id, Depth::Structure, &NoObserver).await
}

/// [`reconcile`] with live per-file progress reported to `observer` —
/// what the sync daemon (issue #265) drives so its status surface can
/// show each file's chunk progress instead of one opaque "syncing".
pub async fn reconcile_with_progress(
    local: &FilesBackend,
    remote: &SyncServiceClient,
    root_id: Uuid,
    observer: &dyn SyncObserver,
) -> Result<ReconcileReport, SyncError> {
    reconcile_at(local, remote, root_id, Depth::Content, observer).await
}

/// [`reconcile_with_progress`] at an explicit [`Depth`].
pub async fn reconcile_at(
    local: &FilesBackend,
    remote: &SyncServiceClient,
    root_id: Uuid,
    depth: Depth,
    observer: &dyn SyncObserver,
) -> Result<ReconcileReport, SyncError> {
    observer.scan_started(root_id);
    let result = reconcile_inner(local, remote, root_id, depth, observer).await;
    observer.pull_finished(
        root_id,
        result.as_ref().err().map(|e| e.to_string()).as_deref(),
    );
    result
}

async fn reconcile_inner(
    local: &FilesBackend,
    remote: &SyncServiceClient,
    root_id: Uuid,
    depth: Depth,
    observer: &dyn SyncObserver,
) -> Result<ReconcileReport, SyncError> {
    let mut report = ReconcileReport::default();
    let remote_heads = remote
        .heads(root_id)
        .await
        .map_err(|e| SyncError::Io(format!("heads rpc: {e}")))?;

    for head in remote_heads {
        let known = {
            let (l, h) = (local.clone(), head.clone());
            off_thread(move || l.sync_has_object(root_id, &h).map_err(from_files)).await?
        };
        if !known {
            import_commit_closure(local, remote, root_id, &head, depth, &mut report, observer)
                .await?;
            report.heads_imported += 1;
        }
        // Always (re)assert visibility: a previous pull may have
        // imported the objects and been interrupted before the view
        // update.
        let (l, h) = (local.clone(), head.clone());
        off_thread(move || l.import_remote_head(root_id, &h).map_err(from_files)).await?;
    }

    // A structure host has no working copy to write, and writing one
    // would be worse than useless: every file would materialise as a
    // stub for content it never asked for.
    if depth.wants_chunks() {
        let l = local.clone();
        report.materialized =
            off_thread(move || l.materialize_head(root_id).map_err(from_files)).await?;
    }
    Ok(report)
}

/// Depth-first import of one commit's full closure: ancestors, trees,
/// copy records, manifests, chunks. Every object is presence-checked
/// before any fetch, so a re-run transfers only what is missing.
async fn import_commit_closure(
    local: &FilesBackend,
    remote: &SyncServiceClient,
    root_id: Uuid,
    head: &str,
    depth: Depth,
    report: &mut ReconcileReport,
    observer: &dyn SyncObserver,
) -> Result<(), SyncError> {
    // Post-order: a commit's object is imported only after its tree
    // closure and every parent are present, so a commit's presence in
    // the store means its WHOLE closure is present — which is what
    // makes an interrupted pull re-runnable (PR #291 review). The
    // fetched-but-not-yet-imported commit bytes are cached between a
    // node's two stack visits so the wire is hit once per commit.
    let mut cached: std::collections::HashMap<String, Vec<u8>> = std::collections::HashMap::new();
    let mut stack: Vec<(String, bool)> = vec![(head.to_string(), false)];
    while let Some((commit_hex, closure_done)) = stack.pop() {
        if closure_done {
            // Trees + parents are in; the commit object lands last.
            if let Some(bytes) = cached.remove(&commit_hex) {
                let (l, c, b) = (local.clone(), commit_hex.clone(), bytes);
                off_thread(move || l.sync_import_object(root_id, &c, b).map_err(from_files))
                    .await?;
                report.objects_imported += 1;
            }
            continue;
        }
        let known = {
            let (l, c) = (local.clone(), commit_hex.clone());
            off_thread(move || l.sync_has_object(root_id, &c).map_err(from_files)).await?
        };
        if known || cached.contains_key(&commit_hex) {
            continue;
        }
        let bytes = remote
            .object(root_id, commit_hex.clone())
            .await
            .map_err(|e| SyncError::Io(format!("object rpc: {e}")))?;
        let (parents, tree) = {
            let (l, b) = (local.clone(), bytes.clone());
            off_thread(move || l.sync_decode_commit(&b).map_err(from_files)).await?
        };
        import_tree_closure(local, remote, root_id, &tree, "", depth, report, observer).await?;
        cached.insert(commit_hex.clone(), bytes);
        // Revisit this commit to import its object after its parents.
        stack.push((commit_hex, true));
        for parent in parents {
            stack.push((parent, false));
        }
    }
    Ok(())
}

async fn import_tree_closure(
    local: &FilesBackend,
    remote: &SyncServiceClient,
    root_id: Uuid,
    tree_hex: &str,
    prefix: &str,
    depth: Depth,
    report: &mut ReconcileReport,
    observer: &dyn SyncObserver,
) -> Result<(), SyncError> {
    // `(tree id, root-relative dir prefix)` so each file's full path is
    // known for progress reporting.
    let mut pending: Vec<(String, String)> = vec![(tree_hex.to_string(), prefix.to_string())];
    while let Some((tree, dir)) = pending.pop() {
        let had = {
            let (l, t) = (local.clone(), tree.clone());
            off_thread(move || l.sync_has_object(root_id, &t).map_err(from_files)).await?
        };
        if !had {
            fetch_object(local, remote, root_id, &tree, report).await?;
        }
        let meta = {
            let (l, t) = (local.clone(), tree.clone());
            off_thread(move || l.sync_tree_meta(root_id, &t).map_err(from_files)).await?
        };
        for (name, subtree) in meta.subtrees {
            let child_dir = if dir.is_empty() {
                name
            } else {
                format!("{dir}/{name}")
            };
            pending.push((subtree, child_dir));
        }
        for (name, file_id, copy_id) in meta.files {
            if let Some(copy) = copy_id {
                let (l, c) = (local.clone(), copy.clone());
                let has =
                    off_thread(move || l.sync_has_object(root_id, &c).map_err(from_files)).await?;
                if !has {
                    fetch_object(local, remote, root_id, &copy, report).await?;
                }
            }
            let path = if dir.is_empty() {
                name
            } else {
                format!("{dir}/{name}")
            };
            import_file(
                local, remote, root_id, &file_id, &path, depth, report, observer,
            )
            .await?;
        }
    }
    Ok(())
}

async fn fetch_object(
    local: &FilesBackend,
    remote: &SyncServiceClient,
    root_id: Uuid,
    id: &str,
    report: &mut ReconcileReport,
) -> Result<(), SyncError> {
    let bytes = remote
        .object(root_id, id.to_string())
        .await
        .map_err(|e| SyncError::Io(format!("object rpc: {e}")))?;
    let (l, id) = (local.clone(), id.to_string());
    off_thread(move || {
        l.sync_import_object(root_id, &id, bytes)
            .map_err(from_files)
    })
    .await?;
    report.objects_imported += 1;
    Ok(())
}

/// Import one file's content: manifest + only the chunks the local
/// store lacks. The chunk-presence pass is the resumability seam - a
/// transfer interrupted after N chunks re-runs as (total - N) fetches.
async fn import_file(
    local: &FilesBackend,
    remote: &SyncServiceClient,
    root_id: Uuid,
    file_id: &str,
    path: &str,
    depth: Depth,
    report: &mut ReconcileReport,
    observer: &dyn SyncObserver,
) -> Result<(), SyncError> {
    let have = {
        let (l, f) = (local.clone(), file_id.to_string());
        off_thread(move || l.sync_has_manifest(root_id, &f).map_err(from_files)).await?
    };
    if have {
        return Ok(());
    }
    let manifest = remote
        .manifest(root_id, file_id.to_string())
        .await
        .map_err(|e| SyncError::Io(format!("manifest rpc: {e}")))?;

    let total_chunks = manifest.chunks.len();
    let logical_bytes: u64 = manifest.chunks.iter().map(|c| c.len).sum();
    let chunk_len: std::collections::HashMap<&str, u64> = manifest
        .chunks
        .iter()
        .map(|c| (c.hash.as_str(), c.len))
        .collect();

    // Accounting is per manifest ENTRY, not per unique chunk hash — a
    // manifest may repeat a hash (silence/padding dedups to one stored
    // chunk), and the report's `fetched + skipped == total` invariant
    // is entry-based (PR #292 review). `missing` is the set of UNIQUE
    // hashes to actually pull; `entries_for` maps each to how many
    // entries it satisfies, so importing it advances progress by that
    // many.
    //
    // A structure pull skips this entirely: it wants none of them, and
    // the presence check is one round trip per entry — which on a 48-file,
    // 244 GB project is the difference between hosting an org for the
    // size of its metadata and paying to be told what it does not want.
    let mut missing: Vec<String> = Vec::new();
    let mut resident = 0usize;
    let mut entries_for: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    for entry in &manifest.chunks {
        if !depth.wants_chunks() {
            report.chunks_skipped += 1;
            continue;
        }
        let (l, h) = (local.clone(), entry.hash.clone());
        let present = off_thread(move || l.sync_has_chunk(root_id, &h).map_err(from_files)).await?;
        if present {
            report.chunks_skipped += 1;
            resident += 1;
        } else {
            *entries_for.entry(entry.hash.clone()).or_default() += 1;
            if !missing.contains(&entry.hash) {
                missing.push(entry.hash.clone());
            }
        }
    }

    // Per-file progress (issue #265): the resumed transfer starts at
    // `resident` entries, the bytes already local.
    let mut done = resident;
    let mut bytes_done: u64 = manifest
        .chunks
        .iter()
        .filter(|c| !entries_for.contains_key(&c.hash))
        .map(|c| c.len)
        .sum();
    observer.file_started(root_id, path, total_chunks, resident, logical_bytes);
    observer.file_progress(root_id, path, done, bytes_done);

    // Hold GC quiescent from the first imported chunk through the
    // manifest write: a synced chunk has no manifest protecting it
    // yet, so without this a sweep firing mid-import destroys it (PR
    // #291 review). Held only while chunks actually need importing —
    // a fully-present file (all chunks skipped) writes just its
    // manifest, which protects them the instant it lands.
    let _quiesce = if missing.is_empty() {
        None
    } else {
        let l = local.clone();
        Some(off_thread(move || l.sync_gc_quiesce(root_id).map_err(from_files)).await?)
    };

    // Large chunks are pulled as windows, small ones in batches. Both
    // paths exist because both are right somewhere — see [`WINDOW_ABOVE`].
    let (large, small): (Vec<String>, Vec<String>) = missing
        .iter()
        .cloned()
        .partition(|h| chunk_len.get(h.as_str()).copied().unwrap_or(0) > WINDOW_ABOVE);

    // The window path. Every large chunk resumes from the first range
    // this store lacks, so an interrupted 800 GB transfer asks for the
    // gap and nothing else — and every window arrives with the proof it
    // belongs to the hash, so a corrupt one is refused where it lands
    // rather than after the file.
    for hash in &large {
        let len = chunk_len.get(hash.as_str()).copied().unwrap_or(0);
        let satisfied = entries_for.get(hash.as_str()).copied().unwrap_or(1);
        let total_chunks = len.div_ceil(BAO_CHUNK);

        loop {
            let from = {
                let (l, h) = (local.clone(), hash.clone());
                off_thread(move || l.sync_missing_from(root_id, &h, len).map_err(from_files))
                    .await?
            };
            // Nothing missing: complete, and verified on the way in.
            let Some(from) = from else { break };

            // Already-held ranges are progress this run did not make;
            // reporting them as if it had would show a resumed transfer
            // starting over.
            bytes_done = bytes_done.max(from * BAO_CHUNK * u64::from(satisfied));
            observer.file_progress(root_id, path, done, bytes_done);

            let want = (total_chunks - from).min(WINDOW_CHUNKS);
            let bao = remote
                .chunk_ranges(root_id, hash.clone(), from, want)
                .await
                .map_err(|e| SyncError::Io(format!("chunk_ranges rpc: {e}")))?;
            if bao.is_empty() {
                return Err(SyncError::Io(format!(
                    "chunk {hash}: peer sent nothing for chunks {from}..{} of {total_chunks}",
                    from + want
                )));
            }
            let (l, h) = (local.clone(), hash.clone());
            off_thread(move || {
                l.sync_import_ranges(root_id, &h, from, want, bao)
                    .map_err(from_files)
            })
            .await?;
        }

        report.chunks_fetched += satisfied;
        done += satisfied as usize;
        bytes_done = bytes_done.max(len * u64::from(satisfied));
        observer.file_progress(root_id, path, done, bytes_done);
    }

    // The batch path, for chunks small enough that a whole one is a
    // bounded response: the resumability seam (only misses are
    // requested) and the bounded-memory seam (no more than a batch in
    // flight).
    for batch in small.chunks(CHUNK_BATCH) {
        let wire = remote
            .chunks(root_id, batch.to_vec())
            .await
            .map_err(|e| SyncError::Io(format!("chunks rpc: {e}")))?;
        for chunk in wire {
            let len = chunk_len.get(chunk.hash.as_str()).copied().unwrap_or(0);
            // How many manifest entries this one unique chunk satisfies.
            let satisfied = entries_for.get(chunk.hash.as_str()).copied().unwrap_or(1);
            let l = local.clone();
            off_thread(move || {
                l.sync_import_chunk(root_id, &chunk.hash, chunk.bytes)
                    .map_err(from_files)
            })
            .await?;
            report.chunks_fetched += satisfied;
            done += satisfied as usize;
            bytes_done += len * u64::from(satisfied);
            observer.file_progress(root_id, path, done, bytes_done);
        }
    }

    let chunks: Vec<(String, u64)> = manifest
        .chunks
        .iter()
        .map(|c| (c.hash.clone(), c.len))
        .collect();
    let (l, f) = (local.clone(), file_id.to_string());
    off_thread(move || {
        l.sync_import_manifest_at(root_id, &f, chunks, depth.wants_chunks())
            .map_err(from_files)
    })
    .await?;
    // The manifest is durable; its chunks are now protected by it, so
    // the quiesce guard can release.
    drop(_quiesce);
    report.manifests_imported += 1;
    observer.file_finished(root_id, path);
    Ok(())
}
