//! [`VersionStoreBackend`]: the custom jj-lib [`Backend`] called for in
//! ADR 0001 — file content streams through
//! [`task_files_chunk_store::ChunkStore`] (FastCDC/blake3/iroh-blobs);
//! trees, commits, and copy-history records are small structural objects
//! held in a sibling [`ObjectStore`]. jj-lib supplies everything above this
//! trait — op-log concurrency, divergent changes, conflicted-tree merges —
//! for free, as long as this impl is faithful.

use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use futures::AsyncRead;
use futures::stream::{self, BoxStream, StreamExt as _};
use jj_lib::backend::{
    Backend, BackendError, BackendResult, ChangeId, Commit, CommitId, CopyHistory, CopyId,
    CopyRecord, FileId, RelatedCopy, SigningFn, SymlinkId, Tree, TreeId,
};
use jj_lib::index::Index;
use jj_lib::object_id::ObjectId as _;
use jj_lib::repo_path::{RepoPath, RepoPathBuf};
use tokio_util::compat::FuturesAsyncReadCompatExt as _;
use tokio_util::compat::TokioAsyncReadCompatExt as _;

use crate::codec;
use crate::error::{Error, Result};
use crate::objects::ObjectStore;

const COMMIT_ID_LEN: usize = 32;
const CHANGE_ID_LEN: usize = 16;

/// Reproduces a `jj_lib::backend::BackendError` from our own [`Error`].
/// Object-not-found maps onto jj-lib's own `ObjectNotFound` variant (with a
/// generic `"object"` type — use [`not_found`] instead at call sites that
/// know the specific kind); everything else is `Other`.
fn to_backend_err(err: Error) -> BackendError {
    match err {
        Error::UnknownObject(hash) => object_not_found("object", hash),
        other => BackendError::Other(other.into()),
    }
}

/// `BackendError::ObjectNotFound` for a specific object kind ("tree",
/// "commit", "copy") — jj-lib surfaces this string to users and to its own
/// missing-object handling, so callers that know what they were reading
/// (`read_tree`/`read_commit`/`read_copy`) report it precisely rather than
/// through the generic [`to_backend_err`].
fn object_not_found(object_type: &str, hash: String) -> BackendError {
    BackendError::ObjectNotFound {
        object_type: object_type.to_string(),
        hash,
        source: Box::new(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "version-store object not found",
        )),
    }
}

/// Maps `Error::UnknownObject` to a typed [`object_not_found`] and
/// everything else through [`to_backend_err`].
fn not_found(object_type: &'static str) -> impl Fn(Error) -> BackendError {
    move |err| match err {
        Error::UnknownObject(hash) => object_not_found(object_type, hash),
        other => to_backend_err(other),
    }
}

fn chunk_file_id(id: &FileId) -> Result<task_files_chunk_store::FileId> {
    task_files_chunk_store::FileId::from_hex(&id.hex()).map_err(Error::from)
}

fn jj_file_id(id: task_files_chunk_store::FileId) -> FileId {
    FileId::from_bytes(id.as_bytes())
}

/// The jj-lib [`Backend`] over the Files CAS chunk store. See the module doc
/// and ADR 0001 (`apps/task/docs/adr/0001-files-version-store-jj-cas.md`).
pub struct VersionStoreBackend {
    chunks: Arc<task_files_chunk_store::ChunkStore>,
    objects: ObjectStore,
    root_commit_id: CommitId,
    root_change_id: ChangeId,
    empty_tree_id: TreeId,
    /// Captured at [`VersionStoreBackend::open`] so the two *sync* `Backend`
    /// methods (`get_copy_records`, `gc`) — whose implementations bottom
    /// out in `tokio::fs` — have an explicit runtime to drive rather than
    /// relying on `pollster::block_on`'s ambient `Handle::current()`, which
    /// panics with an opaque "there is no reactor running" deep inside a
    /// tokio::fs call when there's no runtime on the calling thread.
    /// `Handle::block_on` still cannot be called from a thread already
    /// inside *this* runtime's own task (jj-lib calling these sync methods
    /// directly from async code, rather than via `spawn_blocking`, will
    /// still panic — that tension is inherent to a sync method needing
    /// async I/O, not something this backend can paper over) — but calling
    /// from any other thread, with or without its own ambient runtime, now
    /// works instead of panicking.
    runtime: tokio::runtime::Handle,
}

