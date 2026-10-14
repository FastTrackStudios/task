//! The device's record book: authored records — tasks, inbox captures —
//! kept as documents on the device, listed from an index projected from
//! them, and synced with the server document by document
//! (`docs/spec/offline.md`: `offline.doc.create`, `offline.doc.replica`,
//! `offline.replica`).
//!
//! A [`Book`] is one org's: its documents live in a [`SharedStore`] (the
//! device database), its rows in an [`Index`] (the device's `LocalRows`
//! on the shell, [`MemoryIndex`] in tests), and what a record *is* — its
//! kind, its key, its row — is the domain's [`Projector`], so this module
//! names no task and no capture. Everything here is text-level: the
//! domain hands it file texts (`task_proto::doc`, `inbox_proto::doc`) and
//! the book merges them into documents as changes.
//!
//! Three moves, none of which waits for a server:
//!
//! - [`Book::create`] — a new record is a new document at the path the
//!   device chose, stored before it returns, and in the index at once;
//! - [`Book::edit`] — an edit is a change: merged into the document when
//!   the device holds it; when it does not yet (a task first seen in a
//!   list, edited on a train), the edit **waits** in the index as the two
//!   texts it was made between, and is merged into the server's version
//!   the moment a session attaches — so it is still only what the person
//!   changed, never "this device's copy wins";
//! - [`Book::sync`] — every record with something the server has not
//!   acknowledged, one session each ([`Replica::sync_once`]), until the
//!   server holds it. A connection that is not there stops the pass (the
//!   next one resumes); a refusal is recorded on the row and the record
//!   stays on the device, never dropped (`offline.doc.access`).
//!
//! Index rows are owned by their document (`<scope>/<doc id>`), so a
//! projection replaces exactly what that document said before.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use crdt::loro::{ExportMode, LoroDoc};
use crdt::sync::DocSyncClient;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use vault_proto::VaultSyncClient;

use crate::replica::{Replica, SessionEnd, SharedStore};

/// `Send + Sync` on native, nothing on wasm.
#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSendSync: Send + Sync {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Send + Sync> MaybeSendSync for T {}
/// `Send + Sync` on native, nothing on wasm.
#[cfg(target_arch = "wasm32")]
pub trait MaybeSendSync {}
#[cfg(target_arch = "wasm32")]
impl<T> MaybeSendSync for T {}

/// One index row: its key within `(kind, scope)`, its JSON, and the
/// document that projected it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexRow {
    pub key: String,
    pub owner: Option<String>,
    pub json: String,
}

/// Where a device keeps the rows its documents project — the shell's
/// `architect::LocalRows`, behind a two-call seam.
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub trait Index: MaybeSendSync + 'static {
    /// Replace everything `owner` projected with `rows` (`(kind, scope,
    /// row)` each).
    async fn project(
        &self,
        owner: &str,
        rows: Vec<(String, String, IndexRow)>,
    ) -> Result<(), String>;
    /// The rows of `(kind, scope)` (empty when none were ever stored).
    async fn load(&self, kind: &str, scope: &str) -> Result<Vec<IndexRow>, String>;
}

/// An [`Index`] in memory — tests, and a device with no database.
#[derive(Default)]
pub struct MemoryIndex {
    rows: Mutex<BTreeMap<(String, String), Vec<IndexRow>>>,
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl Index for MemoryIndex {
    async fn project(
        &self,
        owner: &str,
        rows: Vec<(String, String, IndexRow)>,
    ) -> Result<(), String> {
        let mut map = self.rows.lock().map_err(|_| "index lock poisoned")?;
        for set in map.values_mut() {
            set.retain(|r| r.owner.as_deref() != Some(owner));
        }
        for (kind, scope, mut row) in rows {
            row.owner = Some(owner.to_owned());
            let set = map.entry((kind, scope)).or_default();
            set.retain(|r| r.key != row.key);
            set.push(row);
        }
        Ok(())
    }

    async fn load(&self, kind: &str, scope: &str) -> Result<Vec<IndexRow>, String> {
        let map = self.rows.lock().map_err(|_| "index lock poisoned")?;
        Ok(map
            .get(&(kind.to_owned(), scope.to_owned()))
            .cloned()
            .unwrap_or_default())
    }
}

/// What a record's text is in the index: its kind (`"task-docs"`), its
/// key (the task id) and its row (the record as JSON).
#[derive(Debug, Clone, PartialEq)]
pub struct Projected {
    pub kind: String,
    pub key: String,
    pub record: serde_json::Value,
}

/// The domain's reading of a record document: `(path, text)` → its row,
/// or `None` when the text is not (or no longer) such a record.
#[cfg(not(target_arch = "wasm32"))]
pub type Projector = Arc<dyn Fn(&str, &str) -> Option<Projected> + Send + Sync>;
/// The domain's reading of a record document (see the native alias).
#[cfg(target_arch = "wasm32")]
pub type Projector = Arc<dyn Fn(&str, &str) -> Option<Projected>>;

/// An edit made to a document the device did not hold yet, waiting to
/// be merged into the server's version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Waiting {
    /// The text the edit was made from.
    pub base: String,
    /// The text it made.
    pub next: String,
    /// Who/what made it — the change's commit message.
    pub origin: String,
}

