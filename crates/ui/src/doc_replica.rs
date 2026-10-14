//! Device-side document replicas for vault files — the shell's driver
//! for [`vault_doc::Replica`] (`offline.doc.replica`).
//!
//! A [`ReplicaHandle`] is a `Copy` bundle of page-owned signals (the
//! same slot/driver split as `crdt::use_doc_slot`, for the same reason:
//! long-lived editor closures capture it across session remounts). The
//! keyed driver, [`use_replica_into`], fills it:
//!
//! 1. opens the file's document from the device's store ([`doc_store`])
//!    — whatever was edited here before, offline included, is there
//!    before any network;
//! 2. keeps one sync session alive for the scope's lifetime: dial the
//!    org, `open_collab` (the server's admission + write check), then
//!    [`vault_doc::Replica::run`]; a dropped connection is retried with
//!    backoff **without** tearing anything down — edits keep going into
//!    the replica and go up when the next session attaches;
//! 3. reports honestly: [`ReplicaHandle::status`] is `Live` only once an
//!    attach landed, [`ReplicaHandle::unsynced`] is true while the
//!    replica holds anything the server has not acknowledged (and
//!    survives restarts: the acknowledged version vector is kept per
//!    device in [`crate::local_store`]);
//! 4. on a foreign-lineage refusal (the server's document was
//!    recreated), resets the stored copy and hands the discarded text to
//!    the host ([`ReplicaHandle::take_discarded`]) to fold back in.
//!
//! **Where documents are stored** is one function, [`doc_store`]: a
//! `crdt::FilePersistence` directory per org under the app data dir on
//! native, `crdt::IndexedDbPersistence` per org on the web. The device
//! Turso store (`crdt`'s `TursoPersistence`, in progress in architect)
//! replaces the native arm there and nowhere else.

use std::collections::HashMap;
use std::sync::Arc;

use architect::Duration;
use crdt::loro::VersionVector;
use crdt::{CrdtDoc, SyncStatus};
use dioxus::prelude::*;
use uuid::Uuid;
use vault_doc::{Replica, SessionEnd, SessionEvent, SharedStore};

/// Page-owned signals a [`use_replica_into`] driver fills. See the
/// module docs.
#[derive(Clone, Copy, PartialEq)]
pub struct ReplicaHandle {
    doc: Signal<Option<CrdtDoc>>,
    revision: Signal<u64>,
    status: Signal<SyncStatus>,
    unsynced: Signal<bool>,
    /// The replica holds a version worth editing: it was stored here
    /// before, or a session has attached.
    ready: Signal<bool>,
    /// Text of a replica discarded for foreign lineage, until the host
    /// takes it.
    discarded: Signal<Option<String>>,
    /// Why the server refused this person's attach, when it did.
    refused: Signal<Option<String>>,
}

impl ReplicaHandle {
    /// The document, once open.
    #[must_use]
    pub fn doc(&self) -> Option<CrdtDoc> {
        self.doc.read().clone()
    }
    /// Bumps on every committed change, local or remote. Reactive.
    #[must_use]
    pub fn revision(&self) -> u64 {
        (self.revision)()
    }
    /// The sync session. `Live` only after an attach landed. Reactive.
    #[must_use]
    pub fn status(&self) -> SyncStatus {
        (self.status)()
    }
    /// Holds changes the server has not acknowledged. Reactive.
    #[must_use]
    pub fn unsynced(&self) -> bool {
        (self.unsynced)()
    }
    /// Holds a version worth editing (stored before, or attached). Reactive.
    #[must_use]
    pub fn ready(&self) -> bool {
        (self.ready)()
    }
    /// The document carries a tombstone: its file was deleted, here or on
    /// any other node (`meta.deleted`). Reactive — through
    /// [`Self::revision`], so a delete arriving while the note is open
    /// shows at once.
    #[must_use]
    pub fn deleted(&self) -> bool {
        let _ = self.revision();
        self.doc
            .read()
            .as_ref()
            .is_some_and(|d| vault_doc::is_deleted(d.loro()))
    }
    /// The server's refusal, when there is one. Reactive.
    #[must_use]
    pub fn refused(&self) -> Option<String> {
        self.refused.read().clone()
    }
    /// The text of a replica the driver discarded (foreign lineage), once.
    pub fn take_discarded(&self) -> Option<String> {
        let mut d = self.discarded;
        if d.peek().is_some() { d.take() } else { None }
    }

