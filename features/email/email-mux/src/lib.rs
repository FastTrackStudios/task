//! Account-dispatching [`EmailSync`] backend.
//!
//! One org can hold a local Maildir account *and* one or more remote
//! IMAP accounts (a Gmail mailbox, say), but the server mounts exactly
//! one `EmailSync` service. This routes each call to whichever backend
//! owns the named account.
//!
//! Two things make that more than a match statement:
//!
//! - **One stream.** Subscribers attach to a single `EmailChange`
//!   stream, so every sub-backend has to publish into the *same*
//!   `architect::PubSub`. There is no subscribe side to bridge two
//!   hubs with, so the mux builds the hub and hands it down via
//!   `with_changes_hub` before anything is cloned.
//! - **Degrading, not failing.** An IMAP account whose credentials are
//!   wrong or whose host is unreachable must not take the whole
//!   `/email` page down with it. Construction never fails on a bad
//!   account: it is logged and skipped, and the remaining accounts
//!   serve normally.
//!
//! Routing is by account id, which is the directory name under the
//! org's mail root. The two backends' account sets are disjoint by
//! construction — each is built from the subset of configs whose
//! `BackendKind` it handles.

#![cfg(not(target_arch = "wasm32"))]

use std::collections::HashMap;
use std::sync::Arc;

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use email_proto::{
    Account, Draft, EmailSync, EmailSyncError, Envelope, FlagDelta, Folder, Message, SeqRange,
};
use email_store::Store;

/// How long a folder listing served from the store is considered
/// current.
///
/// The store is kept warm independently — `email-product`'s pass runs
/// every 30s and IMAP IDLE pushes changes as they happen — so this is
/// not the freshness mechanism, just a ceiling on how long we will
/// serve a listing without re-checking the backend ourselves.
const LISTING_TTL: Duration = Duration::from_secs(60);

/// Which backend owns an account.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Route {
    Maildir,
    Imap,
}

/// The multiplexed backend. Cheap to clone — every field is either an
/// `Arc` or an already-`Clone` backend handle.
///
/// `HasDispatcher` is derived for the same reason the two sub-backends
/// derive it: `EmailSync`'s methods are synchronous and the IMAP one
/// blocks on a runtime internally, so architect must run them on its
/// tokio *blocking* pool. Mounting this on an async dispatcher would
/// panic with "Cannot start a runtime from within a runtime" the first
/// time anyone opened a remote mailbox.
#[derive(Clone, architect::HasDispatcher)]
pub struct Backend {
    maildir: email_maildir::Backend,
    /// `None` when no IMAP account is configured, or when the IMAP
    /// backend could not be constructed (no tokio runtime). Routing
    /// falls back to an `UnknownAccount` error rather than panicking.
    imap: Option<email_imap::Backend>,
    routes: Arc<HashMap<String, Route>>,
    changes: architect::PubSub<email_proto::EmailChange>,
    /// Per-account sqlite index, when the account has a directory to
    /// keep it in. Envelope listings are served from here rather than
    /// round-tripping the backend — on IMAP that is the difference
    /// between a network fetch and a local query every time you switch
    /// folders.
    stores: Arc<HashMap<String, Arc<Mutex<Store>>>>,
    /// `(account, folder)` → when we last refreshed that listing from
    /// the backend.
    refreshed: Arc<Mutex<HashMap<(String, String), Instant>>>,
}

