//! [`ProductBackend`] — `EmailProduct` impl + the delivery poller.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use email_proto::{
    DERIVATION_VERSION, Derivation, DerivationKind, Draft, EmailChange, EmailEvent, EmailProduct,
    EmailSync, EmailSyncError, OutboxEntry, OutboxStatus, SeqRange,
};
use email_store::{Store, StoreError};

use crate::triage::{ContactLookup, DerivationEngine, DerivationInput, HeuristicEngine};

/// One account the product layer serves: the account id (must
/// match the `EmailSync` backend's), the account root the store
/// lives under (`<root>/index.db`), and the account's own
/// address (triage's self-mail guard + direct-address scoring).
pub struct ProductAccount {
    pub id: String,
    pub root: PathBuf,
    pub address: String,
}

/// Store-backed `EmailProduct` backend + delivery poller. Cheap
/// to `Clone` — internals are `Arc`'d.
#[derive(Clone, architect::HasDispatcher)]
pub struct ProductBackend {
    inner: Arc<Inner>,
}

struct Inner {
    /// Per-account product store (outbox + derivation + notify
    /// tables in the account's `index.db`).
    stores: HashMap<String, Mutex<Store>>,
    /// Account id → its own address (triage self-mail guard).
    addresses: HashMap<String, String>,
    /// Derivation engine (heuristics in v1; the agent plugin may
    /// swap in one whose `derive_llm` is real).
    engine: Arc<dyn DerivationEngine>,
    /// Known-contact resolution for the triage pass.
    contacts: Arc<dyn ContactLookup>,
    /// The mounted mailbox backend — delivery calls its `send`,
    /// so the Sent copy + `NewMessage` event come for free.
    sync: Arc<dyn EmailSync + Send + Sync>,
    /// The `EmailChange` hub the `EmailSync` stream serves —
    /// cloned from the sync backend so product events reach the
    /// same subscribers.
    hub: architect::PubSub<EmailChange>,
    /// Poller wake-up — `approve` pokes it so delivery starts
    /// promptly instead of waiting out the interval.
    wake: tokio::sync::Notify,
}

/// Base delay before retrying a failed delivery; doubles per
/// retry (30s, 1m, 2m, 4m, 8m) up to [`MAX_BACKOFF`].
const BASE_BACKOFF: Duration = Duration::from_secs(30);
const MAX_BACKOFF: Duration = Duration::from_secs(3600);

/// Entries delivered per account per poller pass — keeps one
/// pass bounded (mirrors the triage pass budget).
const DRAIN_BUDGET: u32 = 5;

/// Messages triaged per account per pass (the odysseus
/// `max_process` idea — bound the per-pass work, catch up over
/// successive passes).
const TRIAGE_BUDGET: usize = 5;

/// How many recent INBOX envelopes one triage pass considers.
const TRIAGE_SCAN: u32 = 50;

/// New notification marks per account per pass — a mail flood
/// (or a mis-detected baseline) can't stampede the notifier.
const NOTIFY_CAP: usize = 50;

impl ProductBackend {
    /// Open the per-account stores and build the backend with the
    /// v1 [`HeuristicEngine`].
    ///
    /// `sync` is the mounted `EmailSync` backend delivery goes
    /// through; `hub` is that backend's `EmailChange` hub
    /// (`EmailSyncStreamSource::changes_hub(&b).clone()`), so
    /// outbox events interleave with mailbox events on the one
    /// stream subscribers already hold. `contacts` feeds the
    /// triage pass's known-sender scoring
    /// ([`crate::triage::NoContacts`] when the org has none).
    pub fn new<I>(
        accounts: I,
        sync: Arc<dyn EmailSync + Send + Sync>,
        hub: architect::PubSub<EmailChange>,
        contacts: Arc<dyn ContactLookup>,
    ) -> Result<Self, StoreError>
    where
        I: IntoIterator<Item = ProductAccount>,
    {
        Self::with_engine(accounts, sync, hub, contacts, Arc::new(HeuristicEngine))
    }

