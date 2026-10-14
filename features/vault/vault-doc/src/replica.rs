//! A document replica that lives on the device, and its sync session
//! with the server's document.
//!
//! [`Replica`] is one vault file's Loro document opened over a
//! [`crdt::Persistence`] (whatever the device stores documents in — the
//! seam the local Turso store implements), plus:
//!
//! - an **outbox** of local updates, buffered while there is no session
//!   (offline) and drained when one attaches; a replica reopened after a
//!   restart has an empty outbox but still holds its ops, and the attach
//!   pushes back whatever the server's version vector lacks;
//! - **durable imports**: what arrives from the server is written through
//!   the same persistence, so a replica restarted offline still has the
//!   history its own edits were made on top of;
//! - **acknowledgement tracking**: the server echoes every update it
//!   applies to every attached replica, the sender included, so the union
//!   of the server's version vector at attach and the end of every update
//!   received since is exactly what the server is known to hold —
//!   [`Replica::unsynced`] is "this replica holds ops outside it". The
//!   app never says synced when it is not;
//! - a **lineage check** ([`is_foreign_lineage`]): a replica whose history
//!   shares no peer with the server's document (the server's document was
//!   recreated — a fresh data root, a reseeded demo) must not merge, or
//!   every line would appear twice. The server refuses such an attach
//!   with [`FOREIGN_LINEAGE`]; the replica reports
//!   [`SessionEnd::ForeignLineage`] and its host decides what to keep
//!   ([`Replica::reset`]).
//!
//! No Dioxus, no executor: [`Replica::run`] is one session as a future,
//! driven by the shell's hook on the device and by tests directly.

use std::sync::Arc;

use async_trait::async_trait;
use crdt::loro::{ExportMode, LoroDoc, VersionVector};
use crdt::sync::{DocSyncClient, SyncDown, SyncError};
use crdt::{CrdtDoc, PersistError, Persistence};
use uuid::Uuid;

/// Prefix of the [`SyncError::BadVersion`] message a server answers a
/// foreign-lineage attach with.
pub const FOREIGN_LINEAGE: &str = "foreign-lineage";

/// Compact the device's update log after this many durable writes.
const COMPACT_EVERY: u32 = 64;

/// A replica's history and the server's share no peer: they are two
/// different documents that happen to have the same id. Either side
/// being empty is never foreign (a fresh server document for a file
/// first created offline, or a fresh replica).
#[must_use]
pub fn is_foreign_lineage(client: &VersionVector, server: &VersionVector) -> bool {
    let nonzero = |vv: &VersionVector| vv.iter().any(|(_, c)| *c > 0);
    if !nonzero(client) || !nonzero(server) {
        return false;
    }
    !client
        .iter()
        .any(|(peer, c)| *c > 0 && server.get(peer).is_some_and(|s| *s > 0))
}

/// A shared [`Persistence`] handle, so one store (one database) can back
/// every replica and still be handed to [`CrdtDoc::open`] by value.
#[derive(Clone)]
pub struct SharedStore(pub Arc<dyn Persistence>);

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl Persistence for SharedStore {
    async fn load_snapshot(&self, doc_id: Uuid) -> Result<Option<Vec<u8>>, PersistError> {
        self.0.load_snapshot(doc_id).await
    }
    async fn write_snapshot(&self, doc_id: Uuid, bytes: &[u8]) -> Result<(), PersistError> {
        self.0.write_snapshot(doc_id, bytes).await
    }
    async fn append_update(&self, doc_id: Uuid, bytes: &[u8]) -> Result<(), PersistError> {
        self.0.append_update(doc_id, bytes).await
    }
    async fn load_updates(&self, doc_id: Uuid) -> Result<Vec<Vec<u8>>, PersistError> {
        self.0.load_updates(doc_id).await
    }
    async fn compact(&self, doc_id: Uuid, snapshot: &[u8]) -> Result<(), PersistError> {
        self.0.compact(doc_id, snapshot).await
    }
}

/// Something a replica can hold a sync session over: the `DocSync` call,
/// whoever answers it. A device's replica dials the server's
/// [`DocSyncClient`]; a node exchanging documents with a peer dials the
/// peer's document lane (`crate::peer::PeerDoc`) and its own registry —
/// one session shape, so every guarantee here (the outbox, durable
/// imports, acknowledgement, the lineage refusal) holds on both.
pub trait SyncLane {
    fn sync(
        &self,
        doc_id: Uuid,
        from: Vec<u8>,
        up: vox::Rx<Vec<u8>>,
        down: vox::Tx<SyncDown>,
    ) -> impl std::future::Future<Output = Result<(), vox::VoxError<SyncError>>>
    + crdt::architect::MaybeSend;
}

