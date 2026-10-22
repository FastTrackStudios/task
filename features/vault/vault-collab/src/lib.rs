//! Vault-file ⇄ CRDT reconciliation — the server side of per-file
//! collaborative editing.
//!
//! One [`VaultCollab`] per org wires three loops around a
//! [`crdt::DocRegistry`] of per-file Loro docs (doc id =
//! [`vault_proto::collab_doc_id`] of `(vault_id, path)`):
//!
//! 1. **Open/seed** — the registry factory opens each doc over
//!    [`crdt::FilePersistence`] (one dir per doc under the org's
//!    `crdt/` root) and reconciles it with the vault file on disk:
//!    a fresh doc is made from the file (`body` + `props`, via
//!    `vault-doc`); a re-opened doc whose file changed while it was
//!    evicted merges the edit as a change, three-way against the
//!    version the mirror was rendered from (`meta.flushed_at`).
//! 2. **Write-behind** — a per-doc worker (woken by the doc's root
//!    subscription, debounced ~1 s) renders the doc as markdown
//!    (`vault_doc::render`) into
//!    [`vault_live::Backend::put_file`] with [`IfMatch::Force`],
//!    under the backend's existing write lock. The committed sha is
//!    recorded in the doc's `meta` map and in the in-memory
//!    [`FlushState`] **before** the write, so the resulting
//!    [`VaultEvent::Put`] broadcast is recognized as our own echo.
//!    sha-guarded clients, Obsidian, and the FS watcher all observe
//!    an ordinary `Put` with a coherent sha.
//! 3. **Inbound** — a per-vault listener on the backend's broadcast
//!    channel (fed by both `put_file` from non-CRDT clients and the
//!    FS watcher) merges external file writes into *open* docs as a
//!    change (`vault_doc::merge_text` against the last flushed text:
//!    props by key, the body by character), so edits typed
//!    concurrently interleave instead of being reverted. Docs that
//!    aren't open are skipped — the next open merges the edit.
//!
//! ## Conflict policy (canonical statement)
//!
//! - **CRDT wins between collaborating clients**: concurrent edits
//!   from synced replicas merge by Loro semantics; there is no
//!   conflict banner on the collab path.
//! - **The document is the truth; text coming in is a change**
//!   (`offline.md`, 2026-10-13): an external `put_file` / disk write
//!   is merged three-way against the version it was made from — open
//!   or closed doc alike — and the file is then rewritten from the
//!   doc. The result may interleave with concurrent typing but never
//!   drops a whole edit.
//! - **The sha-conflict banner remains for non-collab clients only**:
//!   they keep conditional writes (`IfMatch::Sha`) against whatever
//!   sha the write-behind last committed.
//!
//! See `docs/architecture/vault-crdt-reconciliation.md` for the
//! prose version (identity scheme, rename caveat, crash windows).
//!
//! ## Locking & lifetime rules
//!
//! - **Never own a strong [`CrdtDoc`] inside a loro subscription
//!   closure** (deadlock on the subscriber registry's non-reentrant
//!   lock — see `CrdtDoc::downgrade`). The write-behind subscription
//!   closure owns only an `UnboundedSender<()>`; the worker holds a
//!   [`crdt::WeakCrdtDoc`] and upgrades per flush.
//! - The subscription is [`detach`](crdt::loro::Subscription::detach)ed:
//!   it lives exactly as long as the doc, so when the registry evicts
//!   (and drops) an idle doc the sender drops, the worker's channel
//!   closes, and the worker exits. The next open spawns a fresh one.
//! - Flush and inbound-merge for one doc serialize on a per-doc
//!   `tokio::sync::Mutex`, so the `read text → write file → record
//!   sha` window can't interleave with a concurrent merge.

#![cfg(not(target_arch = "wasm32"))]

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

// t[impl storage.crdt.layer] — concurrent structured state merges
// through `architect-crdt` over Loro. Since `offline.md`'s decision
// (2026-10-13) the doc is the authored record and the file its mirror:
// it is written from the doc, and text arriving in it merges back as a
// change
use architect_telemetry::wide;
use crdt::codec::TextOp;
use crdt::registry::DocRegistry;
use crdt::sync::{DocSync, SyncDown, SyncError};
use crdt::{CrdtDoc, FilePersistence, PersistError, WeakCrdtDoc};
use sha2::{Digest, Sha256};
use tokio::sync::broadcast;
use uuid::Uuid;
use vault_live::Backend;
use vault_proto::{
    COLLAB_META_CONTAINER, COLLAB_META_FLUSHED_AT, COLLAB_META_FLUSHED_SHA, IfMatch, VaultEvent,
    VaultSync, collab_doc_id,
};

/// Quiet window between the last doc change and the write-behind
/// flush. Long enough to coalesce a typing burst into one disk
/// write + one broadcast, short enough that sha-clients and the
/// folder index never lag far behind the collaborative truth.
pub const WRITE_BEHIND_DEBOUNCE: Duration = Duration::from_millis(1_000);

/// Compact each doc's persistence every N updates (forwarded to the
/// registry → per-doc hosts). Keystroke-grained docs are chatty;
/// without this the update log grows one file per commit.
const COMPACT_EVERY: u32 = 64;

/// Evict (compact + drop) docs with no live sessions and no activity
/// for this long. The next attach re-opens through the factory, which
/// re-seeds from the vault file — so eviction is also the recovery
/// path for any drift while nobody was collaborating.
const IDLE_EVICTION: Duration = Duration::from_secs(900);

/// The last file-side content this server reconciled with — either
/// flushed out (write-behind) or merged in (inbound). `sha` is the
/// echo guard; `text` is the three-way merge base for inbound writes.
#[derive(Clone)]
struct FlushState {
    sha: String,
    text: String,
}

struct Shared {
    backend: Backend,
    debounce: Duration,
    /// doc id → last reconciled file state. See [`FlushState`].
    flushed: Mutex<HashMap<Uuid, FlushState>>,
    /// Per-doc reconcile lock — serializes write-behind flushes
    /// against inbound merges for the same doc.
    locks: Mutex<HashMap<Uuid, Arc<tokio::sync::Mutex<()>>>>,
    /// Vault ids that already have an inbound listener.
    watched: Mutex<HashSet<String>>,
    /// Where documents persist (one dir per doc id) — how a delete finds
    /// out a file had a document even when nobody opened it since boot.
    crdt_root: PathBuf,
}