    /// [`Self::new`] with an explicit engine — the seam the agent
    /// plugin uses to supply one whose `derive_llm` is real.
    pub fn with_engine<I>(
        accounts: I,
        sync: Arc<dyn EmailSync + Send + Sync>,
        hub: architect::PubSub<EmailChange>,
        contacts: Arc<dyn ContactLookup>,
        engine: Arc<dyn DerivationEngine>,
    ) -> Result<Self, StoreError>
    where
        I: IntoIterator<Item = ProductAccount>,
    {
        let mut stores = HashMap::new();
        let mut addresses = HashMap::new();
        for acct in accounts {
            addresses.insert(acct.id.clone(), acct.address);
            stores.insert(acct.id, Mutex::new(Store::open(acct.root)?));
        }
        Ok(Self {
            inner: Arc::new(Inner {
                stores,
                addresses,
                engine,
                contacts,
                sync,
                hub,
                wake: tokio::sync::Notify::new(),
            }),
        })
    }

    fn store(&self, account: &str) -> Result<&Mutex<Store>, EmailSyncError> {
        self.inner
            .stores
            .get(account)
            .ok_or(EmailSyncError::UnknownAccount)
    }

    /// Publish an outbox transition on the shared stream.
    fn publish_outbox(&self, account: &str, entry: &OutboxEntry) {
        self.inner.hub.publish(EmailChange {
            account: account.to_string(),
            event: EmailEvent::OutboxChanged {
                id: entry.id,
                status: entry.status,
            },
        });
    }