    /// Back to the pre-open state — between drivers, so no reader sees
    /// the previous document. No-op writes are skipped.
    pub fn reset(&self) {
        let (mut doc, mut rev, mut status, mut unsynced, mut ready, mut discarded, mut refused) = (
            self.doc,
            self.revision,
            self.status,
            self.unsynced,
            self.ready,
            self.discarded,
            self.refused,
        );
        if doc.peek().is_some() {
            doc.set(None);
        }
        if *rev.peek() != 0 {
            rev.set(0);
        }
        if *status.peek() != SyncStatus::Connecting {
            status.set(SyncStatus::Connecting);
        }
        if *unsynced.peek() {
            unsynced.set(false);
        }
        if *ready.peek() {
            ready.set(false);
        }
        if discarded.peek().is_some() {
            discarded.set(None);
        }
        if refused.peek().is_some() {
            refused.set(None);
        }
    }
}

/// An empty slot owned by the calling (page) scope.
#[must_use]
pub fn use_replica_slot() -> ReplicaHandle {
    ReplicaHandle {
        doc: use_signal(|| None),
        revision: use_signal(|| 0),
        status: use_signal(|| SyncStatus::Connecting),
        unsynced: use_signal(|| false),
        ready: use_signal(|| false),
        discarded: use_signal(|| None),
        refused: use_signal(|| None),
    }
}

// ── Storage ────────────────────────────────────────────────────────

/// The device's document store for `slug`. Doc ids are derived from
/// `(vault_id, path)` alone, so stores are per org.
pub async fn doc_store(slug: &str) -> Result<SharedStore, String> {
    imp::doc_store(slug).await
}

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use std::sync::LazyLock;

    use futures_util::lock::Mutex;

    use super::{Arc, OrgScoped, SharedStore};

    /// The device database's document persistence, opened once.
    static DEVICE: LazyLock<Mutex<Option<Option<SharedStore>>>> =
        LazyLock::new(|| Mutex::new(None));

    /// Documents live in the device database (`crate::device_store`,
    /// crdt's `TursoPersistence`), keyed per org ([`OrgScoped`]); a
    /// directory of `FilePersistence` files per org is the fallback when
    /// that database cannot be opened.
    pub(super) async fn doc_store(slug: &str) -> Result<SharedStore, String> {
        let device = {
            let mut slot = DEVICE.lock().await;
            if slot.is_none() {
                let opened = crate::device_store::doc_persistence()
                    .await
                    .map(|p| SharedStore(Arc::new(p)));
                *slot = Some(opened);
            }
            slot.clone().flatten()
        };
        if let Some(device) = device {
            return Ok(SharedStore(Arc::new(OrgScoped {
                inner: device,
                slug: slug.to_owned(),
            })));
        }
        let dir = crate::server_registry::data_dir()
            .ok_or_else(|| "no app data directory".to_owned())?
            .join("documents")
            .join(safe(slug));
        Ok(SharedStore(Arc::new(crdt::FilePersistence::new(dir))))
    }

    fn safe(slug: &str) -> String {
        slug.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    }
}