/// The org-level reconciler: a [`DocRegistry`] whose factory seeds
/// per-file docs from the vault, plus the write-behind and inbound
/// loops described in the module docs. Cheap to clone.
///
/// Construct **inside a tokio runtime** (the eviction sweeper and the
/// per-doc workers are tokio tasks).
#[derive(Clone)]
pub struct VaultCollab {
    registry: DocRegistry,
    shared: Arc<Shared>,
}

impl VaultCollab {
    /// Reconciler over `backend`, persisting docs under `crdt_root`
    /// (typically `<org>/crdt/`; one subdir per doc id). Production
    /// defaults: 1 s write-behind debounce, compaction every
    /// [`COMPACT_EVERY`] updates, 15 min idle eviction.
    #[must_use]
    pub fn new(backend: Backend, crdt_root: PathBuf) -> Self {
        Self::with_debounce(backend, crdt_root, WRITE_BEHIND_DEBOUNCE)
    }

    /// [`Self::new`] with an explicit write-behind debounce — tests
    /// shrink it so flush assertions don't wait a wall-clock second.
    #[must_use]
    pub fn with_debounce(backend: Backend, crdt_root: PathBuf, debounce: Duration) -> Self {
        let shared = Arc::new(Shared {
            backend: backend.clone(),
            debounce,
            flushed: Mutex::new(HashMap::new()),
            locks: Mutex::new(HashMap::new()),
            watched: Mutex::new(HashSet::new()),
            crdt_root: crdt_root.clone(),
        });

        let factory_shared = shared.clone();
        let registry = DocRegistry::new(move |doc_id| {
            let shared = factory_shared.clone();
            let root = crdt_root.clone();
            Box::pin(async move { open_doc(&shared, doc_id, root).await })
        })
        .with_compaction(COMPACT_EVERY)
        // Admission: a doc id is served only if `open_collab`
        // registered it against a real vault file. The hook sees
        // nothing but the Uuid — the reverse map lives on the backend.
        .with_admission({
            let backend = backend.clone();
            move |doc_id| backend.collab_route(doc_id).is_some()
        })
        // Authorization: the file's write guard, asked for the caller
        // the permissions gate resolved, on every attach through every
        // lane (the collab lane, the peer lane, this node's own).
        .with_authorizer({
            let backend = backend.clone();
            move |who, doc_id| {
                may_attach(&backend, vault_live::write_guard::account_of(who), doc_id).is_ok()
            }
        })
        .with_idle_eviction(IDLE_EVICTION);

        Self { registry, shared }
    }

    /// The registry to mount: implements `DocSync` + `DocPresence`,
    /// so one dispatcher pair serves every vault-file doc.
    #[must_use]
    pub fn registry(&self) -> &DocRegistry {
        &self.registry
    }

    /// Start the inbound listener for `vault_id` (idempotent): every
    /// [`VaultEvent::Put`] on the backend's broadcast channel — from
    /// non-CRDT `put_file` callers *and* the FS watcher, if attached —
    /// is merged into the matching open doc. Echoes of our own
    /// write-behind flushes are recognized by sha and skipped.
    pub fn watch_vault(&self, vault_id: &str) {
        {
            let mut watched = self.shared.watched.lock().expect("watched lock poisoned");
            if !watched.insert(vault_id.to_string()) {
                return;
            }
        }
        let vault_id = vault_id.to_string();
        let shared = self.shared.clone();
        let registry = self.registry.clone();
        tokio::spawn(async move {
            // NOTE: this task holds the registry (which holds `shared`
            // through its factory), so it lives for the backend
            // channel's lifetime — process-scoped, like the registry
            // itself.
            let sender = shared.backend.channel(&vault_id).await;
            let mut rx = sender.subscribe();
            loop {
                match rx.recv().await {
                    Ok(VaultEvent::Put { path, sha256, .. }) => {
                        apply_inbound(&shared, &registry, &vault_id, &path, &sha256).await;
                    }
                    // A file deleted outside the document (the app, an
                    // editor, `rm`) marks the document deleted — the
                    // tombstone that carries the delete to every other
                    // holder instead of being undone by the next flush.
                    Ok(VaultEvent::Delete { path }) => {
                        apply_delete(&shared, &registry, &vault_id, &path).await;
                    }
                    Ok(VaultEvent::Resync) => {}
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        // We re-read file content per event rather
                        // than trusting event payloads, so a lag only
                        // means some merges happen on the *next*
                        // event for that path.
                        tracing::warn!(vault_id, lagged = n, "vault-collab: inbound lagged");
                    }
                    Err(broadcast::error::RecvError::Closed) => return,
                }
            }
        });
    }

    /// The last file sha this reconciler flushed/merged for `doc_id`.
    /// Observability + tests.
    #[must_use]
    pub fn flushed_sha(&self, doc_id: Uuid) -> Option<String> {
        self.shared
            .flushed
            .lock()
            .expect("flushed lock poisoned")
            .get(&doc_id)
            .map(|f| f.sha.clone())
    }

    /// Whether an inbound `Put` with `sha256` for `doc_id` would be
    /// treated as an echo of our own write-behind. Exposed for the
    /// echo-guard unit test.
    #[must_use]
    pub fn is_echo(&self, doc_id: Uuid, sha256: &str) -> bool {
        is_echo(&self.shared, doc_id, sha256)
    }
}

// ── Open / seed ─────────────────────────────────────────────────────────

