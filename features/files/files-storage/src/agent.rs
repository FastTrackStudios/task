//! The storage agent, in-server hosting (glossary "Storage agent": "one
//! protocol, three hostings" — this is the first, "in-process in
//! task-server for its own volumes").
//!
//! An agent does the three things the coordinator cannot, because the
//! coordinator is never the data path (issue #230): it **hosts** a
//! root's live tree (creating the tree and initializing the
//! authoritative version-store repo inside it, per ADR 0001),
//! **measures** the logical bytes that tree references (quota is charged
//! in logical bytes, and only the holder of the authoritative repo can
//! count them), and **replicates** the root's version-store blobs onto a
//! second location.
//!
//! **The agent is where confinement is enforced**, not the coordinator:
//! a directive carries the boundary its work must stay inside
//! ([`ConfinedPath`]), and every path is created through
//! [`files_store::create_confined`] — which refuses to traverse a
//! symlink *before* the first `mkdir`. A coordinator-side check after
//! the fact can only report an escape that already happened, and could
//! never apply to a remote hosting at all (PR #284 review).
//!
//! Everything here is synchronous, driven through `pollster::block_on`
//! wherever it touches the version store / chunk store — jj-lib's futures
//! are not `Send` on every path, so they must never be awaited from
//! inside an `#[architect::rpc]` method's own future. Callers run these
//! on the blocking pool ([`files_store::blocking`]).

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use files_proto::consts::STORE_DIR;
use files_storage_proto::{
    AgentDirective, ConfinedPath, DirectiveKind, DirectiveOutcome, StorageError,
};
use files_store::chunk::{ChunkStore, FileId};
use files_store::version::VersionStoreBackend;
use jj_lib::backend::TreeValue;
use jj_lib::object_id::ObjectId as _;
use jj_lib::repo::{ReadonlyRepo, Repo as _};
use uuid::Uuid;

use crate::error::{Result, io, path as path_err, store};

/// What a local (in-process) agent can be asked to do. The wire protocol
/// ([`files_storage_proto::StorageAgentService`]) is the same contract
/// for remote hostings; this trait is how the coordinator reaches an
/// agent living in its own process without a round trip through vox.
pub trait LocalAgent: Send + Sync + 'static {
    fn id(&self) -> Uuid;
    /// Carry out `directive`, blocking until it is done. Errors are
    /// reported as [`DirectiveOutcome::Failed`] rather than returned —
    /// a failing directive is a placement outcome, not a coordinator
    /// fault.
    fn execute(&self, directive: &AgentDirective) -> DirectiveOutcome;
}

/// What one measurement pass found in a live tree.
#[derive(Debug, Clone, Default)]
pub struct Measured {
    /// Distinct version-store files reachable from the repo's heads.
    pub files: BTreeSet<FileId>,
    /// Their total length — logical bytes, counted once per distinct
    /// file version and NOT discounted for chunk-level dedup (dedup
    /// savings belong to the operator, issue #230).
    pub logical_bytes: u64,
}

/// One live tree's repo slot. The `Mutex` is held across the open, so
/// exactly one open ever happens per store directory however many
/// directives race for it.
type RepoSlot = Arc<Mutex<Option<Arc<ReadonlyRepo>>>>;

/// The in-server agent: speaks for volumes the server itself owns.
///
/// It keeps one repo handle per live tree for the process's lifetime.
/// That is a cache, but it is also a correctness measure: two handles on
/// one version store in a single process is the shape that wedged
/// `files`' own restart test (PR #280 review) — so the open is
/// serialized per store directory rather than merely cached, which a
/// check-then-open would not have achieved (PR #284 review).
pub struct InServerAgent {
    id: Uuid,
    slots: Mutex<HashMap<PathBuf, RepoSlot>>,
}