/// One shared document store, partitioned by org: a vault doc id is a
/// function of `(vault_id, path)` only, so two orgs' `Notes/a.md` share
/// one — the stored key is that id namespaced by the org slug.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
struct OrgScoped {
    inner: SharedStore,
    slug: String,
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
impl OrgScoped {
    fn key(&self, doc_id: Uuid) -> Uuid {
        Uuid::new_v5(&doc_id, self.slug.as_bytes())
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[async_trait::async_trait]
impl crdt::Persistence for OrgScoped {
    async fn load_snapshot(&self, id: Uuid) -> Result<Option<Vec<u8>>, crdt::PersistError> {
        self.inner.load_snapshot(self.key(id)).await
    }
    async fn write_snapshot(&self, id: Uuid, bytes: &[u8]) -> Result<(), crdt::PersistError> {
        self.inner.write_snapshot(self.key(id), bytes).await
    }
    async fn append_update(&self, id: Uuid, bytes: &[u8]) -> Result<(), crdt::PersistError> {
        self.inner.append_update(self.key(id), bytes).await
    }
    async fn load_updates(&self, id: Uuid) -> Result<Vec<Vec<u8>>, crdt::PersistError> {
        self.inner.load_updates(self.key(id)).await
    }
    async fn compact(&self, id: Uuid, snapshot: &[u8]) -> Result<(), crdt::PersistError> {
        self.inner.compact(self.key(id), snapshot).await
    }
}

#[cfg(target_arch = "wasm32")]
mod imp {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use super::{Arc, SharedStore};

    thread_local! {
        static STORES: RefCell<HashMap<String, SharedStore>> = RefCell::new(HashMap::new());
    }

    #[allow(clippy::arc_with_non_send_sync)] // wasm is single-threaded
    pub(super) async fn doc_store(slug: &str) -> Result<SharedStore, String> {
        if let Some(s) = STORES.with(|m| m.borrow().get(slug).cloned()) {
            return Ok(s);
        }
        let db = crdt::IndexedDbPersistence::open(&format!("task-documents-{slug}"))
            .await
            .map_err(|e| e.to_string())?;
        let store = SharedStore(Arc::new(db));
        STORES.with(|m| m.borrow_mut().insert(slug.to_owned(), store.clone()));
        Ok(store)
    }
}

fn acked_key(slug: &str, doc_id: Uuid) -> String {
    format!("doc-acked:{slug}:{doc_id}")
}

fn load_acked(slug: &str, doc_id: Uuid) -> Option<VersionVector> {
    let hex = crate::local_store::get(&acked_key(slug, doc_id))?;
    let bytes = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
        .collect::<Option<Vec<u8>>>()?;
    VersionVector::decode(&bytes).ok()
}

fn save_acked(slug: &str, doc_id: Uuid, vv: &VersionVector) {
    use std::fmt::Write;
    let mut hex = String::new();
    for b in vv.encode() {
        let _ = write!(hex, "{b:02x}");
    }
    crate::local_store::set(&acked_key(slug, doc_id), &hex);
}

// ── Documents created here ─────────────────────────────────────────

fn local_key(slug: &str) -> String {
    format!("docs-local:{slug}")
}

/// Paths of documents created on this device that the server has not
/// seen yet, per vault: `vault_id → paths`. What lets a note made
/// offline open offline (the folder index cannot list it).
fn local_docs(slug: &str) -> HashMap<String, Vec<String>> {
    crate::local_store::get(&local_key(slug))
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default()
}

fn set_local_docs(slug: &str, docs: &HashMap<String, Vec<String>>) {
    if let Ok(j) = serde_json::to_string(docs) {
        crate::local_store::set(&local_key(slug), &j);
    }
}

/// A document for `(vault_id, path)` was created on this device and has
/// not reached the server yet.
#[must_use]
pub fn is_local_doc(slug: &str, vault_id: &str, path: &str) -> bool {
    local_docs(slug)
        .get(vault_id)
        .is_some_and(|ps| ps.iter().any(|p| p == path))
}

fn forget_local_doc(slug: &str, vault_id: &str, path: &str) {
    let mut docs = local_docs(slug);
    if let Some(ps) = docs.get_mut(vault_id) {
        let before = ps.len();
        ps.retain(|p| p != path);
        if ps.len() != before {
            set_local_docs(slug, &docs);
        }
    }
}

/// A path for a new note no other device will pick: the title plus a
/// short random tag (`Untitled 3f9a.md`).
#[must_use]
pub fn device_note_path(title: &str) -> String {
    let tag = Uuid::new_v4().simple().to_string();
    format!("{title} {}.md", &tag[..4])
}

/// Create a note's document on this device, with no server
/// (`offline.doc.create`): stored at once under `path`, with an `id`
/// property naming it, and remembered as local until its first sync.
// t[impl offline.doc.create]
pub async fn create_local_note(slug: &str, vault_id: &str, path: &str) -> Result<(), String> {
    let store = doc_store(slug).await?;
    let doc_id = vault_proto::collab_doc_id(vault_id, path);
    let replica = Replica::open(doc_id, store, None)
        .await
        .map_err(|e| e.to_string())?;
    if replica.has_history() {
        return Err(format!("{path} already has a document on this device"));
    }
    vault_doc::doc::set_prop(
        replica.doc().loro(),
        vault_doc::ID_PROP,
        crdt::loro::LoroValue::from(Uuid::new_v4().to_string()),
    );
    vault_doc::commit_as(replica.doc().loro(), "created on this device");
    // Let the store's writer land the commit before the replica drops.
    for _ in 0..50 {
        let stored = crdt::Persistence::load_updates(&doc_store(slug).await?, doc_id)
            .await
            .map(|u| !u.is_empty())
            .unwrap_or(false);
        if stored {
            break;
        }
        architect::sleep(Duration::from_millis(10)).await;
    }
    let mut docs = local_docs(slug);
    docs.entry(vault_id.to_owned())
        .or_default()
        .push(path.to_owned());
    set_local_docs(slug, &docs);
    Ok(())
}

/// What the device's stored document for `(vault_id, path)` renders,
/// when there is one — for opening a note the server can't serve right
/// now.
pub async fn local_text(slug: &str, vault_id: &str, path: &str) -> Option<String> {
    let store = doc_store(slug).await.ok()?;
    let doc_id = vault_proto::collab_doc_id(vault_id, path);
    let replica = Replica::open(doc_id, store, None).await.ok()?;
    replica
        .has_history()
        .then(|| vault_doc::render(replica.doc().loro(), None))
}

// ── The driver ─────────────────────────────────────────────────────

// t[impl offline.doc.replica] — the device keeps the document and keeps
// syncing it; nothing is torn down offline
// t[impl offline.visible] — `unsynced` is the per-document truth the
// editors show
/// Drive `handle` for the file `(vault_id, path)` of org `slug` for the
/// calling scope's lifetime. Mount keyed by the doc id; the slot is
/// reset on mount.
pub fn use_replica_into(handle: ReplicaHandle, slug: String, vault_id: String, path: String) {
    use_hook(|| handle.reset());
    let ReplicaHandle {
        doc: mut doc_sig,
        revision,
        mut status,
        mut unsynced,
        mut ready,
        mut discarded,
        refused,
    } = handle;

    use_future(move || {
        let (slug, vault_id, path) = (slug.clone(), vault_id.clone(), path.clone());
        async move {
            let doc_id = vault_proto::collab_doc_id(&vault_id, &path);
            let store = match doc_store(&slug).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!(?doc_id, "doc replica: no store: {e}");
                    status.set(SyncStatus::Offline);
                    return;
                }
            };
            let mut replica = match Replica::open(doc_id, store, load_acked(&slug, doc_id)).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::error!(?doc_id, "doc replica: open failed: {e}");
                    status.set(SyncStatus::Offline);
                    return;
                }
            };
            let mut backoff = architect::Backoff::default();
            loop {
                // (Re)publish the document — again after a reset swapped
                // it. The change bridge lives in `drive`.
                ready.set(replica.has_history());
                unsynced.set(replica.unsynced());
                doc_sig.set(Some(replica.doc().clone()));

                let end = drive(
                    &mut replica,
                    &slug,
                    &vault_id,
                    &path,
                    &mut backoff,
                    status,
                    unsynced,
                    ready,
                    refused,
                    revision,
                )
                .await;
                if end == SessionEnd::ForeignLineage {
                    match replica.reset().await {
                        Ok(text) => discarded.set(Some(text)),
                        Err(e) => {
                            tracing::error!(?doc_id, "doc replica: reset failed: {e}");
                            return;
                        }
                    }
                    save_acked(&slug, doc_id, replica.acked());
                    continue;
                }
                return;
            }
        }
    });
}