/// Registry factory body: open the doc over file persistence,
/// reconcile it with the vault file, and spawn its write-behind
/// worker.
async fn open_doc(
    shared: &Arc<Shared>,
    doc_id: Uuid,
    crdt_root: PathBuf,
) -> Result<CrdtDoc, PersistError> {
    let Some((vault_id, path)) = shared.backend.collab_route(doc_id) else {
        // Admission already gates this; double-check so a racing
        // deregistration can't open an unroutable doc.
        return Err(PersistError::Backend(format!(
            "doc {doc_id} has no registered vault file"
        )));
    };
    let doc = CrdtDoc::open(doc_id, FilePersistence::new(crdt_root)).await?;

    // Reconcile doc ⇄ file under the doc's reconcile lock.
    let lock = doc_lock(shared, doc_id);
    let _g = lock.lock().await;

    // A doc made before the body + props split converts once.
    // t[impl offline.doc.shape]
    if vault_doc::migrate_whole_file(doc.loro()) {
        vault_doc::commit_as(doc.loro(), "migrate: whole-file text → body + props");
    }

    let recorded = read_meta_flushed(&doc);
    let file = match shared.backend.get_file(&vault_id, &path) {
        Ok(bytes) => Some(String::from_utf8_lossy(&bytes.0).into_owned()),
        // No file (vanished, or a document first created offline on a
        // device): the doc is the truth; its first flush writes it.
        Err(_) => None,
    };

    let mut dirty = false;
    match file {
        Some(file_text) => {
            let file_sha = sha256_hex(file_text.as_bytes());
            if vault_doc::is_deleted(doc.loro()) {
                // A file at a deleted document's path: somebody made it
                // again (a restore, `git checkout`, a new note of the same
                // name). That revives the document, and the file's text
                // merges in below like any outside edit. A tombstone from
                // a peer never lands here: it arrives through an open
                // session, and the write-behind removes the file then.
                vault_doc::set_deleted(doc.loro(), false);
            }
            if recorded.as_deref() == Some(file_sha.as_str()) {
                // The mirror is what this doc last flushed. Newer
                // (persisted, unflushed) edits — a crash inside the
                // debounce window — flush now.
                dirty = vault_doc::render(doc.loro(), Some(&file_text)) != file_text;
            } else if vault_doc::doc::is_blank(doc.loro()) {
                // First open: the document is made from the file — as
                // the same change every node makes from the same bytes,
                // so two nodes holding one file hold one document.
                vault_doc::seed_from_file(doc.loro(), doc_id, &file_text);
            } else {
                // The file was edited while the doc was closed. The doc
                // is the truth; the edit is text coming in, merged as a
                // change against the version the mirror was rendered
                // from — so neither the doc's unflushed edits nor the
                // file's edit is lost — and the mirror is then written
                // from the merged doc.
                // t[impl offline.doc.materialise]
                let base = flushed_render(&doc, &file_text);
                vault_doc::merge_text(doc.loro(), &base, &file_text);
                dirty = true;
            }
            vault_doc::commit_as(doc.loro(), "outside edit: vault file");
            set_flushed(shared, doc_id, &file_sha, &file_text);
            write_meta_flushed(&doc, &file_sha);
            if !dirty {
                // The doc renders as the mirror right now: this is the
                // version the mirror stands for.
                write_meta_flushed_at(&doc, &doc.loro().oplog_frontiers());
            }
            doc.loro().commit();
        }
        // No file: a document first made elsewhere writes it — unless it
        // is a deleted one, whose file is meant to be gone.
        None => {
            dirty = !vault_doc::doc::is_blank(doc.loro()) && !vault_doc::is_deleted(doc.loro());
        }
    }

    spawn_write_behind(shared, doc_id, &doc, vault_id, path, dirty);
    Ok(doc)
}

/// What the doc rendered at its last flush — the base an outside edit of
/// the mirror was made from. Falls back to the doc as it is (a two-way
/// merge) when the doc never recorded a flushed version.
fn flushed_render(doc: &CrdtDoc, file_text: &str) -> String {
    let at = read_meta_flushed_at(doc);
    let forked = at.and_then(|f| doc.loro().fork_at(&f).ok());
    match forked {
        Some(old) => vault_doc::render(&old, Some(file_text)),
        None => vault_doc::render(doc.loro(), Some(file_text)),
    }
}

// ── Write-behind ────────────────────────────────────────────────────────

/// Wire the doc's change subscription (detached — lives as long as
/// the doc) to a debounced worker that flushes the doc's text into
/// `put_file`. `flush_now` schedules an immediate first flush (the
/// crash-recovery path above).
fn spawn_write_behind(
    shared: &Arc<Shared>,
    doc_id: Uuid,
    doc: &CrdtDoc,
    vault_id: String,
    path: String,
    flush_now: bool,
) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    if flush_now {
        let _ = tx.send(());
    }
    // `subscribe_root` fires for every committed change — local
    // commits (seed, meta) AND imports (client edits relayed by the
    // sync host). The closure owns nothing but the sender (the
    // WeakCrdtDoc rule); detaching ties its lifetime to the doc, so
    // eviction ⇒ sender drops ⇒ the worker exits.
    let bridge = tx.clone();
    doc.loro()
        .subscribe_root(Arc::new(move |_| {
            let _ = bridge.send(());
        }))
        .detach();
    drop(tx);

    let weak = doc.downgrade();
    let shared = shared.clone();
    tokio::spawn(async move {
        loop {
            // Wait for the first change signal…
            if rx.recv().await.is_none() {
                break; // doc dropped (evicted) — worker retires
            }
            // …then absorb the burst until it goes quiet.
            let mut closed = false;
            loop {
                match tokio::time::timeout(shared.debounce, rx.recv()).await {
                    Ok(Some(())) => {}
                    Ok(None) => {
                        closed = true;
                        break;
                    }
                    Err(_) => break, // debounce window elapsed
                }
            }
            flush(&shared, doc_id, &weak, &vault_id, &path).await;
            if closed {
                break;
            }
        }
    });
}

