//! Per-account sync engine. Wraps any `EmailSync` backend in a
//! periodic poll loop, computes diffs against the previous
//! snapshot, and broadcasts events.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use email_proto::{EmailSync, Envelope, SeqRange};
use email_store::Store;
use tokio::sync::{Mutex, broadcast};
use tokio::task::JoinHandle;

use crate::event::SyncEvent;
use crate::snapshot::Snapshot;

#[derive(Debug, Clone)]
pub struct SyncOptions {
    /// How often to run a full reconciliation cycle when there's
    /// no push channel attached. Default: 60 seconds.
    pub poll_interval: Duration,
    /// How many recent envelopes to pull per folder per cycle.
    /// Default: 50. The full-history backfill is a separate
    /// pass owned by `email-store`'s walker.
    pub envelopes_per_folder: u32,
    /// Broadcast channel capacity. Default: 256.
    pub channel_capacity: usize,
}

impl Default for SyncOptions {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(60),
            envelopes_per_folder: 50,
            channel_capacity: 256,
        }
    }
}

/// Live sync task. Drop the handle (or call [`Self::stop`]) to
/// cancel the underlying tokio task; subscribers see a
/// closed-channel error on the next `recv`.
pub struct SyncHandle {
    task: JoinHandle<()>,
    events: broadcast::Sender<SyncEvent>,
}

impl SyncHandle {
    /// Get a fresh subscriber stream. Multiple subscribers
    /// share the same broadcast — slow ones see `Lagged`.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<SyncEvent> {
        self.events.subscribe()
    }

    /// Cancel the underlying task. Idempotent — also runs from
    /// the `Drop` impl when the handle goes out of scope.
    pub fn stop(&self) {
        self.task.abort();
    }
}

impl Drop for SyncHandle {
    fn drop(&mut self) {
        // Abort the spawned task. The broadcast sender drops
        // with this handle, so subscribers naturally observe
        // `RecvError::Closed`.
        self.task.abort();
    }
}

/// Per-account sync driver. Generic over any `EmailSync` impl
/// so the same engine drives imap, maildir, jmap, and
/// nextcloud accounts uniformly.
pub struct SyncEngine<B: EmailSync + Send + Sync + 'static> {
    backend: Arc<B>,
    account_id: String,
    options: SyncOptions,
    snapshot: Arc<Mutex<Snapshot>>,
    /// Optional durable cache. When set, each cycle persists
    /// every fetched envelope through `Store::upsert_envelope`
    /// so the engine survives restart with the same state it
    /// last saw on the server.
    store: Option<Arc<Mutex<Store>>>,
}