/// Sessions until the scope drops (never returns) or a foreign lineage
/// asks the caller to reset.
#[allow(clippy::too_many_arguments)]
async fn drive(
    replica: &mut Replica,
    slug: &str,
    vault_id: &str,
    path: &str,
    backoff: &mut architect::Backoff,
    mut status: Signal<SyncStatus>,
    mut unsynced: Signal<bool>,
    ready: Signal<bool>,
    mut refused: Signal<Option<String>>,
    revision: Signal<u64>,
) -> SessionEnd {
    let doc_id = replica.doc_id();
    // Local commits bump the revision and the unsynced verdict between
    // sessions too: a root subscription bridged into this task.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    let _sub = replica.doc().loro().subscribe_root(Arc::new(move |_| {
        let _ = tx.send(());
    }));
    let doc = replica.doc().clone();
    let mut acked = replica.acked().clone();
    loop {
        // Between sessions: wait out the backoff while still reporting
        // local edits.
        let attempt = async {
            let vault = crate::vox_clients::vault_client(slug).await?;
            vault
                .open_collab(vault_id.to_owned(), path.to_owned())
                .await
                .map_err(|e| match e {
                    vox::VoxError::User(u) => format!("refused: {u:?}"),
                    other => format!("{other:?}"),
                })?;
            crate::vox_clients::establish_for::<crdt::sync::DocSyncClient>(slug).await
        };
        let client = tokio::select! {
            c = attempt => c,
            () = pump(&mut rx, &doc, Some(&acked), unsynced, revision) => unreachable!(),
        };
        let client = match client {
            Ok(c) => c,
            Err(e) => {
                if e.starts_with("refused") {
                    if refused.peek().as_deref() != Some(e.as_str()) {
                        refused.set(Some(e));
                    }
                } else {
                    tracing::debug!(?doc_id, "doc replica: offline: {e}");
                }
                if *status.peek() != SyncStatus::Offline {
                    status.set(SyncStatus::Offline);
                }
                tokio::select! {
                    () = architect::sleep(backoff.next_delay()) => {}
                    () = pump(&mut rx, &doc, Some(&acked), unsynced, revision) => {}
                }
                continue;
            }
        };
        if refused.peek().is_some() {
            refused.set(None);
        }
        let end = {
            let (mut st, mut rd, mut un) = (status, ready, unsynced);
            let bo = &mut *backoff;
            let session = replica.run(&client, move |ev| match ev {
                SessionEvent::Attached => {
                    bo.reset();
                    st.set(SyncStatus::Live);
                    if !*rd.peek() {
                        rd.set(true);
                    }
                }
                SessionEvent::Acked { unsynced: now } => {
                    if now != *un.peek() {
                        un.set(now);
                    }
                }
            });
            // While a session runs its own acknowledgement events say
            // what is unsynced; the bridge only bumps the revision.
            tokio::select! {
                end = session => end,
                () = pump(&mut rx, &doc, None, unsynced, revision) => unreachable!(),
            }
        };
        acked = replica.acked().clone();
        save_acked(slug, doc_id, &acked);
        let now = replica.unsynced();
        if now != *unsynced.peek() {
            unsynced.set(now);
        }
        if !replica.unsynced() {
            forget_local_doc(slug, vault_id, path);
        }
        match end {
            SessionEnd::ForeignLineage => return end,
            SessionEnd::Refused(why) => {
                refused.set(Some(why));
            }
            SessionEnd::Dropped => {}
        }
        if *status.peek() != SyncStatus::Offline {
            status.set(SyncStatus::Offline);
        }
        tokio::select! {
            () = architect::sleep(backoff.next_delay()) => {}
            () = pump(&mut rx, &doc, Some(&acked), unsynced, revision) => {}
        }
    }
}

/// Forward change notifications into the revision + unsynced signals.
/// Never finishes on its own (the subscription lives as long as `rx`).
async fn pump(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<()>,
    doc: &CrdtDoc,
    acked: Option<&VersionVector>,
    mut unsynced: Signal<bool>,
    mut revision: Signal<u64>,
) {
    while rx.recv().await.is_some() {
        let next = revision.peek().wrapping_add(1);
        revision.set(next);
        if let Some(acked) = acked {
            let now = vault_doc::replica::unsynced(doc.loro(), acked);
            if now != *unsynced.peek() {
                unsynced.set(now);
            }
        }
    }
    std::future::pending::<()>().await;
}