impl SyncLane for DocSyncClient {
    async fn sync(
        &self,
        doc_id: Uuid,
        from: Vec<u8>,
        up: vox::Rx<Vec<u8>>,
        down: vox::Tx<SyncDown>,
    ) -> Result<(), vox::VoxError<SyncError>> {
        DocSyncClient::sync(self, doc_id, from, up, down).await
    }
}

/// How a session ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEnd {
    /// The connection went (or never attached): edits keep buffering,
    /// run again when it is back.
    Dropped,
    /// The server refused the attach (permission, unknown doc). Running
    /// again will be refused the same way until something changes.
    Refused(String),
    /// The server's document is a different lineage — see
    /// [`is_foreign_lineage`].
    ForeignLineage,
}

/// What a session reports as it goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionEvent {
    /// The attach landed: the server's history is merged in.
    Attached,
    /// The acknowledged state moved; `unsynced` is the new verdict.
    Acked { unsynced: bool },
}

// t[impl offline.doc.replica] — persisted, outbox-buffered, durable
// imports, acknowledgement-tracked
/// One vault file's document, persisted on the device. See the module
/// docs.
pub struct Replica {
    doc: CrdtDoc,
    doc_id: Uuid,
    store: SharedStore,
    outbox: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
    _local: crdt::loro::Subscription,
    acked: VersionVector,
    writes: u32,
}

impl Replica {
    /// Open `doc_id` from `store` (empty when it has never been stored).
    /// `acked` is the acknowledged version vector last recorded for it
    /// ([`Self::acked`]), or `None` — then nothing it holds counts as
    /// acknowledged until a session attaches.
    pub async fn open(
        doc_id: Uuid,
        store: SharedStore,
        acked: Option<VersionVector>,
    ) -> Result<Self, PersistError> {
        let doc = CrdtDoc::open(doc_id, store.clone()).await?;
        Ok(Self::wire(doc, doc_id, store, acked.unwrap_or_default()))
    }

    fn wire(doc: CrdtDoc, doc_id: Uuid, store: SharedStore, acked: VersionVector) -> Self {
        let (tx, outbox) = tokio::sync::mpsc::unbounded_channel();
        let local = doc.loro().subscribe_local_update(Box::new(move |bytes| {
            let _ = tx.send(bytes.clone());
            true
        }));
        Self {
            doc,
            doc_id,
            store,
            outbox,
            _local: local,
            acked,
            writes: 0,
        }
    }

    /// The document (cheap clone).
    #[must_use]
    pub fn doc(&self) -> &CrdtDoc {
        &self.doc
    }

    #[must_use]
    pub const fn doc_id(&self) -> Uuid {
        self.doc_id
    }

    /// The device holds some history of this document — it was synced, or
    /// created here, before.
    #[must_use]
    pub fn has_history(&self) -> bool {
        self.doc.loro().oplog_vv().iter().any(|(_, c)| *c > 0)
    }

    /// What the server is known to hold.
    #[must_use]
    pub const fn acked(&self) -> &VersionVector {
        &self.acked
    }

    /// The replica holds changes the server has not acknowledged.
    #[must_use]
    pub fn unsynced(&self) -> bool {
        unsynced(self.doc.loro(), &self.acked)
    }

    /// Forget everything stored for this document and start from an empty
    /// one (after [`SessionEnd::ForeignLineage`]). Returns the text the
    /// discarded replica rendered, for the host to fold back in if the
    /// person had typed it.
    pub async fn reset(&mut self) -> Result<String, PersistError> {
        let previous = crate::doc::render(self.doc.loro(), None);
        let empty = LoroDoc::new()
            .export(ExportMode::Snapshot)
            .map_err(|e| PersistError::Backend(format!("export empty: {e}")))?;
        self.store.compact(self.doc_id, &empty).await?;
        let doc = CrdtDoc::open(self.doc_id, self.store.clone()).await?;
        *self = Self::wire(doc, self.doc_id, self.store.clone(), VersionVector::new());
        Ok(previous)
    }

    async fn import_durable(&mut self, bytes: &[u8]) -> Result<(), PersistError> {
        self.doc.apply_remote_durable(self.doc_id, bytes).await?;
        self.writes += 1;
        if self.writes >= COMPACT_EVERY {
            self.writes = 0;
            self.doc.compact(self.doc_id).await?;
        }
        Ok(())
    }