impl std::fmt::Debug for InServerAgent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InServerAgent")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl InServerAgent {
    #[must_use]
    pub fn new(id: Uuid) -> Self {
        Self {
            id,
            slots: Mutex::new(HashMap::new()),
        }
    }

    /// The per-store-directory slot, created on first sight. Only the
    /// map lock is held here — never across an open.
    fn slot(&self, store_dir: &Path) -> RepoSlot {
        self.slots
            .lock()
            .expect("agent repo slot map poisoned")
            .entry(store_dir.to_path_buf())
            .or_default()
            .clone()
    }

    /// The authoritative repo, reloaded at the current operation head.
    ///
    /// Measurement and replication go through this: the cadence engine
    /// (issue #260) writes checkpoints through its own handle on the same
    /// store, and a stale snapshot would silently under-count them.
    /// Reloading reuses the repo's existing `Store`, so no second backend
    /// — and no second chunk store — is ever opened on the same
    /// directory.
    pub fn repo_at_head(&self, live_tree: &Path) -> Result<Arc<ReadonlyRepo>> {
        let store_dir = live_tree.join(STORE_DIR);
        let slot = self.slot(&store_dir);
        let mut held = slot.lock().expect("agent repo slot poisoned");
        let repo = match held.as_ref() {
            Some(repo) => pollster::block_on(repo.loader().load_at_head()).map_err(store)?,
            None => {
                files_store::version::repo::open_or_init_repo_blocking(&store_dir).map_err(store)?
            }
        };
        *held = Some(repo.clone());
        Ok(repo)
    }

    /// Create the live tree — refusing to leave `target`'s boundary —
    /// and initialize the authoritative repo inside it. Idempotent:
    /// hosting an already-hosted tree reopens it. Returns the path the
    /// agent actually resolved.
    pub fn host_live_tree(&self, target: &ConfinedPath) -> Result<PathBuf> {
        let live_tree = resolve(target)?;
        self.repo_at_head(&live_tree)?;
        Ok(live_tree)
    }

    /// Walk every head of the live tree's repo and total what it
    /// references.
    pub fn measure(&self, live_tree: &Path) -> Result<Measured> {
        let repo = self.repo_at_head(live_tree)?;
        let backend = repo
            .store()
            .backend_impl::<VersionStoreBackend>()
            .ok_or_else(|| {
                StorageError::Io("live tree's repo is not a VersionStoreBackend".into())
            })?;
        let heads: Vec<_> = repo.view().heads().iter().cloned().collect();

        pollster::block_on(async {
            let mut files: BTreeSet<FileId> = BTreeSet::new();
            let mut seen_trees = BTreeSet::new();
            for head in &heads {
                let commit = backend.commit(head).await.map_err(store)?;
                // A conflicted (unresolved) root tree is a divergence the
                // UI resolves (ADR 0001); it carries no single tree to
                // walk, so it contributes nothing to this measurement.
                let Ok(tree_id) = commit.root_tree.clone().into_resolved() else {
                    continue;
                };
                let mut stack = vec![tree_id];
                while let Some(id) = stack.pop() {
                    if !seen_trees.insert(id.clone()) {
                        continue;
                    }
                    let tree = backend.tree(&id).await.map_err(store)?;
                    for entry in tree.entries() {
                        match entry.value() {
                            TreeValue::Tree(sub) => stack.push(sub.clone()),
                            TreeValue::File { id, .. } => {
                                if let Ok(file_id) = FileId::from_hex(&id.hex()) {
                                    files.insert(file_id);
                                }
                            }
                            TreeValue::Symlink(_) | TreeValue::GitSubmodule(_) => {}
                        }
                    }
                }
            }
            let mut logical_bytes = 0u64;
            for file_id in &files {
                logical_bytes = logical_bytes.saturating_add(
                    backend
                        .chunks()
                        .manifest(*file_id)
                        .await
                        .map_err(store)?
                        .total_len(),
                );
            }
            Ok(Measured {
                files,
                logical_bytes,
            })
        })
    }

    /// Copy every version-store blob the live tree references into a
    /// chunk store under `dest`'s boundary. Streaming, chunk at a time,
    /// through an in-memory pipe — a multi-GB file is never buffered
    /// whole, which is the whole point of the CAS substrate's streaming
    /// API.
    ///
    /// Content addressing makes this self-verifying: re-chunking the
    /// same bytes in the destination store must yield the same
    /// [`FileId`], so a silent corruption on the way over fails the copy
    /// rather than producing a plausible replica.
    pub fn replicate(&self, live_tree: &Path, dest: &ConfinedPath) -> Result<(Measured, PathBuf)> {
        let measured = self.measure(live_tree)?;
        let repo = self.repo_at_head(live_tree)?;
        let backend = repo
            .store()
            .backend_impl::<VersionStoreBackend>()
            .ok_or_else(|| {
                StorageError::Io("live tree's repo is not a VersionStoreBackend".into())
            })?;
        let source = backend.chunks().clone();
        let dest_dir = resolve(dest)?;

        let target_dir = dest_dir.clone();
        pollster::block_on(async move {
            let target = ChunkStore::open(&target_dir).await.map_err(store)?;
            for file_id in &measured.files {
                if target.has(*file_id).await {
                    continue; // already replicated — resumable by construction
                }
                copy_file(&source, &target, *file_id).await?;
            }
            target.shutdown().await.map_err(store)?;
            Ok((measured, dest_dir))
        })
    }

    /// Flush every cached repo's chunk store — call before dropping an
    /// agent whose process is about to reopen the same live trees (a
    /// server exit, or a test simulating a restart). Mirrors
    /// `files::FilesBackend::shutdown`, and for the same reason:
    /// iroh-blobs' `FsStore` may hold buffered writes open until this.
    pub async fn shutdown(&self) {
        let slots: Vec<RepoSlot> = self
            .slots
            .lock()
            .expect("agent repo slot map poisoned")
            .values()
            .cloned()
            .collect();
        for slot in slots {
            let repo = slot.lock().expect("agent repo slot poisoned").clone();
            if let Some(repo) = repo
                && let Some(backend) = repo.store().backend_impl::<VersionStoreBackend>()
            {
                let _ = backend.chunks().shutdown().await;
            }
        }
    }
}