impl std::fmt::Debug for VersionStoreBackend {
    // `task_files_chunk_store::ChunkStore` doesn't derive `Debug`; the
    // `Backend` trait requires it, so summarize instead of deriving.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VersionStoreBackend")
            .field("root_commit_id", &self.root_commit_id)
            .field("empty_tree_id", &self.empty_tree_id)
            .finish_non_exhaustive()
    }
}

/// Chunk-level GC interval [`VersionStoreBackend::open`] uses by default —
/// a File Root's backend is server-hosted and long-lived (ADR 0001), so
/// there is no latency pressure on iroh-blobs' own background sweep; a test
/// that needs to observe reclamation within its own runtime should use
/// [`VersionStoreBackend::open_with_gc_interval`] with a much shorter one.
/// Re-exports `task_files_chunk_store::gc::DEFAULT_INTERVAL` rather than
/// hardcoding its own copy, so the two layers' production cadence can't
/// silently diverge.
pub const DEFAULT_GC_INTERVAL: Duration = task_files_chunk_store::gc::DEFAULT_INTERVAL;

impl VersionStoreBackend {
    /// Open (creating if absent) a version store rooted at `root`: a
    /// `chunks/` chunk store (file content, GC-enabled at
    /// [`DEFAULT_GC_INTERVAL`]) beside an `objects/` tree/commit/
    /// copy-history store.
    pub async fn open(root: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_gc_interval(root, DEFAULT_GC_INTERVAL).await
    }

    /// Open with a non-default chunk-level GC interval (see
    /// [`DEFAULT_GC_INTERVAL`]) — the seam tests use to observe iroh-blobs'
    /// background chunk reclamation without a multi-minute wait.
    pub async fn open_with_gc_interval(
        root: impl AsRef<Path>,
        gc_interval: Duration,
    ) -> Result<Self> {
        let root = root.as_ref();
        let chunks = task_files_chunk_store::ChunkStore::open_with_gc(
            root.join("chunks"),
            task_files_chunk_store::ChunkerConfig::default(),
            task_files_chunk_store::GcConfig {
                interval: gc_interval,
            },
        )
        .await?;
        let objects = ObjectStore::open(root.join("objects")).await?;

        let empty_tree_bytes = codec::encode_tree(&Tree::default());
        let empty_tree_hash = objects.write(&empty_tree_bytes).await?;

        Ok(Self {
            chunks: Arc::new(chunks),
            objects,
            root_commit_id: CommitId::new(vec![0u8; COMMIT_ID_LEN]),
            root_change_id: ChangeId::new(vec![0u8; CHANGE_ID_LEN]),
            empty_tree_id: TreeId::from_bytes(empty_tree_hash.as_bytes()),
            runtime: tokio::runtime::Handle::current(),
        })
    }

    /// The path a jj repo's `store/type` file must name to load this
    /// backend via [`jj_lib::backend::BackendInitializer`] /
    /// `StoreFactories::add_backend`.
    pub const NAME: &'static str = "fts-files-cas";