    /// Start the delivery poller: wakes every `interval` (or
    /// immediately on approval), claims due `Approved`/retryable
    /// `Failed` entries, and delivers them through
    /// `EmailSync::send`. Abort the returned handle to stop.
    pub fn spawn_poller(&self, interval: Duration) -> tokio::task::JoinHandle<()> {
        let backend = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = backend.inner.wake.notified() => {}
                    () = tokio::time::sleep(interval) => {}
                }
                backend.drain_outbox_once().await;
                backend.background_pass_once().await;
            }
        })
    }

    /// One bounded background pass over every account, sharing a
    /// single envelope scan per account:
    ///
    /// - **notify**: first sight of the account baselines
    ///   silently; afterwards genuinely-new messages get their
    ///   one notification mark ([`NOTIFY_CAP`]/pass).
    /// - **triage**: compute derivations for at most
    ///   [`TRIAGE_BUDGET`] messages lacking current-version rows,
    ///   publishing `DerivationsUpdated` per message
    ///   (publish-after-write).
    ///
    /// Catch-up happens over successive passes — cost stays flat
    /// however big the backlog.
    pub async fn background_pass_once(&self) {
        let accounts: Vec<String> = self.inner.stores.keys().cloned().collect();
        for account in accounts {
            if let Err(err) = self.background_account_pass(&account).await {
                tracing::warn!(account, %err, "email background pass failed");
            }
        }
    }

    async fn background_account_pass(&self, account: &str) -> Result<(), EmailSyncError> {
        // Newest envelopes + the notify step + the missing-work
        // set, off the async thread in one hop.
        let backend = self.clone();
        let account_owned = account.to_string();
        let (envelopes, missing) =
            tokio::task::spawn_blocking(move || -> Result<_, EmailSyncError> {
                let envelopes = backend.inner.sync.fetch_envelopes(
                    &account_owned,
                    "INBOX",
                    SeqRange::Recent(TRIAGE_SCAN),
                )?;
                let ids: Vec<String> = envelopes.iter().map(|e| e.message_id.clone()).collect();
                let store = backend.store(&account_owned)?;
                let mut store = store.lock().expect("store mutex");

                // Alert-once bookkeeping.
                let id_refs = ids.iter().map(String::as_str);
                if store.notify_is_baselined().map_err(map_store)? {
                    store
                        .notify_observe(id_refs, now_ms(), NOTIFY_CAP)
                        .map_err(map_store)?;
                } else {
                    // First sight: everything pre-existing is
                    // seen-for-notification; fire nothing.
                    store
                        .notify_baseline(id_refs, now_ms())
                        .map_err(map_store)?;
                }

                let missing = store
                    .derivations_missing(&ids, DerivationKind::Urgency, DERIVATION_VERSION)
                    .map_err(map_store)?;
                Ok((envelopes, missing))
            })
            .await
            .map_err(|e| EmailSyncError::Internal(e.to_string()))??;

        if missing.is_empty() {
            return Ok(());
        }

        // One contacts snapshot per pass, not per message.
        let known = {
            let contacts = self.inner.contacts.clone();
            tokio::task::spawn_blocking(move || contacts.known_addresses())
                .await
                .map_err(|e| EmailSyncError::Internal(e.to_string()))?
        };
        let address = self
            .inner
            .addresses
            .get(account)
            .cloned()
            .unwrap_or_default();

        for message_id in missing.into_iter().take(TRIAGE_BUDGET) {
            let Some(envelope) = envelopes.iter().find(|e| e.message_id == message_id) else {
                continue;
            };

            // Headers for the header-based heuristics; tolerate a
            // fetch failure (the envelope alone still triages).
            let backend = self.clone();
            let acct = account.to_string();
            let mid = message_id.clone();
            let message = tokio::task::spawn_blocking(move || {
                backend.inner.sync.fetch_message(&acct, &mid).ok()
            })
            .await
            .map_err(|e| EmailSyncError::Internal(e.to_string()))?;
            let headers_raw = message
                .as_ref()
                .map(|m| m.headers_raw.as_str())
                .unwrap_or("");
            let body_text = message.as_ref().and_then(|m| m.body_text.as_deref());

            let sender_known = envelope
                .from
                .first()
                .is_some_and(|a| known.contains(&a.email.to_ascii_lowercase()));
            let input = DerivationInput {
                account_address: &address,
                envelope,
                headers_raw,
                body_text,
                sender_known,
            };
            let rows = self.inner.engine.derive(&input);

            // Persist, then publish.
            {
                let store = self.store(account)?;
                let mut store = store.lock().expect("store mutex");
                for (kind, payload) in &rows {
                    store
                        .derivation_upsert(
                            &message_id,
                            *kind,
                            DERIVATION_VERSION,
                            payload,
                            now_ms(),
                        )
                        .map_err(map_store)?;
                }
            }
            self.inner.hub.publish(EmailChange {
                account: account.to_string(),
                event: EmailEvent::DerivationsUpdated {
                    message_id: message_id.clone(),
                },
            });
        }
        Ok(())
    }

    /// One delivery pass over every account. Public so tests (and
    /// a future explicit "flush now" surface) can drive it
    /// without the poller task.
    pub async fn drain_outbox_once(&self) {
        let accounts: Vec<String> = self.inner.stores.keys().cloned().collect();
        for account in accounts {
            if let Err(err) = self.drain_account(&account).await {
                tracing::warn!(account, %err, "outbox drain failed");
            }
        }
    }

    async fn drain_account(&self, account: &str) -> Result<(), EmailSyncError> {
        // Claim (flips to Sending atomically).
        let claimed = {
            let backend = self.clone();
            let account = account.to_string();
            tokio::task::spawn_blocking(move || -> Result<Vec<OutboxEntry>, EmailSyncError> {
                let store = backend.store(&account)?;
                let mut store = store.lock().expect("store mutex");
                store
                    .outbox_claim_due(&account, now_ms(), DRAIN_BUDGET)
                    .map_err(map_store)
            })
            .await
            .map_err(|e| EmailSyncError::Internal(e.to_string()))??
        };

        for entry in claimed {
            self.publish_outbox(account, &entry);
            let outcome = {
                let sync = self.inner.sync.clone();
                let account = account.to_string();
                let draft = entry.draft.clone();
                tokio::task::spawn_blocking(move || sync.send(&account, draft))
                    .await
                    .map_err(|e| EmailSyncError::Internal(e.to_string()))?
            };

            let backend = self.clone();
            let account_owned = account.to_string();
            let id = entry.id;
            let retries = entry.retries;
            let updated =
                tokio::task::spawn_blocking(move || -> Result<OutboxEntry, EmailSyncError> {
                    let store = backend.store(&account_owned)?;
                    let mut store = store.lock().expect("store mutex");
                    match outcome {
                        Ok(message_id) => store
                            .outbox_mark_sent(&account_owned, id, &message_id, now_ms())
                            .map_err(map_store),
                        Err(err) => {
                            let backoff = BASE_BACKOFF
                                .saturating_mul(2u32.saturating_pow(retries))
                                .min(MAX_BACKOFF);
                            store
                                .outbox_mark_failed(
                                    &account_owned,
                                    id,
                                    &err.to_string(),
                                    now_ms(),
                                    now_ms() + backoff.as_millis() as i64,
                                )
                                .map_err(map_store)
                        }
                    }
                })
                .await
                .map_err(|e| EmailSyncError::Internal(e.to_string()))??;

            if updated.status == OutboxStatus::Failed {
                tracing::warn!(
                    account,
                    id = updated.id,
                    retries = updated.retries,
                    error = updated.last_error.as_deref().unwrap_or(""),
                    "outbox delivery failed"
                );
            }
            self.publish_outbox(account, &updated);
        }
        Ok(())
    }
}

