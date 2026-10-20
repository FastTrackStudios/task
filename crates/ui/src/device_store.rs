//! The device's local store (`docs/spec/offline.md`, step 1): where
//! every list's last-seen rows, the queued service calls and the
//! documents' bytes live on this machine.
//!
//! - **native** (desktop, iOS) — one Turso database,
//!   `<data dir>/device.db` (the same directory the server registry and
//!   cached sessions use, so on iOS it is in `Application Support`).
//!   architect's [`architect::TursoRows`] keeps the index rows and the
//!   call queue in it; crdt's [`crdt::TursoPersistence`] keeps document
//!   snapshots + update logs in the same file ([`doc_persistence`]).
//!   Turso's local engine only.
//! - **web** — the browser's `localStorage` for rows and the queue
//!   (size-guarded; IndexedDB is the target, `docs/spec/unmet.md`), and
//!   crdt's IndexedDB persistence for documents.
//!
//! Opening a database is async and the shell needs the handle while it
//! renders its first frame, so native hands out [`DeviceRows`]: a
//! [`LocalRows`] that opens the file on first use and falls back to
//! memory (online-only, as before) if it cannot.

use std::sync::Arc;

use architect::LocalRows;

/// The device's rows, as the app root provides them
/// (`architect::provide_local_rows`).
#[must_use]
pub fn device_rows() -> Arc<dyn LocalRows> {
    imp::device_rows()
}

/// Document persistence in the device database, for a vault document's
/// replica (`offline.doc.replica`). `None` when the database cannot be
/// opened (no data dir) — callers fall back to their own persistence.
#[cfg(not(target_arch = "wasm32"))]
pub async fn doc_persistence() -> Option<crdt::TursoPersistence> {
    let db = imp::database().await?;
    crdt::TursoPersistence::open(&db).await.ok()
}

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use std::sync::{Arc, LazyLock};

    use architect::{LocalError, LocalRow, LocalRows, MemoryRows, TursoRows};
    use futures_util::lock::Mutex;

    /// The one database handle for the process (`turso::Database` is a
    /// cheap clone over an `Arc`).
    static DATABASE: LazyLock<Mutex<Option<Option<turso::Database>>>> =
        LazyLock::new(|| Mutex::new(None));

    /// The device database, opened on first call. `None` when there is
    /// no data dir or the file cannot be opened.
    pub(super) async fn database() -> Option<turso::Database> {
        let mut slot = DATABASE.lock().await;
        if let Some(db) = &*slot {
            return db.clone();
        }
        let opened = async {
            let dir = crate::server_registry::data_dir()?;
            std::fs::create_dir_all(&dir).ok()?;
            let path = dir.join("device.db");
            turso::Builder::new_local(path.to_str()?).build().await.ok()
        }
        .await;
        *slot = Some(opened.clone());
        opened
    }

    /// [`LocalRows`] over the device database, opened lazily.
    pub(super) struct DeviceRows {
        inner: Mutex<Option<Arc<dyn LocalRows>>>,
    }

    impl DeviceRows {
        async fn rows(&self) -> Arc<dyn LocalRows> {
            let mut slot = self.inner.lock().await;
            if let Some(rows) = &*slot {
                return rows.clone();
            }
            let opened: Arc<dyn LocalRows> = match database().await {
                Some(db) => match TursoRows::open(&db).await {
                    Ok(rows) => Arc::new(rows),
                    Err(_) => Arc::new(MemoryRows::in_memory()),
                },
                None => Arc::new(MemoryRows::in_memory()),
            };
            *slot = Some(opened.clone());
            opened
        }
    }

    static ROWS: LazyLock<Arc<dyn LocalRows>> = LazyLock::new(|| {
        Arc::new(DeviceRows {
            inner: Mutex::new(None),
        })
    });

    pub(super) fn device_rows() -> Arc<dyn LocalRows> {
        ROWS.clone()
    }

    #[async_trait::async_trait]
    impl LocalRows for DeviceRows {
        async fn load(&self, kind: &str, scope: &str) -> Result<Option<Vec<LocalRow>>, LocalError> {
            self.rows().await.load(kind, scope).await
        }
        async fn replace(
            &self,
            kind: &str,
            scope: &str,
            rows: Vec<LocalRow>,
        ) -> Result<(), LocalError> {
            self.rows().await.replace(kind, scope, rows).await
        }
        async fn upsert(&self, kind: &str, scope: &str, row: LocalRow) -> Result<(), LocalError> {
            self.rows().await.upsert(kind, scope, row).await
        }
        async fn remove(&self, kind: &str, scope: &str, key: &str) -> Result<(), LocalError> {
            self.rows().await.remove(kind, scope, key).await
        }
        async fn project(
            &self,
            owner: &str,
            rows: Vec<(String, String, LocalRow)>,
        ) -> Result<(), LocalError> {
            self.rows().await.project(owner, rows).await
        }
        async fn queue_push(&self, body: String) -> Result<u64, LocalError> {
            self.rows().await.queue_push(body).await
        }
        async fn queue_list(&self) -> Result<Vec<(u64, String)>, LocalError> {
            self.rows().await.queue_list().await
        }
        async fn queue_replace(&self, seq: u64, body: String) -> Result<(), LocalError> {
            self.rows().await.queue_replace(seq, body).await
        }
        async fn queue_remove(&self, seq: u64) -> Result<(), LocalError> {
            self.rows().await.queue_remove(seq).await
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod imp {
    use std::sync::Arc;

    use architect::{LocalRows, LocalStorageKv, LocalStorageRows};

    pub(super) fn device_rows() -> Arc<dyn LocalRows> {
        Arc::new(LocalStorageRows::new(
            LocalStorageKv::default(),
            "task.device.",
        ))
    }
}