    /// Drives `fut` to completion from a *sync* context — the seam
    /// `get_copy_records`/`gc` need, since both are sync `Backend` methods
    /// whose implementations do `tokio::fs` I/O. Two cases, handled
    /// differently because `tokio::runtime::Handle::block_on` and
    /// `pollster::block_on` fail in opposite circumstances:
    ///
    /// - Called from a thread that already has an ambient tokio runtime
    ///   (including this backend's own — e.g. jj-lib invoking `gc`
    ///   directly from async code, or from inside `spawn_blocking`, or a
    ///   test calling `get_copy_records` inline): `Handle::block_on` would
    ///   panic ("Cannot start a runtime from within a runtime") if that
    ///   ambient runtime happens to be this one, so use `pollster::block_on`
    ///   instead — it's a plain poll loop, not a runtime entry, so
    ///   `tokio::fs`'s internal `Handle::current()` calls resolve against
    ///   whatever runtime is already ambient with no nesting conflict.
    /// - Called from a plain thread with no ambient runtime at all (a
    ///   non-async caller): `pollster::block_on` would panic deep inside
    ///   `tokio::fs` ("there is no reactor running"), so drive the handle
    ///   captured at `open()` instead — `Handle::block_on` from an
    ///   otherwise-bare thread is exactly tokio's supported pattern for
    ///   this.
    fn block_on<F: std::future::Future>(&self, fut: F) -> F::Output {
        if tokio::runtime::Handle::try_current().is_ok() {
            pollster::block_on(fut)
        } else {
            self.runtime.block_on(fut)
        }
    }

    fn object_hash(id: &[u8]) -> Result<blake3::Hash> {
        let bytes: [u8; 32] = id.try_into().map_err(|_| {
            Error::Object(format!(
                "expected 32-byte object id, got {} bytes",
                id.len()
            ))
        })?;
        Ok(blake3::Hash::from_bytes(bytes))
    }

    async fn write_object(&self, bytes: &[u8]) -> Result<blake3::Hash> {
        self.objects.write(bytes).await
    }

    async fn read_object(&self, id: &[u8]) -> Result<Vec<u8>> {
        let hash = Self::object_hash(id)?;
        self.objects.read(&hash).await
    }

    /// Underlying chunk store, for the version-store crate's own helpers
    /// (checkpoint/chain derivation) that need to write file content
    /// directly rather than through jj-lib's `Backend::write_file`.
    pub fn chunks(&self) -> &Arc<task_files_chunk_store::ChunkStore> {
        &self.chunks
    }

    /// One structural object's **raw encoded bytes** (a tree, commit, or
    /// copy-history record) by its id. The replica-sync wire format
    /// (issue #264): objects are content-addressed (id = blake3 of the
    /// bytes), so shipping the bytes verbatim is what guarantees the
    /// receiver derives the identical id — no re-encode, no drift.
    pub async fn read_raw_object(&self, id: &[u8]) -> Result<Vec<u8>> {
        self.read_object(id).await
    }

    /// Decode a raw commit's `(parent ids, root tree id)` **without
    /// storing it** — the seam replica sync (issue #264) needs to
    /// import a commit's whole closure *before* the commit object
    /// itself, so that a commit's presence in the store means its
    /// closure is present too (the same manifest-last durability
    /// invariant the chunk store upholds). Returns each id as raw
    /// bytes.
    pub fn decode_commit_meta(bytes: &[u8]) -> Result<(Vec<Vec<u8>>, Vec<u8>)> {
        let commit = codec::decode_commit(bytes)?;
        let tree = commit
            .root_tree
            .as_resolved()
            .ok_or_else(|| Error::Object("conflicted root tree in synced commit".into()))?
            .as_bytes()
            .to_vec();
        let parents = commit
            .parents
            .iter()
            .map(|p| p.as_bytes().to_vec())
            .collect();
        Ok((parents, tree))
    }

    /// Store one structural object received from a peer, **verified**:
    /// the bytes must hash to `expected_id` — a sync peer is never
    /// trusted about content addresses. Also verifies the bytes decode
    /// as one of the three object kinds, so a peer can't park arbitrary
    /// data in the object store under a valid hash.
    pub async fn import_raw_object(&self, expected_id: &[u8], bytes: Vec<u8>) -> Result<()> {
        let expected = Self::object_hash(expected_id)?;
        let actual = blake3::hash(&bytes);
        if actual != expected {
            return Err(Error::Object(format!(
                "object bytes hash to {actual}, peer claimed {expected}"
            )));
        }
        if codec::decode_commit(&bytes).is_err()
            && codec::decode_tree(&bytes).is_err()
            && codec::decode_copy_history(&bytes).is_err()
        {
            return Err(Error::Object(format!(
                "object {expected} decodes as no known kind — refusing to store it"
            )));
        }
        self.objects.write(&bytes).await?;
        Ok(())
    }