/// One record document as the index holds it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// The vault-relative file the document is for.
    pub path: String,
    /// The record (the domain's JSON).
    pub record: serde_json::Value,
    /// Holds changes the server has not acknowledged.
    pub unsynced: bool,
    /// An edit waiting for the server's version.
    #[serde(default)]
    pub waiting: Option<Waiting>,
    /// Why the server refused this document, when it did. The record
    /// stays on the device (`offline.doc.access`).
    #[serde(default)]
    pub refused: Option<String>,
}

/// What a [`Book::sync`] pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncReport {
    /// Documents the server now holds in full.
    pub synced: usize,
    /// Documents refused (kept, marked).
    pub refused: Vec<(String, String)>,
    /// The connection went before the pass finished.
    pub interrupted: bool,
}

struct Inner {
    store: SharedStore,
    index: Arc<dyn Index>,
    scope: String,
    vault_id: String,
    kinds: Vec<String>,
    project: Projector,
    /// One document operation at a time: an edit never lands in a copy
    /// a sync session is not looking at.
    lock: tokio::sync::Mutex<()>,
}

/// One org's record book on this device. Cheap to clone.
#[derive(Clone)]
pub struct Book {
    inner: Arc<Inner>,
}

impl Book {
    /// A book over `store` and `index` for org `scope`, whose records live
    /// in vault `vault_id` and project as one of `kinds` through `project`.
    #[must_use]
    pub fn new(
        store: SharedStore,
        index: Arc<dyn Index>,
        scope: impl Into<String>,
        vault_id: impl Into<String>,
        kinds: &[&str],
        project: Projector,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                store,
                index,
                scope: scope.into(),
                vault_id: vault_id.into(),
                kinds: kinds.iter().map(|k| (*k).to_owned()).collect(),
                project,
                lock: tokio::sync::Mutex::new(()),
            }),
        }
    }

    /// The org this book is for.
    #[must_use]
    pub fn scope(&self) -> &str {
        &self.inner.scope
    }

    /// The document id of `path`.
    #[must_use]
    pub fn doc_id(&self, path: &str) -> Uuid {
        vault_proto::collab_doc_id(&self.inner.vault_id, path)
    }

    fn owner(&self, doc_id: Uuid) -> String {
        format!("{}/{doc_id}", self.inner.scope)
    }

    async fn open(&self, path: &str) -> Result<Replica, String> {
        Replica::open(self.doc_id(path), self.inner.store.clone(), None)
            .await
            .map_err(|e| e.to_string())
    }

    /// Commit `doc` under `origin` and write what the commit made through
    /// the store before returning — so a record made or changed here is on
    /// the device when the call is, a relaunch a moment later included.
    async fn commit_durable(
        &self,
        doc_id: Uuid,
        doc: &LoroDoc,
        origin: &str,
    ) -> Result<(), String> {
        let before = doc.oplog_vv();
        crate::doc::commit_as(doc, origin);
        if doc.oplog_vv() == before {
            return Ok(());
        }
        let bytes = doc
            .export(ExportMode::Updates {
                from: std::borrow::Cow::Owned(before),
            })
            .map_err(|e| format!("export: {e}"))?;
        crdt::Persistence::append_update(&self.inner.store, doc_id, &bytes)
            .await
            .map_err(|e| e.to_string())
    }

    /// Every entry of every kind this book projects.
    pub async fn entries(&self) -> Result<Vec<Entry>, String> {
        let mut out = Vec::new();
        for kind in &self.inner.kinds {
            for row in self.inner.index.load(kind, &self.inner.scope).await? {
                if let Ok(e) = serde_json::from_str::<Entry>(&row.json) {
                    out.push(e);
                }
            }
        }
        Ok(out)
    }

    /// The entries of `kind` (e.g. every task this device holds).
    pub async fn entries_of(&self, kind: &str) -> Result<Vec<Entry>, String> {
        Ok(self
            .inner
            .index
            .load(kind, &self.inner.scope)
            .await?
            .iter()
            .filter_map(|r| serde_json::from_str::<Entry>(&r.json).ok())
            .collect())
    }

    async fn entry_at(&self, path: &str) -> Result<Option<Entry>, String> {
        let owner = self.owner(self.doc_id(path));
        for kind in &self.inner.kinds {
            for row in self.inner.index.load(kind, &self.inner.scope).await? {
                if row.owner.as_deref() == Some(owner.as_str()) {
                    return Ok(serde_json::from_str(&row.json).ok());
                }
            }
        }
        Ok(None)
    }

    // t[impl offline.replica] — the index row is a projection of the
    // document, replaced whole per document
    async fn reproject(
        &self,
        path: &str,
        text: &str,
        unsynced: bool,
        waiting: Option<Waiting>,
        refused: Option<String>,
    ) -> Result<Option<Entry>, String> {
        let owner = self.owner(self.doc_id(path));
        let Some(p) = (self.inner.project)(path, text) else {
            self.inner.index.project(&owner, Vec::new()).await?;
            return Ok(None);
        };
        let entry = Entry {
            path: path.to_owned(),
            record: p.record,
            unsynced,
            waiting,
            refused,
        };
        let json = serde_json::to_string(&entry).map_err(|e| e.to_string())?;
        self.inner
            .index
            .project(
                &owner,
                vec![(
                    p.kind,
                    self.inner.scope.clone(),
                    IndexRow {
                        key: p.key,
                        owner: None,
                        json,
                    },
                )],
            )
            .await?;
        Ok(Some(entry))
    }

    /// The record's text as this device has it: the document's, else the
    /// text an edit waiting for it made.
    pub async fn text(&self, path: &str) -> Result<Option<String>, String> {
        let _g = self.inner.lock.lock().await;
        let replica = self.open(path).await?;
        if replica.has_history() {
            return Ok(Some(crate::doc::render(replica.doc().loro(), None)));
        }
        Ok(self
            .entry_at(path)
            .await?
            .and_then(|e| e.waiting.map(|w| w.next)))
    }

    /// The device holds `path`'s document (created here, or synced).
    pub async fn holds(&self, path: &str) -> Result<bool, String> {
        let _g = self.inner.lock.lock().await;
        Ok(self.open(path).await?.has_history())
    }

    // t[impl offline.doc.create] — a record made with no server is a
    // document at once, stored and listed before anything is sent
    /// Create the record `text` at `path` (chosen by the device,
    /// collision-safe). Refused if the device already holds a document
    /// there.
    pub async fn create(&self, path: &str, text: &str, origin: &str) -> Result<Entry, String> {
        let _g = self.inner.lock.lock().await;
        let replica = self.open(path).await?;
        if replica.has_history() {
            return Err(format!("{path} already has a document on this device"));
        }
        let doc = replica.doc().loro().clone();
        crate::doc::adopt_text(&doc, text);
        self.commit_durable(replica.doc_id(), &doc, origin).await?;
        let rendered = crate::doc::render(&doc, Some(text));
        self.reproject(path, &rendered, true, None, None)
            .await?
            .ok_or_else(|| format!("{path}: the new text is not a record this book lists"))
    }

    // t[impl offline.doc.replica] — an edit is a change to the document,
    // or (for a document not held yet) waits to be merged into the
    // server's version, as the change it was
    /// Edit the record at `path`: `next` is what an editor made of `base`.
    /// Only what differs between the two is applied.
    pub async fn edit(
        &self,
        path: &str,
        base: &str,
        next: &str,
        origin: &str,
    ) -> Result<Entry, String> {
        let _g = self.inner.lock.lock().await;
        let replica = self.open(path).await?;
        let previous = self.entry_at(path).await?;
        if replica.has_history() {
            let doc = replica.doc().loro().clone();
            let merged = crate::doc::merge_text(&doc, base, next);
            self.commit_durable(replica.doc_id(), &doc, origin).await?;
            let rendered = crate::doc::render(&doc, None);
            let was_unsynced = previous.as_ref().is_some_and(|e| e.unsynced);
            return self
                .reproject(path, &rendered, was_unsynced || merged.any(), None, None)
                .await?
                .ok_or_else(|| format!("{path}: no longer a record this book lists"));
        }
        // Not held yet: the edit waits, composed with any edit already
        // waiting (its base stays the first one's).
        let waiting = match previous.and_then(|e| e.waiting) {
            Some(w) => Waiting {
                base: w.base,
                next: next.to_owned(),
                origin: origin.to_owned(),
            },
            None => Waiting {
                base: base.to_owned(),
                next: next.to_owned(),
                origin: origin.to_owned(),
            },
        };
        self.reproject(path, next, true, Some(waiting), None)
            .await?
            .ok_or_else(|| format!("{path}: the edit is not a record this book lists"))
    }

    /// Bring the server's version of `path` onto the device (it then holds
    /// the document, and edits go straight into it). `vault` registers the
    /// file (the server's admission + write check); `docs` is the session
    /// lane.
    pub async fn pull(
        &self,
        path: &str,
        vault: &VaultSyncClient,
        docs: &DocSyncClient,
    ) -> Result<(), SessionEnd> {
        let _g = self.inner.lock.lock().await;
        let entry = self.entry_at(path).await.ok().flatten();
        self.sync_one(path, entry, vault, docs).await
    }

    // t[impl offline.doc.replica] — on reconnect every record with
    // unacknowledged changes syncs; the server writes the files
    /// Sync every record the server has not acknowledged in full.
    pub async fn sync(&self, vault: &VaultSyncClient, docs: &DocSyncClient) -> SyncReport {
        let mut report = SyncReport::default();
        let pending: Vec<Entry> = match self.entries().await {
            Ok(es) => es.into_iter().filter(|e| e.unsynced).collect(),
            Err(e) => {
                tracing::warn!(scope = %self.inner.scope, "record book: index unreadable: {e}");
                return report;
            }
        };
        for entry in pending {
            let _g = self.inner.lock.lock().await;
            // Re-read under the lock: an edit may have composed onto it.
            let current = self.entry_at(&entry.path).await.ok().flatten();
            match self.sync_one(&entry.path, current, vault, docs).await {
                Ok(()) => report.synced += 1,
                Err(SessionEnd::Dropped) => {
                    report.interrupted = true;
                    return report;
                }
                Err(SessionEnd::Refused(why)) => report.refused.push((entry.path, why)),
                Err(SessionEnd::ForeignLineage) => report
                    .refused
                    .push((entry.path, crate::replica::FOREIGN_LINEAGE.to_owned())),
            }
        }
        report
    }

    /// One document's session, under the book's lock.
    async fn sync_one(
        &self,
        path: &str,
        entry: Option<Entry>,
        vault: &VaultSyncClient,
        docs: &DocSyncClient,
    ) -> Result<(), SessionEnd> {
        let mark_refused = async |why: &str| {
            if let Some(e) = &entry {
                let text = e.waiting.as_ref().map(|w| w.next.clone());
                let text = match text {
                    Some(t) => Some(t),
                    None => self
                        .open(path)
                        .await
                        .ok()
                        .map(|r| crate::doc::render(r.doc().loro(), None)),
                };
                if let Some(text) = text {
                    let _ = self
                        .reproject(path, &text, true, e.waiting.clone(), Some(why.to_owned()))
                        .await;
                }
            }
        };
        match vault
            .open_collab(self.inner.vault_id.clone(), path.to_owned())
            .await
        {
            Ok(_) => {}
            Err(vox::VoxError::User(e)) => {
                let why = format!("{e:?}");
                mark_refused(&why).await;
                return Err(SessionEnd::Refused(why));
            }
            Err(_) => return Err(SessionEnd::Dropped),
        }
        let mut replica = self.open(path).await.map_err(SessionEnd::Refused)?;
        let waiting = entry.as_ref().and_then(|e| e.waiting.clone());
        let apply = waiting.clone();
        let res = replica
            .sync_once(docs, move |doc| {
                if let Some(w) = apply {
                    crate::doc::merge_text(doc, &w.base, &w.next);
                    crate::doc::commit_as(doc, &w.origin);
                }
            })
            .await;
        match res {
            Ok(()) => {
                let text = crate::doc::render(replica.doc().loro(), None);
                let _ = self.reproject(path, &text, false, None, None).await;
                Ok(())
            }
            Err(SessionEnd::Dropped) => {
                // A waiting edit merged into the server's version before
                // the connection went is in the document now; it must
                // not be merged a second time.
                if waiting.is_some() && replica.has_history() {
                    let text = crate::doc::render(replica.doc().loro(), None);
                    let _ = self.reproject(path, &text, true, None, None).await;
                }
                Err(SessionEnd::Dropped)
            }
            Err(end) => {
                let why = match &end {
                    SessionEnd::Refused(w) => w.clone(),
                    _ => crate::replica::FOREIGN_LINEAGE.to_owned(),
                };
                mark_refused(&why).await;
                Err(end)
            }
        }
    }
}