/// One write-behind flush: project the doc's text onto disk through
/// the backend (its write lock + atomic tmp-rename + broadcast), and
/// record the new sha (echo guard + doc meta) so the resulting
/// `VaultEvent::Put` isn't re-applied as inbound.
async fn flush(shared: &Arc<Shared>, doc_id: Uuid, weak: &WeakCrdtDoc, vault_id: &str, path: &str) {
    let Some(doc) = weak.upgrade() else { return };
    let lock = doc_lock(shared, doc_id);
    let _g = lock.lock().await;

    if vault_doc::is_deleted(doc.loro()) {
        remove_mirror(shared, doc_id, vault_id, path).await;
        return;
    }

    // The mirror is rendered from the doc (`offline.doc.materialise`),
    // keeping the last-written file's unchanged frontmatter lines.
    let previous = {
        let flushed = shared.flushed.lock().expect("flushed lock poisoned");
        flushed.get(&doc_id).map(|f| f.text.clone())
    };
    let rendered_at = doc.loro().oplog_frontiers();
    let doc_text = vault_doc::render(doc.loro(), previous.as_deref());
    let sha = sha256_hex(doc_text.as_bytes());
    let prev = {
        let mut flushed = shared.flushed.lock().expect("flushed lock poisoned");
        let prev = flushed.get(&doc_id).cloned();
        if prev.as_ref().is_some_and(|f| f.sha == sha) {
            return; // clean — nothing new to flush
        }
        // Record BEFORE the write: the broadcast fires inside
        // put_file, and the inbound listener must already recognize
        // the sha as ours.
        flushed.insert(
            doc_id,
            FlushState {
                sha: sha.clone(),
                text: doc_text.clone(),
            },
        );
        prev
    };

    let backend = shared.backend.clone();
    let (v, p, bytes) = (
        vault_id.to_string(),
        path.to_string(),
        doc_text.into_bytes(),
    );
    let put = tokio::task::spawn_blocking(move || {
        // Force: between collaborating clients the CRDT already
        // merged — the doc text IS the converged truth, and external
        // writes are folded in by the inbound listener rather than
        // guarded against here.
        backend.put_file(&v, &p, bytes, IfMatch::Force)
    })
    .await;

    match put {
        Ok(Ok(ack)) => {
            debug_assert_eq!(ack.sha256, sha, "backend sha must match ours");
            write_meta_flushed(&doc, &sha);
            write_meta_flushed_at(&doc, &rendered_at);
            doc.loro().commit();
        }
        Ok(Err(e)) => {
            tracing::warn!(
                ?doc_id,
                path,
                "vault-collab: write-behind put_file failed: {e}"
            );
            restore_guard(shared, doc_id, &sha, prev);
        }
        Err(e) => {
            tracing::warn!(?doc_id, path, "vault-collab: write-behind task failed: {e}");
            restore_guard(shared, doc_id, &sha, prev);
        }
    }
}

/// A deleted document's flush: remove the mirror — only the file this
/// document last wrote (sha-guarded), never one somebody wrote since,
/// which is a revival and arrives as an inbound `Put`. The removal's own
/// `Delete` event finds the document already deleted and does nothing.
async fn remove_mirror(shared: &Arc<Shared>, doc_id: Uuid, vault_id: &str, path: &str) {
    let ours = shared
        .flushed
        .lock()
        .expect("flushed lock poisoned")
        .get(&doc_id)
        .map(|f| f.sha.clone());
    let backend = shared.backend.clone();
    let (v, p) = (vault_id.to_string(), path.to_string());
    let removed = tokio::task::spawn_blocking(move || match ours {
        Some(sha) => backend.delete_file(&v, &p, IfMatch::Sha(sha)),
        // Nothing recorded: the file is not one this document is known
        // to have written, so it is left alone.
        None => Ok(()),
    })
    .await;
    match removed {
        Ok(Ok(())) => {
            shared
                .flushed
                .lock()
                .expect("flushed lock poisoned")
                .remove(&doc_id);
        }
        Ok(Err(e)) => tracing::warn!(?doc_id, "vault-collab: deleted document's file kept: {e}"),
        Err(e) => tracing::warn!(?doc_id, "vault-collab: delete task failed: {e}"),
    }
}

/// A file was deleted outside its document: set the tombstone, so the
/// delete travels with the document. Only for a file that has a document
/// — registered this run, or persisted from an earlier one (a document
/// nobody has opened since boot still has to learn its file is gone).
/// Our own removal and a file that is back by now change nothing.
async fn apply_delete(shared: &Arc<Shared>, registry: &DocRegistry, vault_id: &str, path: &str) {
    let doc_id = collab_doc_id(vault_id, path);
    if shared.backend.collab_route(doc_id).is_none() {
        if !shared.crdt_root.join(doc_id.to_string()).is_dir() {
            return; // never a document here
        }
        if shared.backend.register_collab(vault_id, path).is_err() {
            return;
        }
    }
    let lock = doc_lock(shared, doc_id);
    let _g = lock.lock().await;
    if shared.backend.get_file(vault_id, path).is_ok() {
        return; // written again since the event
    }
    let Ok(doc) = registry.doc(doc_id).await else {
        return;
    };
    if vault_doc::is_deleted(doc.loro()) {
        return;
    }
    vault_doc::set_deleted(doc.loro(), true);
    vault_doc::commit_as(doc.loro(), "deleted: vault file");
    shared
        .flushed
        .lock()
        .expect("flushed lock poisoned")
        .remove(&doc_id);
}

/// Roll the echo guard back to `prev` after a failed flush — but only
/// if it still holds the value we wrote (a concurrent reconcile may
/// have moved it on).
fn restore_guard(shared: &Shared, doc_id: Uuid, ours: &str, prev: Option<FlushState>) {
    let mut flushed = shared.flushed.lock().expect("flushed lock poisoned");
    if flushed.get(&doc_id).is_some_and(|f| f.sha == ours) {
        match prev {
            Some(p) => {
                flushed.insert(doc_id, p);
            }
            None => {
                flushed.remove(&doc_id);
            }
        }
    }
}

// ── Inbound ─────────────────────────────────────────────────────────────

