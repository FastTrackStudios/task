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
//!    a fresh doc is seeded from the file's bytes; a re-opened doc
//!    whose file changed while it was evicted adopts the file via a
//!    character-level diff.
//! 2. **Write-behind** — a per-doc worker (woken by the doc's root
//!    subscription, debounced ~1 s) projects the doc's text into
//!    [`vault_live::Backend::put_file`] with [`IfMatch::Force`],
//!    under the backend's existing write lock. The committed sha is
//!    recorded in the doc's `meta` map and in the in-memory
//!    [`FlushState`] **before** the write, so the resulting
//!    [`VaultEvent::Put`] broadcast is recognized as our own echo.
//!    sha-guarded clients, Obsidian, and the FS watcher all observe
//!    an ordinary `Put` with a coherent sha.
//! 3. **Inbound** — a per-vault listener on the backend's broadcast
//!    channel (fed by both `put_file` from non-CRDT clients and the
//!    FS watcher) merges external file writes into *open* docs. The
//!    merge is three-way at character level: ops are computed from
//!    `last-flushed text → new file text` and applied to the doc
//!    (clamped), so edits typed concurrently with an external write
//!    interleave instead of being reverted. Docs that aren't open
//!    are skipped — the next open re-seeds from disk.
//!
//! ## Conflict policy (canonical statement)
//!
//! - **CRDT wins between collaborating clients**: concurrent edits
//!   from synced replicas merge by Loro semantics; there is no
//!   conflict banner on the collab path.
//! - **Non-CRDT writes merge via text diff**: an external `put_file`
//!   / disk write into an *open* doc is folded in character-by-
//!   character (three-way against the last flushed content). The
//!   result may interleave with concurrent typing but never drops a
//!   whole edit. While a doc is **closed/evicted**, the file is the
//!   source of truth — the next open adopts it (two-way diff, i.e.
//!   re-seed semantics).
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