impl<B: EmailSync + Send + Sync + 'static> SyncEngine<B> {
    pub fn new(backend: Arc<B>, account_id: impl Into<String>) -> Self {
        Self::with_options(backend, account_id, SyncOptions::default())
    }

    pub fn with_options(
        backend: Arc<B>,
        account_id: impl Into<String>,
        options: SyncOptions,
    ) -> Self {
        Self {
            backend,
            account_id: account_id.into(),
            options,
            snapshot: Arc::new(Mutex::new(Snapshot::new())),
            store: None,
        }
    }

    /// Attach a durable cache. Each cycle's envelopes are
    /// persisted through it so the engine survives restart and
    /// the UI can render the last-known state without waiting
    /// for the next backend fetch.
    #[must_use]
    pub fn with_store(mut self, store: Arc<Mutex<Store>>) -> Self {
        self.store = Some(store);
        self
    }

    /// Borrow the current snapshot. Useful for tests and for
    /// the UI to render the last-known state without waiting
    /// for the next cycle.
    pub async fn current_snapshot(&self) -> Snapshot {
        self.snapshot.lock().await.clone()
    }

    /// Spawn the sync loop and return a handle.
    #[must_use]
    pub fn start(self) -> SyncHandle {
        let (tx, _rx) = broadcast::channel(self.options.channel_capacity);
        let events = tx.clone();
        let task = tokio::spawn(async move {
            self.run(tx).await;
        });
        SyncHandle { task, events }
    }

    async fn run(self, tx: broadcast::Sender<SyncEvent>) {
        // First cycle immediately, then on the interval.
        let mut interval = tokio::time::interval(self.options.poll_interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            interval.tick().await;
            // If no subscribers are left, keep running anyway —
            // the snapshot still updates and a new subscriber
            // can attach later. The task only exits on `abort`
            // from the `SyncHandle` drop.
            self.run_cycle(&tx).await;
        }
    }

    /// Run one reconciliation pass. Public so tests + the
    /// "Refresh now" UI action can drive it deterministically.
    pub async fn run_cycle(&self, tx: &broadcast::Sender<SyncEvent>) {
        let started = std::time::Instant::now();
        let prev = self.snapshot.lock().await.clone();
        let _ = tx.send(SyncEvent::CycleStarted {
            folder_count: prev.folders.len(),
        });

        let fetched = match self.collect_envelopes().await {
            Ok(v) => v,
            Err(reason) => {
                let _ = tx.send(SyncEvent::CycleFailed { reason });
                return;
            }
        };

        // Build the new snapshot and persist through the store
        // (if one is attached) before diff'ing — so a consumer
        // who reads the store right after a CycleCompleted event
        // sees the new state.
        let mut next = Snapshot::new();
        for (folder, envelopes) in &fetched {
            let ids: BTreeSet<String> = envelopes.iter().map(|e| e.message_id.clone()).collect();
            next.folders.insert(folder.clone(), ids);
        }
        if let Some(store) = &self.store {
            self.persist(store, &fetched).await;
        }

        for evt in prev.diff(&next) {
            let _ = tx.send(SyncEvent::Email(evt));
        }
        *self.snapshot.lock().await = next;

        let _ = tx.send(SyncEvent::CycleCompleted {
            duration_ms: started.elapsed().as_millis() as u64,
        });
    }

    /// Collect envelopes from every folder. Returns a `Vec` of
    /// `(folder_id, envelopes)` pairs so `run_cycle` can both
    /// snapshot and persist without re-fetching.
    async fn collect_envelopes(&self) -> Result<Vec<(String, Vec<Envelope>)>, String> {
        // `EmailSync` methods are sync. We're inside a tokio
        // runtime — hop to the blocking pool so we don't stall
        // it.
        let backend = self.backend.clone();
        let account = self.account_id.clone();
        let envelopes_per_folder = self.options.envelopes_per_folder;

        tokio::task::spawn_blocking(move || -> Result<Vec<(String, Vec<Envelope>)>, String> {
            let folders = backend
                .list_folders(&account)
                .map_err(|e| format!("list_folders: {e}"))?;

            let mut out = Vec::with_capacity(folders.len());
            for f in &folders {
                let envs = match backend.fetch_envelopes(
                    &account,
                    &f.id,
                    SeqRange::Recent(envelopes_per_folder),
                ) {
                    Ok(v) => v,
                    Err(err) => {
                        tracing::warn!(folder = %f.id, %err, "fetch_envelopes failed");
                        Vec::new()
                    }
                };
                out.push((f.id.clone(), envs));
            }
            Ok(out)
        })
        .await
        .map_err(|e| format!("blocking task panic: {e}"))?
    }

    /// Best-effort persist. Errors are logged but don't fail
    /// the cycle — the in-memory snapshot is still updated, so
    /// a downstream cache failure doesn't lose live events.
    async fn persist(&self, store: &Arc<Mutex<Store>>, fetched: &[(String, Vec<Envelope>)]) {
        let mut s = store.lock().await;
        for (_folder, envelopes) in fetched {
            for env in envelopes {
                if let Err(err) = s.upsert_envelope(env, None, None, None) {
                    tracing::warn!(
                        msg_id = %env.message_id,
                        %err,
                        "upsert_envelope failed"
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use email_proto::AccountId;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn fixture_account() -> email_proto::Account {
        email_proto::Account {
            id: AccountId("scratch".into()),
            name: "scratch".into(),
            address: "you@example.com".into(),
            display_name: None,
        }
    }

    fn write_msg(dir: &std::path::Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).unwrap();
    }

    fn build_fixture_maildir() -> (TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::create_dir_all(root.join("new")).unwrap();
        std::fs::create_dir_all(root.join("cur")).unwrap();
        std::fs::create_dir_all(root.join("tmp")).unwrap();
        write_msg(
            &root.join("new"),
            "1700000000.M1.host",
            "Message-ID: <m1@example.com>\r\n\
             From: a@example.com\r\n\
             To: you@example.com\r\n\
             Subject: First\r\n\
             Date: Mon, 14 Nov 2023 12:00:00 +0000\r\n\
             \r\n\
             hi\r\n",
        );
        (dir, root)
    }

    #[tokio::test]
    async fn first_cycle_yields_new_messages() {
        let (_dir, root) = build_fixture_maildir();
        let account = fixture_account();
        let backend = Arc::new(email_maildir::Backend::single(account.clone(), root).unwrap());
        let engine = SyncEngine::with_options(
            backend,
            account.id.0.clone(),
            SyncOptions {
                poll_interval: Duration::from_secs(3600),
                envelopes_per_folder: 50,
                channel_capacity: 64,
            },
        );

        let (tx, mut rx) = broadcast::channel::<SyncEvent>(64);
        engine.run_cycle(&tx).await;

        let mut new_msg = false;
        let mut cycle_complete = false;
        while let Ok(evt) = rx.try_recv() {
            match evt {
                SyncEvent::Email(email_proto::EmailEvent::NewMessage { folder, message_id }) => {
                    assert_eq!(folder, "INBOX");
                    assert!(message_id.contains("m1@example.com"));
                    new_msg = true;
                }
                SyncEvent::CycleCompleted { .. } => cycle_complete = true,
                _ => {}
            }
        }
        assert!(new_msg, "expected at least one NewMessage event");
        assert!(cycle_complete, "expected a CycleCompleted event");
    }

    #[tokio::test]
    async fn second_cycle_is_quiet_when_nothing_changed() {
        let (_dir, root) = build_fixture_maildir();
        let account = fixture_account();
        let backend = Arc::new(email_maildir::Backend::single(account.clone(), root).unwrap());
        let engine = SyncEngine::with_options(
            backend,
            account.id.0.clone(),
            SyncOptions {
                poll_interval: Duration::from_secs(3600),
                envelopes_per_folder: 50,
                channel_capacity: 64,
            },
        );

        let (tx, mut rx) = broadcast::channel::<SyncEvent>(64);
        engine.run_cycle(&tx).await;
        // Drain everything from cycle 1.
        while rx.try_recv().is_ok() {}

        engine.run_cycle(&tx).await;
        let mut email_events = 0;
        while let Ok(evt) = rx.try_recv() {
            if matches!(evt, SyncEvent::Email(_)) {
                email_events += 1;
            }
        }
        assert_eq!(email_events, 0, "second cycle should be diff-clean");
    }

    #[tokio::test]
    async fn new_file_between_cycles_emits_new_message() {
        let (dir, root) = build_fixture_maildir();
        let account = fixture_account();
        let backend =
            Arc::new(email_maildir::Backend::single(account.clone(), root.clone()).unwrap());
        let engine = SyncEngine::with_options(
            backend,
            account.id.0.clone(),
            SyncOptions {
                poll_interval: Duration::from_secs(3600),
                envelopes_per_folder: 50,
                channel_capacity: 64,
            },
        );

        let (tx, mut rx) = broadcast::channel::<SyncEvent>(64);
        engine.run_cycle(&tx).await;
        while rx.try_recv().is_ok() {}

        // Drop a new message between cycles.
        write_msg(
            &dir.path().join("new"),
            "1700000100.M2.host",
            "Message-ID: <m2@example.com>\r\n\
             From: b@example.com\r\n\
             Subject: Second\r\n\
             Date: Mon, 14 Nov 2023 13:00:00 +0000\r\n\
             \r\n\
             hi again\r\n",
        );
        engine.run_cycle(&tx).await;

        let mut saw_m2 = false;
        while let Ok(evt) = rx.try_recv() {
            if let SyncEvent::Email(email_proto::EmailEvent::NewMessage { message_id, .. }) = &evt {
                if message_id.contains("m2@example.com") {
                    saw_m2 = true;
                }
            }
        }
        assert!(saw_m2, "expected NewMessage for the second file");
    }

    #[tokio::test]
    async fn cycle_persists_envelopes_through_store() {
        let (_dir, root) = build_fixture_maildir();
        let account = fixture_account();
        let backend = Arc::new(email_maildir::Backend::single(account.clone(), root).unwrap());

        let store_dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Mutex::new(
            email_store::Store::open(store_dir.path()).unwrap(),
        ));

        let engine = SyncEngine::with_options(
            backend,
            account.id.0.clone(),
            SyncOptions {
                poll_interval: Duration::from_secs(3600),
                envelopes_per_folder: 50,
                channel_capacity: 64,
            },
        )
        .with_store(store.clone());

        let (tx, _rx) = broadcast::channel::<SyncEvent>(64);
        engine.run_cycle(&tx).await;

        // The fixture INBOX message should now be in the store.
        let s = store.lock().await;
        let known = s.known_message_ids("INBOX").unwrap();
        assert!(
            known.iter().any(|id| id.contains("m1@example.com")),
            "store known_message_ids should contain m1: {known:?}"
        );
        let envs = s.query_envelopes("INBOX", 10).unwrap();
        assert_eq!(envs.len(), 1);
        assert_eq!(envs[0].envelope.subject, "First");
    }
}