/// Merge an external file write into the matching open doc. Skips:
/// unregistered paths, docs that aren't open (the next open
/// re-seeds), and echoes of our own flushes.
async fn apply_inbound(
    shared: &Arc<Shared>,
    registry: &DocRegistry,
    vault_id: &str,
    path: &str,
    event_sha: &str,
) {
    let doc_id = collab_doc_id(vault_id, path);
    if shared.backend.collab_route(doc_id).is_none() {
        return; // not a collab file
    }
    if is_echo(shared, doc_id, event_sha) {
        return; // our own write-behind (or the watcher's duplicate of it)
    }
    if !registry.is_open(doc_id).await {
        return; // closed/evicted — the next open re-seeds from disk
    }

    let lock = doc_lock(shared, doc_id);
    let _g = lock.lock().await;

    // Re-read under the lock — the event's payload may already be
    // stale; we always reconcile against the file's current content.
    let Ok(bytes) = shared.backend.get_file(vault_id, path) else {
        return; // deleted since — see the Delete note in watch_vault
    };
    let file_text = String::from_utf8_lossy(&bytes.0).into_owned();
    let file_sha = sha256_hex(file_text.as_bytes());
    if is_echo(shared, doc_id, &file_sha) {
        return;
    }
    let Ok(doc) = registry.doc(doc_id).await else {
        return;
    };

    let base = {
        let flushed = shared.flushed.lock().expect("flushed lock poisoned");
        flushed.get(&doc_id).map(|f| f.text.clone())
    };

    // A file written at a deleted document's path is the document made
    // again: the tombstone comes off, and the text merges in as usual.
    vault_doc::set_deleted(doc.loro(), false);

    // Text coming in is a change, never "the file wins": what differs
    // between the last reconciled file (base) and the new one — each
    // property by key, the body's edit character by character — is
    // merged into the doc, so doc edits typed concurrently with the
    // external write stay. No base (first event before any flush
    // record) falls back to a two-way merge against the doc's render.
    // t[impl offline.doc.materialise]
    let old = base.unwrap_or_else(|| vault_doc::render(doc.loro(), Some(&file_text)));
    vault_doc::merge_text(doc.loro(), &old, &file_text);
    vault_doc::commit_as(doc.loro(), "outside edit: vault file");

    // The file content is now incorporated: record it as the new
    // reconcile baseline (echo for watcher duplicates of this very
    // event included). If concurrent doc edits made the doc differ
    // from the file, the commit below wakes the write-behind, which
    // writes the mirror from the merged doc.
    set_flushed(shared, doc_id, &file_sha, &file_text);
    write_meta_flushed(&doc, &file_sha);
    if vault_doc::render(doc.loro(), Some(&file_text)) == file_text {
        write_meta_flushed_at(&doc, &doc.loro().oplog_frontiers());
    }
    doc.loro().commit();
}

// ── Small helpers ───────────────────────────────────────────────────────

fn doc_lock(shared: &Shared, doc_id: Uuid) -> Arc<tokio::sync::Mutex<()>> {
    shared
        .locks
        .lock()
        .expect("locks lock poisoned")
        .entry(doc_id)
        .or_default()
        .clone()
}

fn is_echo(shared: &Shared, doc_id: Uuid, sha256: &str) -> bool {
    shared
        .flushed
        .lock()
        .expect("flushed lock poisoned")
        .get(&doc_id)
        .is_some_and(|f| f.sha == sha256)
}

fn set_flushed(shared: &Shared, doc_id: Uuid, sha: &str, text: &str) {
    shared
        .flushed
        .lock()
        .expect("flushed lock poisoned")
        .insert(
            doc_id,
            FlushState {
                sha: sha.to_string(),
                text: text.to_string(),
            },
        );
}

/// `meta.flushed_sha` on the doc — survives restarts so a re-opened
/// doc can tell "my flush is current" from "the file changed".
fn read_meta_flushed(doc: &CrdtDoc) -> Option<String> {
    use crdt::loro::{LoroValue, ValueOrContainer};
    match doc
        .loro()
        .get_map(COLLAB_META_CONTAINER)
        .get(COLLAB_META_FLUSHED_SHA)
    {
        Some(ValueOrContainer::Value(LoroValue::String(s))) => Some((*s).clone()),
        _ => None,
    }
}

fn write_meta_flushed(doc: &CrdtDoc, sha: &str) {
    if let Err(e) = doc
        .loro()
        .get_map(COLLAB_META_CONTAINER)
        .insert(COLLAB_META_FLUSHED_SHA, sha)
    {
        tracing::warn!("vault-collab: failed to record flushed sha: {e}");
    }
}

/// `meta.flushed_at` — the doc version the mirror file was rendered
/// from: the base an outside edit of the mirror merges against.
fn read_meta_flushed_at(doc: &CrdtDoc) -> Option<crdt::loro::Frontiers> {
    use crdt::loro::{LoroValue, ValueOrContainer};
    match doc
        .loro()
        .get_map(COLLAB_META_CONTAINER)
        .get(COLLAB_META_FLUSHED_AT)
    {
        Some(ValueOrContainer::Value(LoroValue::Binary(b))) => {
            crdt::loro::Frontiers::decode(&b).ok()
        }
        _ => None,
    }
}

fn write_meta_flushed_at(doc: &CrdtDoc, at: &crdt::loro::Frontiers) {
    let bytes: Vec<u8> = at.encode();
    if let Err(e) = doc
        .loro()
        .get_map(COLLAB_META_CONTAINER)
        .insert(COLLAB_META_FLUSHED_AT, bytes)
    {
        tracing::warn!("vault-collab: failed to record flushed version: {e}");
    }
}

/// Minimal insert/delete pair from `old → new` (Unicode scalars) — the
/// codec's own, re-exported for callers that diffed through this crate.
#[must_use]
pub fn diff_text_ops(old: &str, new: &str) -> Vec<TextOp> {
    vault_doc::body::diff_ops(old, new)
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    let out = h.finalize();
    let mut hex = String::with_capacity(out.len() * 2);
    for b in out {
        use std::fmt::Write;
        let _ = write!(hex, "{b:02x}");
    }
    hex
}

/// May `account` attach to `doc_id`? A vault file is written two ways:
/// a save (`VaultSync::put_file`, which asks the backend's
/// [`vault_live::WriteGuard`] itself) and a live session, whose edits
/// reach disk through the write-behind, which runs as the server. So the
/// rule is applied at the attach: the doc is looked back up to its file
/// and the same guard is asked. An id no `open_collab` registered is the
/// registry's admission to refuse.
fn may_attach(backend: &Backend, account: Option<&str>, doc_id: Uuid) -> Result<(), SyncError> {
    let Some((vault_id, path)) = backend.collab_route(doc_id) else {
        return Ok(());
    };
    backend
        .check_write(&vault_id, &path, account)
        .map_err(|e| SyncError::Internal(e.to_string()))
}