/// Turn a directive's [`ConfinedPath`] into a real directory, enforcing
/// the boundary before anything is created.
fn resolve(target: &ConfinedPath) -> Result<PathBuf> {
    let relative = files_store::safe_relative(&target.relative).map_err(path_err)?;
    files_store::create_confined(Path::new(&target.boundary), &relative).map_err(path_err)
}

/// Stream one file from `source` into `target` without ever holding it
/// whole in memory: the source writes chunks into one end of a duplex
/// pipe while the destination's chunker reads the other.
async fn copy_file(source: &ChunkStore, target: &ChunkStore, file_id: FileId) -> Result<()> {
    use tokio::io::AsyncWriteExt as _;

    let (mut writer, reader) = tokio::io::duplex(64 * 1024);
    let pump = async {
        let out = source.read_to(file_id, &mut writer).await;
        // Always close the pipe, success or not — otherwise the reader
        // below waits forever for an EOF that never comes.
        let _ = writer.shutdown().await;
        out
    };
    let (read_result, written) = tokio::join!(pump, target.write_stream(reader));
    read_result.map_err(|e| io("replicate read", e))?;
    let written = written.map_err(|e| io("replicate write", e))?;
    if written != file_id {
        return Err(StorageError::Io(format!(
            "replicated content addressed to {written:?}, expected {file_id:?}"
        )));
    }
    Ok(())
}

impl LocalAgent for InServerAgent {
    fn id(&self) -> Uuid {
        self.id
    }

    fn execute(&self, directive: &AgentDirective) -> DirectiveOutcome {
        fn failed(err: StorageError) -> DirectiveOutcome {
            DirectiveOutcome::Failed {
                reason: err.to_string(),
            }
        }
        match &directive.kind {
            DirectiveKind::HostLiveTree { target, .. } => match self.host_live_tree(target) {
                Ok(path) => match files_store::to_utf8(&path) {
                    Ok(absolute_path) => DirectiveOutcome::Hosted {
                        repo_initialized: true,
                        absolute_path,
                    },
                    Err(e) => failed(path_err(e)),
                },
                Err(e) => failed(e),
            },
            DirectiveKind::MeasureLiveTree { live_tree_path, .. } => {
                match self.measure(Path::new(live_tree_path)) {
                    Ok(m) => DirectiveOutcome::Measured {
                        files: m.files.len() as u64,
                        logical_bytes: m.logical_bytes,
                    },
                    Err(e) => failed(e),
                }
            }
            DirectiveKind::ReplicateBlobs {
                source_path, dest, ..
            } => match self.replicate(Path::new(source_path), dest) {
                Ok((m, dir)) => match files_store::to_utf8(&dir) {
                    Ok(absolute_path) => DirectiveOutcome::Replicated {
                        files_present: m.files.len() as u64,
                        logical_bytes: m.logical_bytes,
                        absolute_path,
                    },
                    Err(e) => failed(path_err(e)),
                },
                Err(e) => failed(e),
            },
        }
    }
}