use crdt::codec::TextOp;
use crdt::loro::LoroText;
use crdt::registry::DocRegistry;
use crdt::{CrdtDoc, FilePersistence, PersistError, WeakCrdtDoc};
use sha2::{Digest, Sha256};
use tokio::sync::broadcast;
use uuid::Uuid;
use vault_live::Backend;
use vault_proto::{
    COLLAB_META_CONTAINER, COLLAB_META_FLUSHED_SHA, COLLAB_TEXT_CONTAINER, IfMatch, VaultEvent,
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
                    // Deletes don't tear collab down in v1: the doc
                    // stays authoritative while open and the next
                    // flush recreates the file. Documented in the
                    // conflict-policy note.
                    Ok(VaultEvent::Delete { .. } | VaultEvent::Resync) => {}
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

    let text = doc.loro().get_text(COLLAB_TEXT_CONTAINER);
    let doc_text = text.to_string();
    let recorded = read_meta_flushed(&doc);
    let file = match shared.backend.get_file(&vault_id, &path) {
        Ok(bytes) => Some(String::from_utf8_lossy(&bytes.0).into_owned()),
        // File vanished between registration and open: the doc wins;
        // the first flush recreates the file.
        Err(_) => None,
    };

    let mut dirty = false;
    if let Some(file_text) = file {
        let file_sha = sha256_hex(file_text.as_bytes());
        if recorded.as_deref() == Some(file_sha.as_str()) {
            // The file is exactly what this doc last flushed. If the
            // doc has newer (persisted but unflushed) edits — a crash
            // inside the debounce window — flush them now.
            dirty = doc_text != file_text;
        } else if doc_text == file_text {
            // Contents already agree (e.g. a fresh doc over an empty
            // file) — just record the baseline. Writing the meta also
            // guarantees the doc has at least one commit, so a fresh
            // client's first sync always observes a revision.
        } else {
            // First open (empty doc → pure seed) or the file changed
            // while the doc was closed/evicted: the file is the
            // source of truth, adopt it via character diff. NOTE: in
            // the double-crash window (unflushed doc edits AND an
            // external write while down) this reverts the unflushed
            // tail — documented v1 tradeoff in the policy note.
            let ops = diff_text_ops(&doc_text, &file_text);
            apply_ops_clamped(&text, &ops);
        }
        set_flushed(shared, doc_id, &file_sha, &file_text);
        write_meta_flushed(&doc, &file_sha);
        doc.loro().commit();
    }

    spawn_write_behind(shared, doc_id, &doc, vault_id, path, dirty);
    Ok(doc)
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

    let doc_text = doc.loro().get_text(COLLAB_TEXT_CONTAINER).to_string();
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

    let text = doc.loro().get_text(COLLAB_TEXT_CONTAINER);
    let doc_text = text.to_string();
    let base = {
        let flushed = shared.flushed.lock().expect("flushed lock poisoned");
        flushed.get(&doc_id).map(|f| f.text.clone())
    };

    // Three-way at character level: ops are computed from the last
    // reconciled file content (base) to the new file content, then
    // applied to the doc — so doc edits typed concurrently with the
    // external write interleave instead of being reverted. When the
    // doc is at base (the common case), this lands the doc exactly on
    // the file's content. No base (first event before any flush
    // record) falls back to a two-way doc→file diff.
    let old = base.unwrap_or_else(|| doc_text.clone());
    let ops = diff_text_ops(&old, &file_text);
    apply_ops_clamped(&text, &ops);

    // The file content is now incorporated: record it as the new
    // reconcile baseline (echo for watcher duplicates of this very
    // event included). If concurrent doc edits made the doc differ
    // from the file, the commit below wakes the write-behind, which
    // flushes the merged text back out.
    set_flushed(shared, doc_id, &file_sha, &file_text);
    write_meta_flushed(&doc, &file_sha);
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

/// Minimal insert/delete pair from `old → new`: common-prefix +
/// common-suffix stripping over unicode scalars. Same algorithm as
/// `crdt::codec::apply_text_diff`, surfaced as ops so they can be
/// applied (clamped) to a *root* `LoroText` container.
#[must_use]
pub fn diff_text_ops(old: &str, new: &str) -> Vec<TextOp> {
    if old == new {
        return Vec::new();
    }
    let old_chars: Vec<char> = old.chars().collect();
    let new_chars: Vec<char> = new.chars().collect();

    let mut prefix = 0usize;
    while prefix < old_chars.len()
        && prefix < new_chars.len()
        && old_chars[prefix] == new_chars[prefix]
    {
        prefix += 1;
    }
    let mut suffix = 0usize;
    while suffix < old_chars.len() - prefix
        && suffix < new_chars.len() - prefix
        && old_chars[old_chars.len() - 1 - suffix] == new_chars[new_chars.len() - 1 - suffix]
    {
        suffix += 1;
    }

    let del_len = old_chars.len() - prefix - suffix;
    let ins: String = new_chars[prefix..new_chars.len() - suffix].iter().collect();

    let mut ops = Vec::with_capacity(2);
    if del_len > 0 {
        ops.push(TextOp::Delete {
            pos: prefix as u32,
            len: del_len as u32,
        });
    }
    if !ins.is_empty() {
        ops.push(TextOp::Insert {
            pos: prefix as u32,
            text: ins,
        });
    }
    ops
}

/// Apply ops to a root `LoroText`, clamping positions to the current
/// length. Clamping matters only on the three-way inbound path, where
/// concurrent doc edits may have shifted the base positions — the
/// interleaving caveat of the conflict policy.
fn apply_ops_clamped(text: &LoroText, ops: &[TextOp]) {
    for op in ops {
        let len = text.len_unicode();
        let result = match op {
            TextOp::Insert { pos, text: t } => {
                let at = (*pos as usize).min(len);
                text.insert(at, t)
            }
            TextOp::Delete { pos, len: n } => {
                let from = (*pos as usize).min(len);
                let n = (*n as usize).min(len - from);
                if n == 0 {
                    continue;
                }
                text.delete(from, n)
            }
        };
        if let Err(e) = result {
            tracing::warn!("vault-collab: text op failed: {e}");
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let text = doc.loro().get_text(COLLAB_TEXT_CONTAINER).to_string();
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

    #[tokio::test(flavor = "multi_thread")]
    async fn write_behind_flushes_doc_edits_to_disk() {
        let (_tmp, backend, collab) = setup();
        put(&backend, "n.md", "hello").await;
        let ack = backend.open_collab("v1", "n.md").unwrap();
        let doc = collab.registry().doc(ack.doc_id).await.unwrap();

        // Subscribe before editing so we observe the flush broadcast.
        let chan = backend.channel("v1").await;
        let mut rx = chan.subscribe();

        let text = doc.loro().get_text(COLLAB_TEXT_CONTAINER);
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
        // Give the listener a beat to subscribe before the write.
        tokio::time::sleep(Duration::from_millis(50)).await;

        // A non-CRDT client (sha-guarded UI, Obsidian, CLI) writes.
        put(&backend, "n.md", "alpha\nbeta\n").await;

        let d = doc.clone();
        eventually("inbound merge applies", async || {
            d.loro().get_text(COLLAB_TEXT_CONTAINER).to_string() == "alpha\nbeta\n"
        })
        .await;
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
        let text = doc.loro().get_text(COLLAB_TEXT_CONTAINER);
        text.insert(3, " two").unwrap();
        doc.loro().commit();
        let backend2 = backend.clone();
        eventually("flush lands", async || read(&backend2, "n.md") == "one two").await;
        // Let the broadcast → listener cycle drain, then assert the
        // doc didn't change again (no echo re-merge, no duplication).
        tokio::time::sleep(DEBOUNCE * 4).await;
        assert_eq!(
            doc.loro().get_text(COLLAB_TEXT_CONTAINER).to_string(),
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
        let text = doc.loro().get_text(COLLAB_TEXT_CONTAINER);
        text.insert(5, "crdt-line\n").unwrap();
        doc.loro().commit();
        put(&backend, "n.md", "base\nfile-line\n").await;

        // Both edits must survive in the converged doc AND on disk.
        let d = doc.clone();
        eventually("both edits present in doc", async || {
            let t = d.loro().get_text(COLLAB_TEXT_CONTAINER).to_string();
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

    #[tokio::test(flavor = "multi_thread")]
    async fn reopen_after_external_change_adopts_file() {
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
                doc.loro().get_text(COLLAB_TEXT_CONTAINER).to_string(),
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
        // factory sees meta.flushed_sha != file sha and adopts disk.
        let collab = VaultCollab::with_debounce(backend.clone(), crdt_root, DEBOUNCE);
        let doc = collab.registry().doc(ack.doc_id).await.unwrap();
        assert_eq!(
            doc.loro().get_text(COLLAB_TEXT_CONTAINER).to_string(),
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