/// The collab lane: the registry's `DocSync`, refusing (before anything
/// is opened) a caller the write guard refuses, with the guard's reason,
/// and a replica whose history shares nothing with the served document:
/// the server's document was recreated (a fresh data root, a reseeded
/// demo) under the same id. Merging the two would put every line in
/// twice; refusing with [`crdt::peer::FOREIGN_LINEAGE`] lets the device
/// keep its copy and decide (`vault_doc::Replica::reset`).
impl DocSync for VaultCollab {
    // t[impl offline.doc.access] — every attach (each reconnect of each
    // device, offline-created documents' first sync included) is checked
    // against the file's write guard for the caller on this request, and
    // a replica from another lineage is refused rather than merged
    async fn sync(
        &self,
        doc_id: Uuid,
        from: Vec<u8>,
        up: vox::Rx<Vec<u8>>,
        down: vox::Tx<SyncDown>,
    ) -> Result<(), SyncError> {
        let who = architect::permissions_gate::caller();
        if let Err(e) = may_attach(
            &self.shared.backend,
            vault_live::write_guard::account_of(who.as_ref()),
            doc_id,
        ) {
            wide::set("collab.sync", "refused");
            return Err(e);
        }
        if let Err(e) = crdt::peer::check_lineage(&self.registry, doc_id, &from).await {
            wide::set("collab.sync", "foreign_lineage");
            return Err(e);
        }
        wide::set("collab.sync", "attached");
        self.registry.sync(doc_id, from, up, down).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vault_proto::COLLAB_BODY_CONTAINER;

    const DEBOUNCE: Duration = Duration::from_millis(60);

    fn setup() -> (tempfile::TempDir, Backend, VaultCollab) {
        let tmp = tempfile::tempdir().unwrap();
        let backend = Backend::single("v1", tmp.path().join("vault")).unwrap();
        let collab = VaultCollab::with_debounce(backend.clone(), tmp.path().join("crdt"), DEBOUNCE);
        (tmp, backend, collab)
    }

    /// `Backend`'s sync methods broadcast through
    /// `blocking_read`-style locks — they must run on the blocking
    /// pool, exactly as the `TokioBlockingDispatcher` runs them in
    /// production.
    async fn put(backend: &Backend, path: &str, body: &str) -> String {
        let (b, p, body) = (backend.clone(), path.to_string(), body.as_bytes().to_vec());
        tokio::task::spawn_blocking(move || {
            b.put_file("v1", &p, body, IfMatch::Force).unwrap().sha256
        })
        .await
        .unwrap()
    }

    fn read(backend: &Backend, path: &str) -> String {
        String::from_utf8(backend.get_file("v1", path).unwrap().0).unwrap()
    }

    async fn eventually(what: &str, mut cond: impl AsyncFnMut() -> bool) {
        for _ in 0..200 {
            if cond().await {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("timed out waiting for: {what}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn first_open_seeds_doc_from_file() {
        let (_tmp, backend, collab) = setup();
        put(&backend, "n.md", "hello vault").await;
        let ack = backend.open_collab("v1", "n.md").unwrap();

        let doc = collab.registry().doc(ack.doc_id).await.unwrap();
        let text = doc.loro().get_text(COLLAB_BODY_CONTAINER).to_string();
        assert_eq!(text, "hello vault");
        assert_eq!(
            collab.flushed_sha(ack.doc_id).as_deref(),
            Some(ack.sha256.as_str())
        );
        assert_eq!(
            read_meta_flushed(&doc).as_deref(),
            Some(ack.sha256.as_str())
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn unregistered_doc_is_refused() {
        let (_tmp, _backend, collab) = setup();
        let stranger = collab_doc_id("v1", "never-registered.md");
        assert!(collab.registry().doc(stranger).await.is_err());
    }

    async fn remove(backend: &Backend, path: &str) {
        let (b, p) = (backend.clone(), path.to_string());
        tokio::task::spawn_blocking(move || b.delete_file("v1", &p, IfMatch::Force).unwrap())
            .await
            .unwrap();
    }

    fn exists(backend: &Backend, path: &str) -> bool {
        backend.get_file("v1", path).is_ok()
    }

    // t[verify offline.peers] — a delete travels with the document:
    // deleting the file marks it, a tombstone from elsewhere removes the
    // file instead of being undone, and writing the file again revives it
    #[tokio::test(flavor = "multi_thread")]
    async fn a_delete_is_a_tombstone_that_travels_and_a_rewrite_revives() {
        let (_tmp, backend, collab) = setup();
        collab.watch_vault("v1");
        put(&backend, "n.md", "keep me").await;
        let ack = backend.open_collab("v1", "n.md").unwrap();
        let doc = collab.registry().doc(ack.doc_id).await.unwrap();

        // Deleted outside the document → the document says so, and its
        // next flush does not bring the file back.
        remove(&backend, "n.md").await;
        let d = doc.clone();
        eventually("the tombstone", async || vault_doc::is_deleted(d.loro())).await;
        tokio::time::sleep(DEBOUNCE * 4).await;
        assert!(
            !exists(&backend, "n.md"),
            "a flush recreated a deleted file"
        );

        // Written again → revived, with the new text.
        put(&backend, "n.md", "back again").await;
        let d = doc.clone();
        eventually("the revival", async || !vault_doc::is_deleted(d.loro())).await;
        assert_eq!(read(&backend, "n.md"), "back again");

        // A tombstone that arrives from elsewhere (a peer, a device) on
        // the open document removes the file it had written.
        vault_doc::set_deleted(doc.loro(), true);
        vault_doc::commit_as(doc.loro(), "deleted elsewhere");
        let b = backend.clone();
        eventually("the mirror removed", async || !exists(&b, "n.md")).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn write_behind_flushes_doc_edits_to_disk() {
        let (_tmp, backend, collab) = setup();
        put(&backend, "n.md", "hello").await;
        let ack = backend.open_collab("v1", "n.md").unwrap();
        let doc = collab.registry().doc(ack.doc_id).await.unwrap();

        // Subscribe before editing so we observe the flush broadcast.
        let chan = backend.channel("v1").await;
        let mut rx = chan.subscribe();

        let text = doc.loro().get_text(COLLAB_BODY_CONTAINER);
        text.insert(5, " world").unwrap();
        doc.loro().commit();

        let backend2 = backend.clone();
        eventually("flush lands on disk", async || {
            read(&backend2, "n.md") == "hello world"
        })
        .await;

        // The broadcast carried the flushed sha — coherent for
        // sha-clients — and the guard recognizes it as our echo.
        let evt = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("event timeout")
            .expect("recv ok");
        match evt {
            VaultEvent::Put { path, sha256, .. } => {
                assert_eq!(path, "n.md");
                assert_eq!(sha256, sha256_hex(b"hello world"));
                assert!(
                    collab.is_echo(ack.doc_id, &sha256),
                    "own flush must be an echo"
                );
            }
            other => panic!("expected Put, got {other:?}"),
        }
        assert_eq!(
            read_meta_flushed(&doc).as_deref(),
            Some(sha256_hex(b"hello world").as_str())
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn inbound_put_merges_into_open_doc() {
        let (_tmp, backend, collab) = setup();
        put(&backend, "n.md", "alpha\n").await;
        let ack = backend.open_collab("v1", "n.md").unwrap();
        let doc = collab.registry().doc(ack.doc_id).await.unwrap();
        collab.watch_vault("v1");

        // A non-CRDT client (sha-guarded UI, Obsidian, CLI) writes. The
        // listener subscribes asynchronously, so on a loaded machine a
        // single write can land before it is listening: write again until
        // the merge shows (the same bytes again are a no-op once merged).
        let d = doc.clone();
        let merged = || d.loro().get_text(COLLAB_BODY_CONTAINER).to_string() == "alpha\nbeta\n";
        for _ in 0..50 {
            put(&backend, "n.md", "alpha\nbeta\n").await;
            tokio::time::sleep(Duration::from_millis(100)).await;
            if merged() {
                break;
            }
        }
        eventually("inbound merge applies", async || merged()).await;
        assert_eq!(
            collab.flushed_sha(ack.doc_id).as_deref(),
            Some(sha256_hex(b"alpha\nbeta\n").as_str())
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn echo_guard_skips_own_flush() {
        let (_tmp, backend, collab) = setup();
        put(&backend, "n.md", "one").await;
        let ack = backend.open_collab("v1", "n.md").unwrap();
        let doc = collab.registry().doc(ack.doc_id).await.unwrap();
        collab.watch_vault("v1");

        // Our flush sha must read as an echo; a foreign sha must not.
        assert!(collab.is_echo(ack.doc_id, &ack.sha256));
        assert!(!collab.is_echo(ack.doc_id, &sha256_hex(b"someone else")));

        // End-to-end: a flush's own broadcast must not be re-applied —
        // the doc text stays exactly what the clients converged on.
        let text = doc.loro().get_text(COLLAB_BODY_CONTAINER);
        text.insert(3, " two").unwrap();
        doc.loro().commit();
        let backend2 = backend.clone();
        eventually("flush lands", async || read(&backend2, "n.md") == "one two").await;
        // Let the broadcast → listener cycle drain, then assert the
        // doc didn't change again (no echo re-merge, no duplication).
        tokio::time::sleep(DEBOUNCE * 4).await;
        assert_eq!(
            doc.loro().get_text(COLLAB_BODY_CONTAINER).to_string(),
            "one two"
        );
        assert_eq!(read(&backend, "n.md"), "one two");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn concurrent_doc_edit_and_external_put_interleave_without_loss() {
        let (_tmp, backend, collab) = setup();
        put(&backend, "n.md", "base\n").await;
        let ack = backend.open_collab("v1", "n.md").unwrap();
        let doc = collab.registry().doc(ack.doc_id).await.unwrap();
        collab.watch_vault("v1");
        tokio::time::sleep(Duration::from_millis(50)).await;

        // A collab client appends (not yet flushed — debounce) while a
        // non-CRDT writer appends to the *old* content.
        let text = doc.loro().get_text(COLLAB_BODY_CONTAINER);
        text.insert(5, "crdt-line\n").unwrap();
        doc.loro().commit();
        put(&backend, "n.md", "base\nfile-line\n").await;

        // Both edits must survive in the converged doc AND on disk.
        let d = doc.clone();
        eventually("both edits present in doc", async || {
            let t = d.loro().get_text(COLLAB_BODY_CONTAINER).to_string();
            t.contains("crdt-line") && t.contains("file-line") && t.contains("base")
        })
        .await;
        let backend2 = backend.clone();
        eventually("merged text flushed to disk", async || {
            let t = read(&backend2, "n.md");
            t.contains("crdt-line") && t.contains("file-line")
        })
        .await;
    }

    fn prop_of(doc: &CrdtDoc, key: &str) -> Option<String> {
        vault_doc::doc::prop(doc.loro(), key).and_then(|v| match v {
            crdt::loro::LoroValue::String(s) => Some(s.to_string()),
            _ => None,
        })
    }

    // t[verify offline.doc.shape] — a vault file's doc is body + props;
    // t[verify offline.doc.materialise] — the mirror is rendered from the
    // doc, and an outside edit merges in as a change (props by key, body
    // by character) without reverting concurrent doc edits
    #[tokio::test(flavor = "multi_thread")]
    async fn frontmatter_lives_in_props_and_outside_edits_merge_by_key() {
        let (_tmp, backend, collab) = setup();
        let original = "---\ntitle: Mix\nstatus: open\n---\nTake one.\n";
        put(&backend, "n.md", original).await;
        let ack = backend.open_collab("v1", "n.md").unwrap();
        let doc = collab.registry().doc(ack.doc_id).await.unwrap();
        collab.watch_vault("v1");
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(prop_of(&doc, "title").as_deref(), Some("Mix"));
        assert_eq!(
            doc.loro().get_text(COLLAB_BODY_CONTAINER).to_string(),
            "Take one.\n"
        );

        // In the app: the status changes and a line is typed (not yet
        // flushed)…
        vault_doc::doc::set_prop(doc.loro(), "status", crdt::loro::LoroValue::from("done"));
        doc.loro()
            .get_text(COLLAB_BODY_CONTAINER)
            .push_str("Take two.\n")
            .unwrap();
        doc.loro().commit();
        // …while an editor on disk renames it, from the original file.
        put(
            &backend,
            "n.md",
            "---\ntitle: Final mix\nstatus: open\n---\nTake one.\n",
        )
        .await;

        let want = "---\ntitle: Final mix\nstatus: done\n---\nTake one.\nTake two.\n";
        let b = backend.clone();
        eventually("the mirror holds every change", async || {
            read(&b, "n.md") == want
        })
        .await;
        assert_eq!(prop_of(&doc, "title").as_deref(), Some("Final mix"));
    }

    // t[verify offline.doc.shape] — a doc made before the split (whole
    // file in `content`) converts once, and the mirror does not change
    #[tokio::test(flavor = "multi_thread")]
    async fn a_whole_file_doc_converts_on_open() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = Backend::single("v1", tmp.path().join("vault")).unwrap();
        let crdt_root = tmp.path().join("crdt");
        let text = "---\ntitle: Old\n---\nOld body\n";
        let sha = put(&backend, "n.md", text).await;
        let ack = backend.open_collab("v1", "n.md").unwrap();
        {
            // What the previous server persisted.
            let legacy = CrdtDoc::open(ack.doc_id, FilePersistence::new(&crdt_root))
                .await
                .unwrap();
            legacy
                .loro()
                .get_text(vault_proto::COLLAB_TEXT_CONTAINER)
                .insert(0, text)
                .unwrap();
            write_meta_flushed(&legacy, &sha);
            legacy.loro().commit();
            let p = FilePersistence::new(&crdt_root);
            eventually("legacy persisted", async || {
                !crdt::Persistence::load_updates(&p, ack.doc_id)
                    .await
                    .unwrap()
                    .is_empty()
            })
            .await;
        }
        let collab = VaultCollab::with_debounce(backend.clone(), crdt_root, DEBOUNCE);
        let doc = collab.registry().doc(ack.doc_id).await.unwrap();
        assert_eq!(prop_of(&doc, "title").as_deref(), Some("Old"));
        assert_eq!(
            doc.loro().get_text(COLLAB_BODY_CONTAINER).to_string(),
            "Old body\n"
        );
        tokio::time::sleep(DEBOUNCE * 4).await;
        assert_eq!(read(&backend, "n.md"), text, "conversion rewrote the file");
    }

    // t[verify offline.doc.materialise] — the doc is the truth: a file
    // edited while the doc was closed merges into it, and the doc's own
    // unflushed edits survive
    #[tokio::test(flavor = "multi_thread")]
    async fn reopen_merges_a_closed_edit_with_unflushed_doc_edits() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = Backend::single("v1", tmp.path().join("vault")).unwrap();
        let crdt_root = tmp.path().join("crdt");
        put(&backend, "n.md", "one\n").await;
        let ack = backend.open_collab("v1", "n.md").unwrap();
        {
            // A long debounce: the doc edit is persisted, never flushed.
            let collab = VaultCollab::with_debounce(
                backend.clone(),
                crdt_root.clone(),
                Duration::from_secs(60),
            );
            let doc = collab.registry().doc(ack.doc_id).await.unwrap();
            doc.loro()
                .get_text(COLLAB_BODY_CONTAINER)
                .push_str("two (in the app)\n")
                .unwrap();
            doc.loro().commit();
            let p = FilePersistence::new(&crdt_root);
            eventually("edit persisted", async || {
                crdt::Persistence::load_updates(&p, ack.doc_id)
                    .await
                    .unwrap()
                    .len()
                    >= 2
            })
            .await;
        }
        put(&backend, "n.md", "zero (on disk)\none\n").await;
        let collab = VaultCollab::with_debounce(backend.clone(), crdt_root, DEBOUNCE);
        let doc = collab.registry().doc(ack.doc_id).await.unwrap();
        assert_eq!(
            doc.loro().get_text(COLLAB_BODY_CONTAINER).to_string(),
            "zero (on disk)\none\ntwo (in the app)\n"
        );
        let b = backend.clone();
        eventually("the mirror is rewritten from the doc", async || {
            read(&b, "n.md") == "zero (on disk)\none\ntwo (in the app)\n"
        })
        .await;
    }

    // t[verify offline.doc.create] — a document for a path with no file
    // opens blank, writes nothing until it has content, and its first
    // content creates the file
    #[tokio::test(flavor = "multi_thread")]
    async fn a_path_without_a_file_opens_and_its_first_content_creates_it() {
        let (_tmp, backend, collab) = setup();
        let ack = backend.open_collab("v1", "new.md").unwrap();
        let doc = tokio::time::timeout(Duration::from_secs(5), collab.registry().doc(ack.doc_id))
            .await
            .expect("open did not hang")
            .unwrap();
        tokio::time::sleep(DEBOUNCE * 3).await;
        assert!(
            backend.get_file("v1", "new.md").is_err(),
            "a blank doc writes no file"
        );
        doc.loro()
            .get_text(COLLAB_BODY_CONTAINER)
            .insert(0, "made offline\n")
            .unwrap();
        doc.loro().commit();
        let b = backend.clone();
        eventually("the first content creates the file", async || {
            b.get_file("v1", "new.md")
                .is_ok_and(|f| f.0 == b"made offline\n")
        })
        .await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn reopen_after_external_change_merges_it() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = Backend::single("v1", tmp.path().join("vault")).unwrap();
        let crdt_root = tmp.path().join("crdt");
        put(&backend, "n.md", "v1 text").await;
        let ack = backend.open_collab("v1", "n.md").unwrap();

        // First lifetime: open + let the seed persist.
        {
            let collab = VaultCollab::with_debounce(backend.clone(), crdt_root.clone(), DEBOUNCE);
            let doc = collab.registry().doc(ack.doc_id).await.unwrap();
            assert_eq!(
                doc.loro().get_text(COLLAB_BODY_CONTAINER).to_string(),
                "v1 text"
            );
            // Wait for the persistence writer to land the seed commit.
            eventually("seed persisted", async || {
                crdt_root.join(ack.doc_id.to_string()).exists()
            })
            .await;
        }

        // The file changes while no registry has the doc open.
        put(&backend, "n.md", "v2 text from disk").await;

        // Second lifetime (fresh registry, same persistence): the
        // factory sees meta.flushed_sha != file sha and merges the edit.
        let collab = VaultCollab::with_debounce(backend.clone(), crdt_root, DEBOUNCE);
        let doc = collab.registry().doc(ack.doc_id).await.unwrap();
        assert_eq!(
            doc.loro().get_text(COLLAB_BODY_CONTAINER).to_string(),
            "v2 text from disk"
        );
    }

    #[test]
    fn diff_ops_minimal_and_correct() {
        assert!(diff_text_ops("same", "same").is_empty());
        assert_eq!(
            diff_text_ops("hello", "hello!"),
            vec![TextOp::Insert {
                pos: 5,
                text: "!".into()
            }]
        );
        assert_eq!(
            diff_text_ops("hello!", "hello"),
            vec![TextOp::Delete { pos: 5, len: 1 }]
        );
        // Unicode: positions are scalar units, not bytes.
        assert_eq!(
            diff_text_ops("a🦀b", "a🦁b"),
            vec![
                TextOp::Delete { pos: 1, len: 1 },
                TextOp::Insert {
                    pos: 1,
                    text: "🦁".into()
                }
            ]
        );
    }
}