impl Backend {
    /// Build from the org's account configs plus the maildir entries
    /// the server already resolved for local accounts.
    ///
    /// `maildir_entries` and `configs` describe the same set of
    /// accounts; the configs decide routing, the entries carry the
    /// maildir-specific bits (root path, submit transport) the server
    /// resolves while scanning the mail root.
    pub fn build(
        maildir_entries: Vec<email_maildir::AccountEntry>,
        configs: Vec<email_config::AccountConfig>,
        account_dirs: HashMap<String, PathBuf>,
    ) -> Self {
        let changes = architect::PubSub::sliding(256);

        let mut routes: HashMap<String, Route> = HashMap::new();
        for entry in &maildir_entries {
            routes.insert(entry.account.id.0.clone(), Route::Maildir);
        }

        let imap_configs: Vec<email_config::AccountConfig> = configs
            .iter()
            .filter(|c| matches!(c.backend, email_config::BackendKind::Imap { .. }))
            .cloned()
            .collect();
        for cfg in &imap_configs {
            routes.insert(cfg.id.0.clone(), Route::Imap);
        }

        let maildir = email_maildir::Backend::with_configured_accounts(maildir_entries)
            .with_changes_hub(changes.clone());

        let imap = if imap_configs.is_empty() {
            None
        } else {
            match email_imap::Backend::from_configs(imap_configs) {
                Ok(b) => Some(b.with_changes_hub(changes.clone())),
                Err(err) => {
                    // Only happens off a tokio runtime. Log rather than
                    // fail the org: the maildir accounts still work.
                    tracing::error!(%err, "imap backend unavailable; imap accounts disabled");
                    for (_, route) in routes.iter_mut().filter(|(_, r)| **r == Route::Imap) {
                        *route = Route::Maildir;
                    }
                    routes.retain(|_, r| *r == Route::Maildir);
                    None
                }
            }
        };

        // One sqlite index per account that has somewhere to keep it.
        // A store that fails to open is not fatal — the account just
        // loses caching and serves straight from the backend.
        let mut stores = HashMap::new();
        for (id, dir) in account_dirs {
            if !routes.contains_key(&id) {
                continue;
            }
            match Store::open(&dir) {
                Ok(store) => {
                    stores.insert(id, Arc::new(Mutex::new(store)));
                }
                Err(err) => {
                    tracing::warn!(account = %id, %err, "email index unavailable; serving uncached");
                }
            }
        }

        Self {
            maildir,
            imap,
            routes: Arc::new(routes),
            changes,
            stores: Arc::new(stores),
            refreshed: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Is our cached listing for `(account, folder)` still inside
    /// [`LISTING_TTL`]?
    fn listing_fresh(&self, account: &str, folder: &str) -> bool {
        self.refreshed
            .lock()
            .map(|m| {
                m.get(&(account.to_owned(), folder.to_owned()))
                    .is_some_and(|t| t.elapsed() < LISTING_TTL)
            })
            .unwrap_or(false)
    }

    fn mark_refreshed(&self, account: &str, folder: &str) {
        if let Ok(mut m) = self.refreshed.lock() {
            m.insert((account.to_owned(), folder.to_owned()), Instant::now());
        }
    }

    /// Drop the cached-listing marks for an account, so the next read
    /// re-checks the backend.
    ///
    /// Called after any mutation we performed: a flag flip, a move or a
    /// delete changes what the listing should say, and continuing to
    /// serve the pre-mutation rows for up to a minute would look like
    /// the action silently failed.
    fn invalidate(&self, account: &str) {
        if let Ok(mut m) = self.refreshed.lock() {
            m.retain(|(a, _), _| a != account);
        }
    }

    /// How many accounts route to IMAP. Used by the server to decide
    /// whether to start IDLE watchers.
    #[must_use]
    pub fn imap_account_ids(&self) -> Vec<String> {
        self.routes
            .iter()
            .filter(|(_, r)| **r == Route::Imap)
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// The IMAP backend, when one is configured — the server needs it
    /// to start the per-account IDLE loops.
    #[must_use]
    pub fn imap(&self) -> Option<&email_imap::Backend> {
        self.imap.as_ref()
    }

    /// The backend that owns `account`.
    ///
    /// An unknown account is `UnknownAccount`, never a panic and never
    /// a silent fall-through to the wrong store — filing a Gmail
    /// message into a local maildir because a lookup missed would be
    /// data loss.
    fn route(&self, account: &str) -> Result<&dyn EmailSync, EmailSyncError> {
        match self.routes.get(account) {
            Some(Route::Maildir) => Ok(&self.maildir),
            Some(Route::Imap) => self
                .imap
                .as_ref()
                .map(|b| b as &dyn EmailSync)
                .ok_or(EmailSyncError::UnknownAccount),
            None => Err(EmailSyncError::UnknownAccount),
        }
    }
}

impl EmailSync for Backend {
    /// The union of both backends' accounts, maildir first so a
    /// single-account org's ordering is unchanged.
    fn accounts(&self) -> Result<Vec<Account>, EmailSyncError> {
        let mut out = self.maildir.accounts()?;
        if let Some(imap) = &self.imap {
            // A backend that can't enumerate (transient network) must
            // not blank the local accounts too.
            match imap.accounts() {
                Ok(mut list) => out.append(&mut list),
                Err(err) => tracing::warn!(?err, "imap: accounts() failed; listing local only"),
            }
        }
        Ok(out)
    }

    fn list_folders(&self, account: &str) -> Result<Vec<Folder>, EmailSyncError> {
        self.route(account)?.list_folders(account)
    }

    /// Read-through: serve the listing from the local index when it is
    /// current, otherwise fetch from the backend and write through.
    ///
    /// This is the hot path — the UI lists a folder on every switch,
    /// and on IMAP each of those was a TLS connect, LOGIN, SELECT and
    /// FETCH of 50 message headers. Serving it from sqlite turns a
    /// network round trip into a local query.
    ///
    /// Only `Recent` listings are cached. `All` and explicit ranges are
    /// asking for something specific and are rare; sending them to the
    /// backend keeps the cache honest rather than guessing whether the
    /// stored rows satisfy the range.
    fn fetch_envelopes(
        &self,
        account: &str,
        folder: &str,
        range: SeqRange,
    ) -> Result<Vec<Envelope>, EmailSyncError> {
        let cacheable = matches!(range, SeqRange::Recent(_));
        let limit = match range {
            SeqRange::Recent(n) => n,
            _ => 0,
        };

        if cacheable && self.listing_fresh(account, folder) {
            if let Some(store) = self.stores.get(account) {
                if let Ok(store) = store.lock() {
                    match store.query_envelopes(folder, limit) {
                        // An empty result is ambiguous — an empty
                        // folder and a cold index look identical — so
                        // only a non-empty hit short-circuits.
                        Ok(rows) if !rows.is_empty() => {
                            return Ok(rows.into_iter().map(|r| r.envelope).collect());
                        }
                        Ok(_) => {}
                        Err(err) => {
                            tracing::warn!(account, folder, %err, "email index read failed")
                        }
                    }
                }
            }
        }

        let envelopes = self.route(account)?.fetch_envelopes(account, folder, range)?;

        if cacheable {
            if let Some(store) = self.stores.get(account) {
                if let Ok(mut store) = store.lock() {
                    for env in &envelopes {
                        if let Err(err) = store.upsert_envelope(env, None, None, None) {
                            tracing::warn!(account, %err, "email index write failed");
                            break;
                        }
                    }
                }
            }
            self.mark_refreshed(account, folder);
        }
        Ok(envelopes)
    }

    fn fetch_message(&self, account: &str, message_id: &str) -> Result<Message, EmailSyncError> {
        self.route(account)?.fetch_message(account, message_id)
    }

    fn fetch_attachment(
        &self,
        account: &str,
        message_id: &str,
        part: &str,
    ) -> Result<Vec<u8>, EmailSyncError> {
        self.route(account)?
            .fetch_attachment(account, message_id, part)
    }

    fn set_flags(
        &self,
        account: &str,
        message_id: &str,
        delta: FlagDelta,
    ) -> Result<(), EmailSyncError> {
        let out = self.route(account)?.set_flags(account, message_id, delta);
        if out.is_ok() {
            self.invalidate(account);
        }
        out
    }

    fn move_message(
        &self,
        account: &str,
        message_id: &str,
        dest_folder: &str,
    ) -> Result<(), EmailSyncError> {
        let out = self
            .route(account)?
            .move_message(account, message_id, dest_folder);
        if out.is_ok() {
            self.invalidate(account);
        }
        out
    }

    fn delete_message(&self, account: &str, message_id: &str) -> Result<(), EmailSyncError> {
        let out = self.route(account)?.delete_message(account, message_id);
        if out.is_ok() {
            // Also drop the row so a stale listing cannot resurrect it.
            if let Some(store) = self.stores.get(account) {
                if let Ok(mut store) = store.lock() {
                    let _ = store.delete_message(message_id);
                }
            }
            self.invalidate(account);
        }
        out
    }

    fn append_draft(&self, account: &str, draft: Draft) -> Result<String, EmailSyncError> {
        self.route(account)?.append_draft(account, draft)
    }

    fn send(&self, account: &str, draft: Draft) -> Result<String, EmailSyncError> {
        self.route(account)?.send(account, draft)
    }
}

/// The single hub both sub-backends publish into — see the module
/// docs.
impl email_proto::EmailSyncStreamSource for Backend {
    fn changes_hub(&self) -> &architect::PubSub<email_proto::EmailChange> {
        &self.changes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn imap_cfg(id: &str) -> email_config::AccountConfig {
        email_config::AccountConfig {
            id: email_proto::AccountId(id.to_owned()),
            name: id.to_owned(),
            address: format!("{id}@example.com"),
            display_name: None,
            backend: email_config::BackendKind::Imap {
                host: "imap.example.com".into(),
                port: 993,
                tls: email_config::TlsMode::Implicit,
                username: id.to_owned(),
                password: email_secret::Secret::raw("pw"),
                submit: None,
            },
            signature: None,
            folder_aliases: email_config::FolderAliases::new(),
        }
    }

    #[test]
    fn unknown_accounts_are_rejected_not_misrouted() {
        // No tokio runtime here, so the IMAP backend can't build —
        // which is exactly the degraded path worth pinning: the mux
        // must still construct, and must not answer for an account it
        // cannot serve.
        let mux = Backend::build(Vec::new(), vec![imap_cfg("gmail")], HashMap::new());
        assert!(matches!(
            mux.list_folders("nope"),
            Err(EmailSyncError::UnknownAccount)
        ));
        // And it must never fall through to the maildir backend, which
        // would file remote mail into a local store.
        assert!(matches!(
            mux.list_folders("gmail"),
            Err(EmailSyncError::UnknownAccount)
        ));
    }

    /// IMAP's `EmailSync` methods are sync wrappers around
    /// `runtime.block_on`, which **panics if called on a runtime
    /// worker thread**. In the server they run on architect's tokio
    /// *blocking* dispatcher (hence `architect/dispatch-tokio`), so
    /// tests must call them the same way — `spawn_blocking`, never
    /// straight from an `async fn`.
    async fn blocking<T, F>(f: F) -> T
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        tokio::task::spawn_blocking(f).await.expect("join")
    }


    /// A maildir account whose store is warmed, then whose maildir is
    /// emptied underneath it. A cached read still returns the rows; an
    /// invalidated one does not. That is the whole contract, and it is
    /// observable without a network backend.
    #[tokio::test(flavor = "multi_thread")]
    async fn listings_come_from_the_index_until_invalidated() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        for sub in ["cur", "new", "tmp"] {
            std::fs::create_dir_all(root.join(sub)).unwrap();
        }
        std::fs::write(
            root.join("cur").join("1785900000.M1.host:2,S"),
            "Message-ID: <a@example.com>\r\nFrom: Alice <alice@example.com>\r\n\
             To: you@example.com\r\nSubject: Cached\r\n\
             Date: Mon, 04 Aug 2026 09:00:00 +0000\r\n\r\nbody\r\n",
        )
        .unwrap();

        let account = email_proto::Account {
            id: email_proto::AccountId("local".into()),
            name: "local".into(),
            address: "you@example.com".into(),
            display_name: None,
        };
        let entry = email_maildir::AccountEntry {
            account,
            root: root.clone(),
            aliases: email_config::FolderAliases::new(),
            submit: None,
        };
        let dirs = HashMap::from([("local".to_owned(), root.clone())]);
        let mux = Backend::build(vec![entry], Vec::new(), dirs);

        // Cold: goes to the maildir and writes through.
        let m = mux.clone();
        let first = tokio::task::spawn_blocking(move || {
            m.fetch_envelopes("local", "INBOX", SeqRange::Recent(50))
        })
        .await
        .unwrap()
        .unwrap();
        assert_eq!(first.len(), 1, "cold read hits the backend");
        assert_eq!(first[0].subject, "Cached");

        // Delete the message on disk. A backend read would now return
        // nothing; a cached read still answers.
        std::fs::remove_file(root.join("cur").join("1785900000.M1.host:2,S")).unwrap();

        let m = mux.clone();
        let cached = tokio::task::spawn_blocking(move || {
            m.fetch_envelopes("local", "INBOX", SeqRange::Recent(50))
        })
        .await
        .unwrap()
        .unwrap();
        assert_eq!(cached.len(), 1, "served from the index, not the maildir");

        // A mutation drops the mark, so the next read re-checks.
        mux.invalidate("local");
        let m = mux.clone();
        let after = tokio::task::spawn_blocking(move || {
            m.fetch_envelopes("local", "INBOX", SeqRange::Recent(50))
        })
        .await
        .unwrap()
        .unwrap();
        assert!(after.is_empty(), "invalidated: back to the backend");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn explicit_ranges_are_never_served_from_the_index() {
        // `All` / `Range` ask for something specific; the stored rows
        // may not satisfy them, so they must reach the backend.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        for sub in ["cur", "new", "tmp"] {
            std::fs::create_dir_all(root.join(sub)).unwrap();
        }
        let account = email_proto::Account {
            id: email_proto::AccountId("local".into()),
            name: "local".into(),
            address: "you@example.com".into(),
            display_name: None,
        };
        let entry = email_maildir::AccountEntry {
            account,
            root: root.clone(),
            aliases: email_config::FolderAliases::new(),
            submit: None,
        };
        let mux = Backend::build(
            vec![entry],
            Vec::new(),
            HashMap::from([("local".to_owned(), root)]),
        );
        // Not cacheable, so no freshness mark is recorded.
        let m = mux.clone();
        let _ = tokio::task::spawn_blocking(move || {
            m.fetch_envelopes("local", "INBOX", SeqRange::All)
        })
        .await
        .unwrap();
        assert!(!mux.listing_fresh("local", "INBOX"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn imap_accounts_route_to_imap() {
        let mux = Backend::build(Vec::new(), vec![imap_cfg("gmail")], HashMap::new());
        assert_eq!(mux.imap_account_ids(), vec!["gmail".to_owned()]);
        assert!(mux.imap().is_some());
        // Reachability isn't asserted (no server here) — routing is:
        // the call must reach IMAP and fail as a network/auth error,
        // not as UnknownAccount.
        let err = blocking(move || mux.list_folders("gmail")).await.unwrap_err();
        assert!(
            !matches!(err, EmailSyncError::UnknownAccount),
            "routed to imap, got {err:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn accounts_lists_both_backends() {
        let mux = Backend::build(Vec::new(), vec![imap_cfg("gmail")], HashMap::new());
        let ids: Vec<String> = blocking(move || mux.accounts())
            .await
            .unwrap()
            .into_iter()
            .map(|a| a.id.0)
            .collect();
        assert_eq!(ids, vec!["gmail".to_owned()]);
    }
}