    /// Run one sync session until the connection drops or the server
    /// refuses. Local edits made before or during it go up; the server's
    /// come down durably; `on_event` hears the attach and every change in
    /// the acknowledged state.
    pub async fn run(
        &mut self,
        client: &impl SyncLane,
        on_event: impl FnMut(SessionEvent),
    ) -> SessionEnd {
        match self.session(client, on_event, None, false).await {
            Ok(()) => SessionEnd::Dropped,
            Err(end) => end,
        }
    }

    /// One session that ends as soon as the server holds everything this
    /// replica has: `Ok` then, else how it ended. `on_attached` runs once
    /// the server's history is merged in and before anything is judged
    /// synced — the moment to apply an edit that had to wait for the
    /// server's version (a change made offline to a document this device
    /// did not hold yet); what it commits goes up in the same session.
    pub async fn sync_once(
        &mut self,
        client: &impl SyncLane,
        on_attached: impl FnOnce(&LoroDoc) + Send + 'static,
    ) -> Result<(), SessionEnd> {
        self.session(client, |_| {}, Some(Box::new(on_attached)), true)
            .await
    }

    async fn session(
        &mut self,
        client: &impl SyncLane,
        mut on_event: impl FnMut(SessionEvent),
        mut on_attached: Option<Box<dyn FnOnce(&LoroDoc) + Send>>,
        until_synced: bool,
    ) -> Result<(), SessionEnd> {
        let (up_tx, up_rx) = vox::channel::<Vec<u8>>();
        let (down_tx, mut down_rx) = vox::channel::<SyncDown>();
        let our_vv = self.doc.loro().oplog_vv();
        let doc_id = self.doc_id;
        let call = client.sync(doc_id, our_vv.encode(), up_rx, down_tx);
        let mut call = std::pin::pin!(call);
        let mut was_unsynced = self.unsynced();
        let mut attached = false;

        loop {
            tokio::select! {
                // The call first, always: it sends nothing until polled,
                // and its request is what binds `up_tx`. Polled in random
                // order, a replica with edits already waiting (a document
                // made offline) could take the outbox branch first and
                // await a send on an unbound channel while the request
                // that would bind it never went out — a session that
                // hung before saying a word, about half the time.
                biased;
                res = &mut call => return Err(classify(res)),
                Some(update) = self.outbox.recv() => {
                    if up_tx.send(update).await.is_err() {
                        return Err(SessionEnd::Dropped);
                    }
                }
                incoming = down_rx.recv() => {
                    let mut owned: Option<SyncDown> = None;
                    match incoming {
                        Ok(Some(frame)) => {
                            let _ = frame.map(|f| owned = Some(f));
                        }
                        // The stream ended: a refused attach delivers its
                        // error as the response, which also closes the
                        // channels — read the call to say which it was.
                        _ => return Err(classify(call.await)),
                    }
                    match owned {
                        Some(SyncDown::Attached { server_vv, backlog }) => {
                            if !backlog.is_empty() && self.import_durable(&backlog).await.is_err() {
                                return Err(SessionEnd::Dropped);
                            }
                            let Ok(server_vv) = VersionVector::decode(&server_vv) else {
                                return Err(SessionEnd::Dropped);
                            };
                            if !server_vv.includes_vv(&our_vv) {
                                // History the server lacks (edited offline,
                                // restarted): push exactly that delta.
                                let missing = self.doc.loro().export(ExportMode::Updates {
                                    from: std::borrow::Cow::Owned(server_vv.clone()),
                                });
                                match missing {
                                    Ok(bytes) => {
                                        if up_tx.send(bytes).await.is_err() {
                                            return Err(SessionEnd::Dropped);
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!(?doc_id, "vault-doc: export delta: {e}");
                                    }
                                }
                            }
                            self.acked.merge(&server_vv);
                            attached = true;
                            if let Some(apply) = on_attached.take() {
                                // Committed here, so the local-update
                                // subscription queues it for this session.
                                apply(self.doc.loro());
                            }
                            on_event(SessionEvent::Attached);
                        }
                        Some(SyncDown::Update(bytes)) => {
                            if self.import_durable(&bytes).await.is_err() {
                                return Err(SessionEnd::Dropped);
                            }
                            // The server publishes every update it applied —
                            // ours included — so what comes down is held there.
                            if let Ok(meta) = LoroDoc::decode_import_blob_meta(&bytes, false) {
                                self.acked.merge(&meta.partial_end_vv);
                            }
                        }
                        None => continue,
                    }
                    let now = self.unsynced();
                    if now != was_unsynced {
                        was_unsynced = now;
                        on_event(SessionEvent::Acked { unsynced: now });
                    }
                    if until_synced && attached && !now {
                        return Ok(());
                    }
                }
            }
        }
    }
}

/// How a finished `sync` call ended the session.
fn classify(res: Result<(), vox::VoxError<SyncError>>) -> SessionEnd {
    match res {
        Ok(()) => SessionEnd::Dropped,
        Err(vox::VoxError::User(e)) => match *e {
            SyncError::BadVersion(m) if m.starts_with(FOREIGN_LINEAGE) => {
                SessionEnd::ForeignLineage
            }
            other => SessionEnd::Refused(other.to_string()),
        },
        Err(_) => SessionEnd::Dropped,
    }
}

/// `doc` holds ops outside `acked`.
#[must_use]
pub fn unsynced(doc: &LoroDoc, acked: &VersionVector) -> bool {
    !acked.includes_vv(&doc.oplog_vv())
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    fn store(dir: &std::path::Path) -> SharedStore {
        SharedStore(Arc::new(crdt::FilePersistence::new(dir)))
    }

    async fn settle(store: &SharedStore, id: Uuid, want: usize) {
        for _ in 0..200 {
            if store.load_updates(id).await.unwrap().len() >= want {
                return;
            }
            tokio::task::yield_now().await;
        }
    }

    #[test]
    fn lineage_is_foreign_only_when_nothing_is_shared() {
        let a = LoroDoc::new();
        a.get_text("t").insert(0, "x").unwrap();
        a.commit();
        let b = LoroDoc::new();
        b.get_text("t").insert(0, "y").unwrap();
        b.commit();
        let (va, vb) = (a.oplog_vv(), b.oplog_vv());
        assert!(is_foreign_lineage(&va, &vb));
        assert!(!is_foreign_lineage(&va, &VersionVector::new()));
        assert!(!is_foreign_lineage(&VersionVector::new(), &vb));
        // Once b has a's history they share a lineage.
        b.import(&a.export(ExportMode::Snapshot).unwrap()).unwrap();
        assert!(!is_foreign_lineage(&va, &b.oplog_vv()));
    }

    // t[verify offline.doc.replica] — a replica reopened from storage
    // ("a restart") keeps its unsynced edits, on top of the history it
    // was synced with
    #[tokio::test(flavor = "multi_thread")]
    async fn a_reopened_replica_keeps_unsynced_edits() {
        let dir = tempfile::tempdir().unwrap();
        let st = store(dir.path());
        let id = Uuid::new_v4();
        // A "server" document the replica first synced.
        let server = LoroDoc::new();
        crate::doc::adopt_text(&server, "---\ntitle: Mix\n---\nhello\n");
        server.commit();
        let acked;
        {
            let mut r = Replica::open(id, st.clone(), None).await.unwrap();
            // What a session's backlog import does.
            r.import_durable(&server.export(ExportMode::Snapshot).unwrap())
                .await
                .unwrap();
            r.acked.merge(&server.oplog_vv());
            assert!(!r.unsynced());
            acked = r.acked().clone();
            // Offline edit.
            crate::doc::merge_text(
                r.doc().loro(),
                "---\ntitle: Mix\n---\nhello\n",
                "---\ntitle: Mix\n---\nhello offline\n",
            );
            r.doc().loro().commit();
            assert!(r.unsynced());
            settle(&st, id, 2).await;
        }
        let r = Replica::open(id, st.clone(), Some(acked)).await.unwrap();
        assert_eq!(
            crate::doc::render(r.doc().loro(), None),
            "---\ntitle: Mix\n---\nhello offline\n"
        );
        assert!(r.unsynced(), "the restart did not forget what is unsynced");
        assert!(r.has_history());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn reset_forgets_the_stored_document() {
        let dir = tempfile::tempdir().unwrap();
        let st = store(dir.path());
        let id = Uuid::new_v4();
        let mut r = Replica::open(id, st.clone(), None).await.unwrap();
        crate::doc::adopt_text(r.doc().loro(), "old lineage\n");
        r.doc().loro().commit();
        settle(&st, id, 1).await;
        let was = r.reset().await.unwrap();
        assert_eq!(was, "old lineage\n");
        assert!(!r.has_history());
        let again = Replica::open(id, st, None).await.unwrap();
        assert!(!again.has_history());
    }
}