    /// The tree/commit/copy-history object store, for `gc.rs`'s sweep.
    pub(crate) fn objects(&self) -> &ObjectStore {
        &self.objects
    }

    /// `empty_tree_id()` without going through the `Backend` trait, for
    /// `gc.rs`'s sweep — it must pin this tree unconditionally (see that
    /// module's doc on why the root commit's own walk can't be relied on
    /// for it).
    pub(crate) fn empty_tree_id_for_gc(&self) -> &TreeId {
        &self.empty_tree_id
    }

    /// Read one directory level of a tree, exposed for `chain.rs`'s own
    /// diff walk (which needs to recurse without going through the trait
    /// object).
    pub async fn tree(&self, id: &TreeId) -> Result<Tree> {
        let bytes = self.read_object(id.as_bytes()).await?;
        codec::decode_tree(&bytes)
    }

    pub async fn commit(&self, id: &CommitId) -> Result<Commit> {
        if *id == self.root_commit_id {
            return Ok(jj_lib::backend::make_root_commit(
                self.root_change_id.clone(),
                self.empty_tree_id.clone(),
            ));
        }
        let bytes = self.read_object(id.as_bytes()).await?;
        codec::decode_commit(&bytes)
    }

    pub async fn copy_history(&self, id: &CopyId) -> Result<CopyHistory> {
        let bytes = self.read_object(id.as_bytes()).await?;
        codec::decode_copy_history(&bytes)
    }

    /// Establish a fresh copy-history record for a file with no recorded
    /// ancestry (a newly-created path).
    pub async fn write_origin_copy(&self, path: &RepoPath, salt: Vec<u8>) -> Result<CopyId> {
        self.write_copy_history(&CopyHistory {
            current_path: path.to_owned(),
            parents: vec![],
            salt,
        })
        .await
    }

    /// Record `new_path` as a copy/rename descending from `parent`.
    pub async fn write_copy_from(&self, new_path: &RepoPath, parent: CopyId) -> Result<CopyId> {
        self.write_copy_history(&CopyHistory {
            current_path: new_path.to_owned(),
            parents: vec![parent],
            salt: vec![],
        })
        .await
    }

    async fn write_copy_history(&self, history: &CopyHistory) -> Result<CopyId> {
        let bytes = codec::encode_copy_history(history);
        let hash = self.write_object(&bytes).await?;
        let id = CopyId::new(hash.as_bytes().to_vec());
        for parent in &history.parents {
            self.objects
                .append_index_line("copy-children", &parent.hex(), &id.hex())
                .await?;
        }
        Ok(id)
    }