impl EmailProduct for ProductBackend {
    fn derivations(
        &self,
        account: &str,
        ids: Vec<String>,
    ) -> Result<Vec<Derivation>, EmailSyncError> {
        let store = self.store(account)?.lock().expect("store mutex");
        store
            .derivations_for(&ids, DERIVATION_VERSION)
            .map_err(map_store)
    }

    fn list_outbox(&self, account: &str) -> Result<Vec<OutboxEntry>, EmailSyncError> {
        let store = self.store(account)?.lock().expect("store mutex");
        store.outbox_list(account).map_err(map_store)
    }

    fn submit_draft(
        &self,
        account: &str,
        draft: Draft,
        origin: &str,
    ) -> Result<OutboxEntry, EmailSyncError> {
        if draft.to.is_empty() && draft.cc.is_empty() && draft.bcc.is_empty() {
            return Err(EmailSyncError::Protocol("draft has no recipients".into()));
        }
        let entry = {
            let mut store = self.store(account)?.lock().expect("store mutex");
            store
                .outbox_submit(account, &draft, origin, now_ms())
                .map_err(map_store)?
        };
        self.publish_outbox(account, &entry);
        Ok(entry)
    }

    fn approve(&self, account: &str, id: u64) -> Result<OutboxEntry, EmailSyncError> {
        let entry = {
            let mut store = self.store(account)?.lock().expect("store mutex");
            store
                .outbox_approve(account, id, now_ms())
                .map_err(map_store)?
        };
        self.publish_outbox(account, &entry);
        // Deliver promptly — don't wait out the poller interval.
        self.inner.wake.notify_one();
        Ok(entry)
    }

    fn cancel(&self, account: &str, id: u64) -> Result<OutboxEntry, EmailSyncError> {
        let entry = {
            let mut store = self.store(account)?.lock().expect("store mutex");
            store
                .outbox_cancel(account, id, now_ms())
                .map_err(map_store)?
        };
        self.publish_outbox(account, &entry);
        Ok(entry)
    }

    fn unnotified(&self, account: &str, limit: u32) -> Result<Vec<String>, EmailSyncError> {
        let store = self.store(account)?.lock().expect("store mutex");
        store.notify_unnotified(limit).map_err(map_store)
    }

    fn mark_notified(&self, account: &str, ids: Vec<String>) -> Result<u32, EmailSyncError> {
        let mut store = self.store(account)?.lock().expect("store mutex");
        store
            .notify_mark(ids.iter().map(String::as_str))
            .map_err(map_store)
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn map_store(err: StoreError) -> EmailSyncError {
    match err {
        StoreError::OutboxNotFound(_) => EmailSyncError::NotFound,
        e @ StoreError::OutboxTransition { .. } => EmailSyncError::Protocol(e.to_string()),
        e => EmailSyncError::Internal(e.to_string()),
    }
}