    /// Children of `id` per the `copy-children` index. The index is a hint
    /// written alongside every `write_copy_history` call, not an authority
    /// (see `ObjectStore::append_index_line`'s doc): `gc` can sweep a
    /// child's own copy-history object without knowing (or needing to know)
    /// which parents' index files still name it. So a line naming an object
    /// that no longer exists is skipped here rather than erroring — it's
    /// unreachable by definition once its own object is gone, exactly the
    /// case `get_related_copies` should treat as absent.
    async fn copy_children(&self, id: &CopyId) -> Result<Vec<CopyId>> {
        let lines = self
            .objects
            .read_index_lines("copy-children", &id.hex())
            .await?;
        let mut children = Vec::with_capacity(lines.len());
        for hex in lines {
            let child = CopyId::try_from_hex(&hex)
                .ok_or_else(|| Error::Object(format!("bad copy-children hex {hex:?}")))?;
            match self.copy_history(&child).await {
                Ok(_) => children.push(child),
                Err(Error::UnknownObject(_)) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(children)
    }
}

#[async_trait]
impl Backend for VersionStoreBackend {
    fn name(&self) -> &str {
        Self::NAME
    }

    fn commit_id_length(&self) -> usize {
        COMMIT_ID_LEN
    }

    fn change_id_length(&self) -> usize {
        CHANGE_ID_LEN
    }

    fn root_commit_id(&self) -> &CommitId {
        &self.root_commit_id
    }

    fn root_change_id(&self) -> &ChangeId {
        &self.root_change_id
    }

    fn empty_tree_id(&self) -> &TreeId {
        &self.empty_tree_id
    }

    fn concurrency(&self) -> usize {
        // Server-hosted, not a local single-writer store (ADR 0001).
        8
    }

    async fn read_file(
        &self,
        path: &RepoPath,
        id: &FileId,
    ) -> BackendResult<Pin<Box<dyn AsyncRead + Send>>> {
        let file_id = chunk_file_id(id).map_err(to_backend_err)?;
        let chunks = self.chunks.clone();
        let path = path.to_owned();
        // Stream chunk-by-chunk through a bounded pipe rather than
        // buffering the whole file: `ChunkStore` only exposes a
        // sink-shaped `read_to`, so a background task drives it into the
        // write half and the caller reads the other end. This is what
        // makes reads bounded-memory in both directions, matching
        // `write_file` (which already streams the caller's reader straight
        // into the chunker). `StreamingRead` below is what turns the
        // background task's failure into an `io::Error` on the caller's
        // reader instead of a silently-truncated clean EOF.
        let (reader, mut writer) = tokio::io::duplex(64 * 1024);
        let task = tokio::spawn(async move {
            chunks
                .read_to(file_id, &mut writer)
                .await
                .map_err(|err| format!("streaming read of {path:?} failed: {err}"))
        });
        Ok(Box::pin(StreamingRead {
            reader: reader.compat(),
            task: Some(task),
        }))
    }

    async fn write_file(
        &self,
        _path: &RepoPath,
        contents: &mut (dyn AsyncRead + Send + Unpin),
    ) -> BackendResult<FileId> {
        let file_id = self
            .chunks
            .write_stream(contents.compat())
            .await
            .map_err(Error::from)
            .map_err(to_backend_err)?;
        Ok(jj_file_id(file_id))
    }

    async fn read_symlink(&self, _path: &RepoPath, id: &SymlinkId) -> BackendResult<String> {
        let bytes = self
            .read_object(id.as_bytes())
            .await
            .map_err(to_backend_err)?;
        String::from_utf8(bytes)
            .map_err(|e| BackendError::Other(format!("symlink target not utf-8: {e}").into()))
    }

    async fn write_symlink(&self, _path: &RepoPath, target: &str) -> BackendResult<SymlinkId> {
        let hash = self
            .write_object(target.as_bytes())
            .await
            .map_err(to_backend_err)?;
        Ok(SymlinkId::from_bytes(hash.as_bytes()))
    }

    async fn read_copy(&self, id: &CopyId) -> BackendResult<CopyHistory> {
        self.copy_history(id).await.map_err(not_found("copy"))
    }

    async fn write_copy(&self, contents: &CopyHistory) -> BackendResult<CopyId> {
        self.write_copy_history(contents)
            .await
            .map_err(to_backend_err)
    }

    async fn get_related_copies(&self, copy_id: &CopyId) -> BackendResult<Vec<RelatedCopy>> {
        // Ancestors of `copy_id` (inclusive), plus all descendants of those
        // ancestors — the doc contract on `Backend::get_related_copies`.
        // Over-inclusive is explicitly allowed ("valid but wasteful"), so
        // this simple two-phase closure is correct even though it isn't
        // the tightest possible set.
        let mut ancestors = vec![copy_id.clone()];
        let mut frontier = vec![copy_id.clone()];
        while let Some(id) = frontier.pop() {
            let history = self.copy_history(&id).await.map_err(to_backend_err)?;
            for parent in history.parents {
                if !ancestors.contains(&parent) {
                    ancestors.push(parent.clone());
                    frontier.push(parent);
                }
            }
        }

        // Children-before-parents topological order: repeatedly emit any
        // not-yet-emitted node whose parents (within `related`) are all
        // already emitted... but we want the *reverse* (children first), so
        // instead emit whenever a node still has unemitted children.
        let mut related: Vec<CopyId> = ancestors.clone();
        let mut frontier: Vec<CopyId> = ancestors;
        while let Some(id) = frontier.pop() {
            for child in self.copy_children(&id).await.map_err(to_backend_err)? {
                if !related.contains(&child) {
                    related.push(child.clone());
                    frontier.push(child);
                }
            }
        }

        // related is currently in a mixed discovery order; sort so every
        // child appears before its parents by repeatedly peeling off nodes
        // whose children (within `related`) all precede them.
        let mut histories = Vec::with_capacity(related.len());
        for id in &related {
            let history = self.copy_history(id).await.map_err(to_backend_err)?;
            histories.push((id.clone(), history));
        }
        let depths: Vec<usize> = histories
            .iter()
            .map(|(id, _)| depth_from_roots(id, &histories))
            .collect();
        let mut indexed: Vec<usize> = (0..histories.len()).collect();
        indexed.sort_by_key(|&i| depths[i]);
        indexed.reverse();
        let histories: Vec<_> = indexed.into_iter().map(|i| histories[i].clone()).collect();

        Ok(histories
            .into_iter()
            .map(|(id, history)| RelatedCopy { id, history })
            .collect())
    }

    async fn read_tree(&self, _path: &RepoPath, id: &TreeId) -> BackendResult<Tree> {
        self.tree(id).await.map_err(not_found("tree"))
    }

    async fn write_tree(&self, _path: &RepoPath, contents: &Tree) -> BackendResult<TreeId> {
        let bytes = codec::encode_tree(contents);
        let hash = self.write_object(&bytes).await.map_err(to_backend_err)?;
        Ok(TreeId::from_bytes(hash.as_bytes()))
    }

    async fn read_commit(&self, id: &CommitId) -> BackendResult<Commit> {
        self.commit(id).await.map_err(not_found("commit"))
    }

    async fn write_commit(
        &self,
        mut commit: Commit,
        sign_with: Option<&mut SigningFn>,
    ) -> BackendResult<(CommitId, Commit)> {
        if commit.parents.is_empty() {
            return Err(BackendError::Other(
                "cannot write a commit with no parents".into(),
            ));
        }
        let mut encoded = codec::encode_commit(&commit);
        if let Some(sign) = sign_with {
            let sig = sign(&encoded).map_err(|e| BackendError::Other(e.into()))?;
            commit.secure_sig = Some(jj_lib::backend::SecureSig {
                data: encoded.clone(),
                sig,
            });
            encoded = codec::encode_commit(&commit);
        }
        let hash = self.write_object(&encoded).await.map_err(to_backend_err)?;
        Ok((CommitId::from_bytes(hash.as_bytes()), commit))
    }

    fn get_copy_records(
        &self,
        paths: Option<&[RepoPathBuf]>,
        root: &CommitId,
        head: &CommitId,
    ) -> BackendResult<BoxStream<'_, BackendResult<CopyRecord>>> {
        let paths = paths.map(<[RepoPathBuf]>::to_vec);
        let root = root.clone();
        let head = head.clone();
        let records = self
            .block_on(crate::chain::copy_records_between(
                self,
                paths.as_deref(),
                &root,
                &head,
            ))
            .map_err(to_backend_err)?;
        Ok(stream::iter(records.into_iter().map(Ok)).boxed())
    }

    fn gc(&self, index: &dyn Index, keep_newer: SystemTime) -> BackendResult<()> {
        // `Backend::gc`'s trait signature has no room for a protect
        // callback (see `gc.rs`'s module doc), and this is jj-lib's own
        // generic entry point — reachable by any jj-lib-native caller, not
        // just ones that know about Vault-referenced protection. Rather
        // than sweeping the chunk store with an implicit, always-empty
        // `protected_commits` (which would durably delete manifests for
        // any commit that's Vault-referenced but not currently
        // index-reachable), this calls the structural-only sweep, which
        // never touches the chunk store. The Vault-facing entry point
        // (future RPC work) calls `crate::gc::sweep` directly with its own
        // resolved protected-commit set.
        self.block_on(crate::gc::sweep_objects_only(self, index, keep_newer))
            .map(|_objects_swept| ())
            .map_err(to_backend_err)
    }
}

/// Rough topological depth (distance from a copy-history record with no
/// parents) used to order `get_related_copies`' output children-before-
/// parents. `histories` is the closed set being ordered, so lookups never
/// escape it.
fn depth_from_roots(id: &CopyId, histories: &[(CopyId, CopyHistory)]) -> usize {
    fn depth(id: &CopyId, histories: &[(CopyId, CopyHistory)], seen: &mut Vec<CopyId>) -> usize {
        if seen.contains(id) {
            return 0; // guard against any accidental cycle
        }
        seen.push(id.clone());
        let Some((_, history)) = histories.iter().find(|(cid, _)| cid == id) else {
            return 0;
        };
        history
            .parents
            .iter()
            .map(|parent| depth(parent, histories, seen) + 1)
            .max()
            .unwrap_or(0)
    }
    depth(id, histories, &mut Vec::new())
}

/// Wraps the reader half of `read_file`'s streaming duplex pipe so that a
/// failure in the background task feeding it (see `read_file`) surfaces as
/// an `io::Error` on the caller's reader instead of a silently-truncated
/// clean EOF. Only consulted at true EOF (0 bytes read from the duplex):
/// any bytes already buffered are always returned first.
struct StreamingRead {
    reader: tokio_util::compat::Compat<tokio::io::DuplexStream>,
    task: Option<tokio::task::JoinHandle<std::result::Result<(), String>>>,
}

impl AsyncRead for StreamingRead {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut [u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        use std::future::Future as _;
        use std::task::Poll;

        let this = self.get_mut();
        match Pin::new(&mut this.reader).poll_read(cx, buf) {
            Poll::Ready(Ok(0)) => {
                let Some(task) = this.task.as_mut() else {
                    return Poll::Ready(Ok(0));
                };
                match Pin::new(task).poll(cx) {
                    Poll::Ready(Ok(Ok(()))) => {
                        this.task = None;
                        Poll::Ready(Ok(0))
                    }
                    Poll::Ready(Ok(Err(message))) => {
                        this.task = None;
                        Poll::Ready(Err(std::io::Error::other(message)))
                    }
                    Poll::Ready(Err(join_err)) => {
                        this.task = None;
                        Poll::Ready(Err(std::io::Error::other(join_err)))
                    }
                    Poll::Pending => Poll::Pending,
                }
            }
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression test: `gc` can sweep a copy-history object without
    /// pruning the `copy-children` index entries that name it under its
    /// parent (see `gc.rs`'s doc — the index is a hint, not an authority).
    /// `get_related_copies` must skip a dangling entry rather than
    /// erroring on the resulting `UnknownObject`.
    #[tokio::test]
    async fn get_related_copies_tolerates_a_swept_child_in_the_index() {
        let dir = tempfile::tempdir().unwrap();
        let backend = VersionStoreBackend::open(dir.path()).await.unwrap();

        let path = RepoPath::from_internal_string("a").unwrap();
        let origin = backend
            .write_origin_copy(path, vec![1, 2, 3])
            .await
            .unwrap();

        // A child hex that names no real object — simulating one whose own
        // copy-history object `gc` already removed.
        let swept_child_hex = "deadbeef".repeat(8);
        backend
            .objects
            .append_index_line("copy-children", &origin.hex(), &swept_child_hex)
            .await
            .unwrap();

        let related = Backend::get_related_copies(&backend, &origin)
            .await
            .unwrap();
        assert_eq!(
            related.len(),
            1,
            "the dangling child entry must be skipped, not surfaced as an error: {related:?}"
        );
        assert_eq!(related[0].id, origin);
    }
}
