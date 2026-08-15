//! `EmailSync` impl backed by an IMAP server via `async-imap`.
//! Connections are opened on demand per call; a future pass
//! pools them. Folder aliases (`email-config::FolderAliases`)
//! are honored at the wire boundary, same shape as
//! `email-maildir::Backend`.

use std::collections::HashMap;
use std::sync::Arc;

use email_config::{BackendKind, FolderAliases, SmtpConfig, TlsMode};
use email_proto::{
    Account, Draft, EmailEvent, EmailSync, EmailSyncError, Envelope, FlagDelta, Folder, FolderRole,
    Message, SeqRange,
};
use futures::StreamExt;
use tokio::sync::{Mutex, RwLock, broadcast};

use crate::connect::{self, ConnectError, ImapSession};
use crate::parse;

/// One configured IMAP account. Holds the connection
/// parameters + folder alias map; the actual session is
/// opened per-op until we add pooling.
struct AccountState {
    account: Account,
    host: String,
    port: u16,
    tls: TlsMode,
    username: String,
    password: email_secret::Secret,
    aliases: FolderAliases,
    smtp: Option<SmtpConfig>,
}

/// IMAP backend. Cheap to `Clone` — all internals are `Arc`'d.
#[derive(Clone, architect::HasDispatcher)]
pub struct Backend {
    accounts: Arc<HashMap<String, AccountState>>,
    /// Per-account broadcast sender, lazily created on first
    /// `subscribe`. Same shape as `vault::sync::Backend` +
    /// `email-maildir::Backend`.
    channels: Arc<RwLock<HashMap<String, broadcast::Sender<EmailEvent>>>>,
    /// Fan-out hub behind the `#[subscribe] fn changes` stream.
    /// Every event that goes onto a per-account broadcast channel
    /// is published here too, wrapped with its `account` so
    /// subscribers — who see every account this backend serves —
    /// can filter. Sliding mailbox: a slow subscriber loses its
    /// oldest queued events and re-pulls on reconnect, which is
    /// what `EmailEvent::Resync` asks for anyway.
    changes: architect::PubSub<email_proto::EmailChange>,
    /// `account → (Message-ID → (backend folder, uid))`, filled in as
    /// envelopes are listed.
    ///
    /// The proto addresses messages by Message-ID; IMAP addresses them
    /// by UID, so every read/flag/move otherwise costs a
    /// `UID SEARCH HEADER Message-ID` per mailbox. Gmail does not index
    /// arbitrary headers, and `[Gmail]/All Mail` is a superset of the
    /// account — that search took **over a minute** on a real mailbox,
    /// which is a hang as far as anyone clicking a message is
    /// concerned.
    ///
    /// Listing a folder already returns UID and Message-ID together, so
    /// the mapping is free at exactly the moment the UI learns a
    /// message exists — and the UI always lists before it opens. Search
    /// stays as the fallback for anything not listed this session.
    /// (`email-store`'s sqlite index is the durable version of this.)
    uid_index: Arc<RwLock<HashMap<String, HashMap<String, (String, u32)>>>>,
    /// Tokio runtime needed inside the sync `EmailSync` methods.
    /// We use `block_on` via `TokioBlockingDispatcher`; this
    /// handle gives us access to the same runtime the backend
    /// was built on.
    runtime: tokio::runtime::Handle,
    /// Coarse per-account session lock. IMAP sessions are
    /// single-stream; serialize ops until we add a pool.
    locks: Arc<RwLock<HashMap<String, Arc<Mutex<()>>>>>,
}

impl Backend {
    /// Build a backend from one or more
    /// [`email_config::AccountConfig`] entries. Skips configs
    /// whose `BackendKind` isn't `Imap`. The current tokio
    /// runtime handle is captured at build time and reused for
    /// every blocking call.
    pub fn from_configs<I>(configs: I) -> Result<Self, &'static str>
    where
        I: IntoIterator<Item = email_config::AccountConfig>,
    {
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|_| "Backend::from_configs must be called from a tokio runtime")?;

        let mut accounts = HashMap::new();
        for cfg in configs {
            let BackendKind::Imap {
                host,
                port,
                tls,
                username,
                password,
                submit,
            } = cfg.backend.clone()
            else {
                continue;
            };
            let account = cfg.to_account();
            accounts.insert(
                account.id.0.clone(),
                AccountState {
                    account,
                    host,
                    port,
                    tls,
                    username,
                    password,
                    aliases: cfg.folder_aliases.clone(),
                    smtp: submit,
                },
            );
        }

        Ok(Self {
            accounts: Arc::new(accounts),
            channels: Arc::new(RwLock::new(HashMap::new())),
            changes: architect::PubSub::sliding(256),
            uid_index: Arc::new(RwLock::new(HashMap::new())),
            runtime,
            locks: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Publish this backend's change events into `hub` instead of its
    /// own.
    ///
    /// A server that serves several accounts across different backends
    /// mounts ONE `EmailSync` service, so there must be ONE stream for
    /// subscribers to attach to. `architect::PubSub` has no subscribe
    /// side to bridge with, so the multiplexer hands its hub down to
    /// each sub-backend at build time instead. Call before the backend
    /// is cloned or used.
    #[must_use]
    pub fn with_changes_hub(mut self, hub: architect::PubSub<email_proto::EmailChange>) -> Self {
        self.changes = hub;
        self
    }

    fn state(&self, account: &str) -> Result<&AccountState, EmailSyncError> {
        self.accounts
            .get(account)
            .ok_or(EmailSyncError::UnknownAccount)
    }

    async fn account_lock(&self, account: &str) -> Arc<Mutex<()>> {
        if let Some(l) = self.locks.read().await.get(account) {
            return l.clone();
        }
        let mut w = self.locks.write().await;
        w.entry(account.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    /// Per-account broadcast sender for live events.
    pub async fn channel(&self, account: &str) -> broadcast::Sender<EmailEvent> {
        if let Some(tx) = self.channels.read().await.get(account) {
            return tx.clone();
        }
        let mut chans = self.channels.write().await;
        if let Some(tx) = chans.get(account) {
            return tx.clone();
        }
        let (tx, _rx) = broadcast::channel::<EmailEvent>(256);
        chans.insert(account.to_string(), tx.clone());
        tx
    }

    /// Announce a change on both paths: `account`'s in-process
    /// broadcast channel and the wire hub. Call only once the
    /// mailbox actually changed — subscribers re-read on the event.
    pub async fn emit(&self, account: &str, event: EmailEvent) {
        let _ = self.channel(account).await.send(event.clone());
        self.changes.publish(email_proto::EmailChange {
            account: account.to_string(),
            event,
        });
    }

    /// Start a long-lived IDLE loop on `folder` (alias name).
    /// Returns a `JoinHandle` callers (typically `email-sync`)
    /// can abort to stop the loop. Server responses break IDLE
    /// every ~28 minutes (under the RFC 2177 30-minute cap) so
    /// the session never goes stale; on each break we emit
    /// `EmailEvent::Resync` — on the per-account broadcast AND
    /// the wire hub the `changes` stream serves — and re-enter
    /// IDLE.
    ///
    /// Emitting `Resync` instead of fine-grained events is
    /// intentional for phase 1 — `email-sync`'s next poll cycle
    /// will pick up the actual deltas. A future pass will parse
    /// IDLE's untagged EXISTS / EXPUNGE / FETCH responses and
    /// emit the matching `NewMessage` / `Deleted` /
    /// `FlagsChanged` events directly.
    pub async fn start_idle(
        &self,
        account: &str,
        folder: &str,
    ) -> Result<tokio::task::JoinHandle<()>, EmailSyncError> {
        let state = self.state(account)?;
        let resolved = state.aliases.resolve(folder).to_string();
        let backend = self.clone();
        let account = account.to_string();
        let handle = tokio::spawn(async move {
            backend.idle_loop(account, resolved).await;
        });
        Ok(handle)
    }

    /// Continuous IDLE driver. Reconnects on any error with a
    /// short backoff so a transient network blip doesn't kill
    /// the watcher.
    async fn idle_loop(self, account: String, folder: String) {
        const IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(28 * 60);
        const RECONNECT_BACKOFF: std::time::Duration = std::time::Duration::from_secs(5);

        loop {
            let state = match self.state(&account) {
                Ok(s) => s,
                Err(_) => return,
            };
            let session = match self.open(state).await {
                Ok(s) => s,
                Err(err) => {
                    tracing::warn!(%err, "idle: open failed, backing off");
                    tokio::time::sleep(RECONNECT_BACKOFF).await;
                    continue;
                }
            };
            // The whole IDLE cycle takes a separate borrow of
            // the session, so wrap it in a block + reassign.
            let session_result = run_idle_cycle(session, &folder, IDLE_TIMEOUT).await;
            match session_result {
                Ok(()) => {
                    // Break of IDLE = server told us something
                    // changed (or the timeout fired). Either way
                    // the safe answer is `Resync` — let
                    // `email-sync` (in-process) and every wire
                    // subscriber re-pull deltas. Keep idling
                    // regardless of who is listening: the wire hub
                    // has no "no subscribers" signal, and a
                    // subscriber can attach at any time.
                    self.emit(&account, EmailEvent::Resync).await;
                }
                Err(err) => {
                    tracing::warn!(%err, "idle: cycle failed, backing off");
                    tokio::time::sleep(RECONNECT_BACKOFF).await;
                }
            }
        }
    }

    /// Open + login. Used inside every op for now; pooling
    /// lands in the IDLE pass.
    async fn open(&self, state: &AccountState) -> Result<ImapSession, EmailSyncError> {
        let password = state
            .password
            .resolve()
            .await
            .map_err(|_| EmailSyncError::Auth)?;
        connect::connect_and_login(
            &state.host,
            state.port,
            state.tls,
            &state.username,
            &password,
        )
        .await
        .map_err(map_connect_err)
    }

    /// Drive one operation. Each opens a fresh session,
    /// performs the op, then drops the session. Sufficient for
    /// phase 2; pooling lands next.
    async fn run_list_folders(&self, state: &AccountState) -> Result<Vec<Folder>, EmailSyncError> {
        let lock = self.account_lock(&state.account.id.0).await;
        let _g = lock.lock().await;
        let mut session = self.open(state).await?;
        let mut folders = Vec::new();
        let mut stream = session
            .list(Some(""), Some("*"))
            .await
            .map_err(|e| EmailSyncError::Protocol(e.to_string()))?;
        while let Some(item) = stream.next().await {
            let m = item.map_err(|e| EmailSyncError::Protocol(e.to_string()))?;
            let backend_name = m.name().to_string();
            let delim = m.delimiter().unwrap_or("/").to_string();
            let role = infer_role(&backend_name);
            // Translate backend → UI (alias) before reporting.
            let ui_name = state
                .aliases
                .alias_for(&backend_name)
                .map_or_else(|| backend_name.clone(), str::to_string);
            folders.push(Folder {
                id: ui_name.clone(),
                name: ui_name,
                delimiter: delim,
                role,
                message_count: None,
                unread_count: None,
            });
        }
        drop(stream);
        let _ = session.logout().await;
        Ok(folders)
    }

    async fn run_fetch_envelopes(
        &self,
        state: &AccountState,
        folder: &str,
        range: SeqRange,
    ) -> Result<Vec<Envelope>, EmailSyncError> {
        // Translate UI/alias → backend name.
        let resolved = state.aliases.resolve(folder).to_string();

        let lock = self.account_lock(&state.account.id.0).await;
        let _g = lock.lock().await;
        let mut session = self.open(state).await?;
        let mailbox = session
            .select(&resolved)
            .await
            .map_err(|e| EmailSyncError::Protocol(e.to_string()))?;

        let last_uid = mailbox.uid_next.unwrap_or(1).saturating_sub(1);
        let seq = match range {
            SeqRange::All => "1:*".to_string(),
            SeqRange::Recent(n) => {
                let start = last_uid.saturating_sub(n.saturating_sub(1));
                format!("{}:{}", start.max(1), last_uid.max(1))
            }
            SeqRange::Range { from, to } => format!("{}:{}", from.max(1), to.max(1)),
        };

        let mut envs = Vec::new();
        let mut learned: Vec<(String, u32)> = Vec::new();
        let mut stream = session
            .uid_fetch(&seq, "(UID FLAGS RFC822.SIZE BODY.PEEK[HEADER])")
            .await
            .map_err(|e| EmailSyncError::Protocol(e.to_string()))?;
        while let Some(item) = stream.next().await {
            let fetch = item.map_err(|e| EmailSyncError::Protocol(e.to_string()))?;
            let header = fetch.header().unwrap_or(&[]).to_vec();
            let flags: Vec<String> = fetch.flags().map(|f| format!("{f:?}")).collect();
            let size = u64::from(fetch.size.unwrap_or(0));
            // IMAP UID isn't a Message-ID, but we use it as a
            // stable secondary key when the header lacks one.
            let uid_synth = fetch.uid.map(|u| format!("<uid-{u}@imap.local>"));
            let uid = fetch.uid;
            match parse::envelope_from_bytes(&header, folder, flags, uid_synth, size) {
                Ok(env) => {
                    if let Some(uid) = uid {
                        learned.push((env.message_id.clone(), uid));
                    }
                    envs.push(env);
                }
                Err(err) => tracing::warn!(error = %err, "envelope parse failed"),
            }
        }
        drop(stream);
        let _ = session.logout().await;

        // Record the UID mapping for everything just listed — one
        // write, outside the fetch loop.
        if !learned.is_empty() {
            let mut idx = self.uid_index.write().await;
            let per_account = idx.entry(state.account.id.0.clone()).or_default();
            // Bounded: a long-lived server listing many folders would
            // otherwise grow this without limit. Dropping it costs one
            // slow search per message afterwards, never correctness.
            if per_account.len() > 20_000 {
                per_account.clear();
            }
            for (message_id, uid) in learned {
                per_account.insert(message_id, (resolved.clone(), uid));
            }
        }

        envs.sort_by(|a, b| b.date_ms.cmp(&a.date_ms));
        Ok(envs)
    }

    /// `(backend folder, uid)` for a Message-ID we listed earlier.
    ///
    /// The fast path for every open / flag / move: the UI lists a
    /// folder before it can click anything in it, so this hits.
    async fn cached_uid(&self, account: &str, message_id: &str) -> Option<(String, u32)> {
        self.uid_index
            .read()
            .await
            .get(account)?
            .get(message_id)
            .cloned()
    }

    /// Order the mailboxes to scan when resolving a Message-ID.
    ///
    /// Ordering is the difference between "instant" and "a minute" on a
    /// real Gmail account. Gmail exposes ~15 mailboxes, and
    /// `[Gmail]/All Mail` is a *superset* of every other one — so a
    /// naive LIST-order scan can burn a dozen SELECT+SEARCH round trips
    /// before reaching the message, with the single most expensive
    /// mailbox somewhere in the middle.
    ///
    /// Inbox first, because that is where a reader almost always is.
    /// All Mail last: it matches essentially everything, which makes it
    /// the correct fallback and the worst first guess. Non-selectable
    /// container nodes (Gmail's bare `[Gmail]`) are dropped — SELECT on
    /// them always fails.
    fn search_order(folders: Vec<Folder>) -> Vec<Folder> {
        fn rank(f: &Folder) -> u8 {
            match f.role {
                Some(FolderRole::Inbox) => 0,
                // Superset mailbox — always matches, always slowest.
                Some(FolderRole::All) => 3,
                // Virtual views over other mailboxes; a hit here is a
                // duplicate of one we will reach anyway.
                Some(FolderRole::Flagged) => 2,
                _ => 1,
            }
        }
        let mut out: Vec<Folder> = folders
            .into_iter()
            .filter(|f| !f.name.eq_ignore_ascii_case("[Gmail]"))
            .collect();
        out.sort_by_key(rank);
        out
    }

    /// Find `(alias folder id, backend folder name, uid)` for a
    /// Message-ID, reusing ONE open session.
    ///
    /// The previous shape opened a fresh TLS connection and logged in
    /// *per folder*, then logged out again — up to 15 full handshakes
    /// for a single click on a Gmail message, which Gmail also
    /// rate-limits. One session, N cheap SELECTs.
    async fn locate_in_session(
        session: &mut connect::ImapSession,
        state: &AccountState,
        folders: &[Folder],
        message_id: &str,
    ) -> Option<(String, String, u32)> {
        let needle = format!(
            "HEADER Message-ID \"{}\"",
            message_id.trim_matches(|c| c == '<' || c == '>')
        );
        for folder in folders {
            let backend_name = state.aliases.resolve(&folder.id).to_string();
            if session.select(&backend_name).await.is_err() {
                continue;
            }
            let Ok(uids) = session.uid_search(&needle).await else {
                continue;
            };
            if let Some(uid) = uids.into_iter().next() {
                return Some((folder.id.clone(), backend_name, uid));
            }
        }
        None
    }

    async fn run_fetch_message(
        &self,
        state: &AccountState,
        message_id: &str,
    ) -> Result<Message, EmailSyncError> {
        // The proto identifies messages by RFC2822 Message-ID; IMAP
        // indexes by UID, so this has to search. One session, ordered
        // folders — see `locate_in_session` / `search_order`.
        // `email-store`'s index short-circuits it entirely later.
        // Fast path: we almost certainly listed this message a moment
        // ago, which taught us its folder and UID. Only fall back to
        // the per-mailbox header search when we have not.
        let cached = self.cached_uid(&state.account.id.0, message_id).await;

        let folders = match &cached {
            Some(_) => Vec::new(),
            None => Self::search_order(self.run_list_folders(state).await?),
        };
        let lock = self.account_lock(&state.account.id.0).await;
        let _g = lock.lock().await;
        let mut session = self.open(state).await?;

        let located = match cached {
            Some((backend, uid)) => {
                if session.select(&backend).await.is_ok() {
                    Some((backend.clone(), backend, uid))
                } else {
                    None
                }
            }
            None => Self::locate_in_session(&mut session, state, &folders, message_id).await,
        };
        let Some((alias_id, _backend, uid)) = located else {
            let _ = session.logout().await;
            return Err(EmailSyncError::NotFound);
        };

        let fetched = {
            let mut stream = session
                .uid_fetch(uid.to_string(), "(UID FLAGS RFC822.SIZE BODY.PEEK[])")
                .await
                .map_err(|e| EmailSyncError::Protocol(e.to_string()))?;
            match stream.next().await {
                Some(item) => {
                    let fetch = item.map_err(|e| EmailSyncError::Protocol(e.to_string()))?;
                    let body = fetch.body().unwrap_or(&[]).to_vec();
                    let flags: Vec<String> = fetch.flags().map(|f| format!("{f:?}")).collect();
                    let size = u64::from(fetch.size.unwrap_or(0));
                    Some((body, flags, size))
                }
                None => None,
            }
        };
        let _ = session.logout().await;

        let Some((body, flags, size)) = fetched else {
            return Err(EmailSyncError::NotFound);
        };
        parse::message_from_bytes(&body, &alias_id, flags, size)
    }

    /// Locate `(backend folder name, uid)` for one Message-ID.
    ///
    /// Returns the **backend** folder name (already alias-resolved) so
    /// callers can hand it straight to `session.select`. Shares the
    /// ordered single-session scan with `run_fetch_message` — this used
    /// to open a connection per folder, which made every flag / move /
    /// delete on a Gmail account as slow as a full mailbox walk.
    async fn locate_uid(
        &self,
        state: &AccountState,
        message_id: &str,
    ) -> Result<(String, u32), EmailSyncError> {
        // Same fast path as `run_fetch_message` — see `cached_uid`.
        if let Some((backend, uid)) = self.cached_uid(&state.account.id.0, message_id).await {
            return Ok((backend, uid));
        }
        let folders = Self::search_order(self.run_list_folders(state).await?);
        let lock = self.account_lock(&state.account.id.0).await;
        let _g = lock.lock().await;
        let mut session = self.open(state).await?;
        let found = Self::locate_in_session(&mut session, state, &folders, message_id).await;
        let _ = session.logout().await;
        match found {
            Some((_alias, backend, uid)) => Ok((backend, uid)),
            None => Err(EmailSyncError::NotFound),
        }
    }

    async fn run_set_flags(
        &self,
        state: &AccountState,
        message_id: &str,
        delta: FlagDelta,
    ) -> Result<(), EmailSyncError> {
        let (folder, uid) = self.locate_uid(state, message_id).await?;
        let lock = self.account_lock(&state.account.id.0).await;
        let _g = lock.lock().await;
        let mut session = self.open(state).await?;
        session
            .select(&folder)
            .await
            .map_err(|e| EmailSyncError::Protocol(e.to_string()))?;

        let uid_seq = uid.to_string();
        if !delta.add.is_empty() {
            let flags = delta.add.join(" ");
            let cmd = format!("+FLAGS ({flags})");
            // `uid_store` returns a stream of updated FETCH
            // responses; drive it to completion + discard.
            let mut stream = session
                .uid_store(&uid_seq, &cmd)
                .await
                .map_err(|e| EmailSyncError::Protocol(e.to_string()))?;
            while stream.next().await.is_some() {}
        }
        if !delta.remove.is_empty() {
            let flags = delta.remove.join(" ");
            let cmd = format!("-FLAGS ({flags})");
            let mut stream = session
                .uid_store(&uid_seq, &cmd)
                .await
                .map_err(|e| EmailSyncError::Protocol(e.to_string()))?;
            while stream.next().await.is_some() {}
        }
        let _ = session.logout().await;
        Ok(())
    }

    async fn run_move_message(
        &self,
        state: &AccountState,
        message_id: &str,
        dest: &str,
    ) -> Result<(), EmailSyncError> {
        let (source_folder, uid) = self.locate_uid(state, message_id).await?;
        // Caller's `dest` is the alias/UI name; translate.
        let dest_backend = state.aliases.resolve(dest).to_string();
        let lock = self.account_lock(&state.account.id.0).await;
        let _g = lock.lock().await;
        let mut session = self.open(state).await?;
        session
            .select(&source_folder)
            .await
            .map_err(|e| EmailSyncError::Protocol(e.to_string()))?;

        // Prefer UID MOVE (RFC 6851). async-imap's
        // `uid_mv` issues the command but doesn't gate on the
        // server advertising the MOVE capability — if the server
        // doesn't support it, we fall back to UID COPY + STORE
        // \Deleted + UID EXPUNGE.
        if let Err(err) = session.uid_mv(uid.to_string(), &dest_backend).await {
            tracing::debug!(?err, "UID MOVE failed, falling back to COPY+EXPUNGE");
            session
                .uid_copy(uid.to_string(), &dest_backend)
                .await
                .map_err(|e| EmailSyncError::Protocol(e.to_string()))?;
            {
                let s = session
                    .uid_store(uid.to_string(), "+FLAGS (\\Deleted)")
                    .await
                    .map_err(|e| EmailSyncError::Protocol(e.to_string()))?;
                let mut s = Box::pin(s);
                while s.next().await.is_some() {}
            }
            {
                let e = session
                    .uid_expunge(uid.to_string())
                    .await
                    .map_err(|e| EmailSyncError::Protocol(e.to_string()))?;
                let mut e = Box::pin(e);
                while e.next().await.is_some() {}
            }
        }
        let _ = session.logout().await;
        Ok(())
    }

    async fn run_delete_message(
        &self,
        state: &AccountState,
        message_id: &str,
    ) -> Result<(), EmailSyncError> {
        let (folder, uid) = self.locate_uid(state, message_id).await?;
        let lock = self.account_lock(&state.account.id.0).await;
        let _g = lock.lock().await;
        let mut session = self.open(state).await?;
        session
            .select(&folder)
            .await
            .map_err(|e| EmailSyncError::Protocol(e.to_string()))?;
        // Scope each `&mut session` borrow to drain its stream
        // before issuing the next command — async-imap streams
        // hold the session borrow, and `uid_expunge`'s stream is
        // not Unpin so we pin it on the heap.
        {
            let s = session
                .uid_store(uid.to_string(), "+FLAGS (\\Deleted)")
                .await
                .map_err(|e| EmailSyncError::Protocol(e.to_string()))?;
            let mut s = Box::pin(s);
            while s.next().await.is_some() {}
        }
        {
            let e = session
                .uid_expunge(uid.to_string())
                .await
                .map_err(|e| EmailSyncError::Protocol(e.to_string()))?;
            let mut e = Box::pin(e);
            while e.next().await.is_some() {}
        }
        let _ = session.logout().await;
        Ok(())
    }

    async fn run_append_draft(
        &self,
        state: &AccountState,
        draft: Draft,
    ) -> Result<String, EmailSyncError> {
        let (bytes, message_id) = email_smtp::build_message(&draft)
            .map_err(|e| EmailSyncError::Protocol(format!("draft build: {e}")))?;
        // Look up the Drafts folder via the alias map; fall
        // back to the literal name `Drafts` when unaliased.
        let drafts_folder = state.aliases.resolve("Drafts").to_string();

        let lock = self.account_lock(&state.account.id.0).await;
        let _g = lock.lock().await;
        let mut session = self.open(state).await?;
        session
            .append(&drafts_folder, Some("(\\Draft)"), None, &bytes)
            .await
            .map_err(|e| EmailSyncError::Protocol(format!("APPEND: {e}")))?;
        let _ = session.logout().await;
        Ok(message_id)
    }

    async fn run_send(&self, state: &AccountState, draft: Draft) -> Result<String, EmailSyncError> {
        let smtp = state.smtp.clone().ok_or_else(|| {
            EmailSyncError::Unsupported(
                "imap: send requires SmtpConfig on the account (submit field)".into(),
            )
        })?;
        let sender = email_smtp::SmtpSender::new(smtp);
        let message_id = sender
            .send(&draft)
            .await
            .map_err(|e| EmailSyncError::Protocol(format!("smtp: {e}")))?;

        // After a successful submit, append a sent copy to the
        // server's Sent folder. Best-effort — the message is
        // already on the wire; an APPEND failure shouldn't
        // surface as a send failure.
        let sent_folder = state.aliases.resolve("Sent").to_string();
        if let Ok((bytes, _)) = email_smtp::build_message(&draft) {
            let lock = self.account_lock(&state.account.id.0).await;
            let _g = lock.lock().await;
            match self.open(state).await {
                Ok(mut session) => {
                    let _ = session
                        .append(&sent_folder, Some("(\\Seen)"), None, &bytes)
                        .await;
                    let _ = session.logout().await;
                }
                Err(err) => {
                    tracing::warn!(?err, "append sent-copy: open failed");
                }
            }
        }

        Ok(message_id)
    }
}

/// Run one IDLE round on `session`: SELECT, IDLE for at most
/// `timeout`, then DONE. Drops the session when complete (the
/// caller opens a fresh one for each cycle so a stale TLS
/// connection doesn't accumulate).
async fn run_idle_cycle(
    mut session: ImapSession,
    folder: &str,
    timeout: std::time::Duration,
) -> Result<(), EmailSyncError> {
    session
        .select(folder)
        .await
        .map_err(|e| EmailSyncError::Protocol(format!("idle select: {e}")))?;
    let mut idle = session.idle();
    idle.init()
        .await
        .map_err(|e| EmailSyncError::Protocol(format!("idle init: {e}")))?;
    let (idle_wait, _interrupt) = idle.wait_with_timeout(timeout);
    // We discard the response detail — `idle_loop` translates
    // any break into `EmailEvent::Resync` and lets `email-sync`
    // re-pull the deltas. Parsing the untagged EXISTS / EXPUNGE
    // / FETCH details for fine-grained events is the next pass.
    let _ = idle_wait.await;
    let mut session = idle
        .done()
        .await
        .map_err(|e| EmailSyncError::Protocol(format!("idle done: {e}")))?;
    let _ = session.logout().await;
    Ok(())
}

impl EmailSync for Backend {
    fn accounts(&self) -> Result<Vec<Account>, EmailSyncError> {
        Ok(self.accounts.values().map(|s| s.account.clone()).collect())
    }

    fn list_folders(&self, account: &str) -> Result<Vec<Folder>, EmailSyncError> {
        let state = self.state(account)?;
        // The trait is sync; we hop into the runtime via
        // `block_on`. Same pattern as `vault::sync::Backend`'s
        // tokio bridge.
        let backend = self.clone();
        let account = state.account.id.0.clone();
        self.runtime.block_on(async move {
            let state = backend.state(&account)?;
            backend.run_list_folders(state).await
        })
    }

    fn fetch_envelopes(
        &self,
        account: &str,
        folder: &str,
        range: SeqRange,
    ) -> Result<Vec<Envelope>, EmailSyncError> {
        let backend = self.clone();
        let account = account.to_string();
        let folder = folder.to_string();
        self.runtime.block_on(async move {
            let state = backend.state(&account)?;
            backend.run_fetch_envelopes(state, &folder, range).await
        })
    }

    fn fetch_message(&self, account: &str, message_id: &str) -> Result<Message, EmailSyncError> {
        let backend = self.clone();
        let account = account.to_string();
        let message_id = message_id.to_string();
        self.runtime.block_on(async move {
            let state = backend.state(&account)?;
            backend.run_fetch_message(state, &message_id).await
        })
    }

    fn fetch_attachment(
        &self,
        account: &str,
        message_id: &str,
        part: &str,
    ) -> Result<Vec<u8>, EmailSyncError> {
        // Re-fetch the whole message, then descend the parsed
        // MIME structure. Wasteful but correct; the cache in
        // `email-store` lets us skip the second hit.
        let msg = self.fetch_message(account, message_id)?;
        let _ = msg; // body bytes aren't kept on Message; will
        // need a fetch path that returns raw rfc822.
        // For now require the cache layer.
        let _ = part;
        Err(EmailSyncError::Unsupported(
            "imap: fetch_attachment via direct call needs email-store cache (phase 5)".into(),
        ))
    }

    fn set_flags(
        &self,
        account: &str,
        message_id: &str,
        delta: FlagDelta,
    ) -> Result<(), EmailSyncError> {
        let backend = self.clone();
        let account = account.to_string();
        let message_id = message_id.to_string();
        self.runtime.block_on(async move {
            let state = backend.state(&account)?;
            backend.run_set_flags(state, &message_id, delta).await
        })
    }

    fn move_message(
        &self,
        account: &str,
        message_id: &str,
        dest_folder: &str,
    ) -> Result<(), EmailSyncError> {
        let backend = self.clone();
        let account = account.to_string();
        let message_id = message_id.to_string();
        let dest = dest_folder.to_string();
        self.runtime.block_on(async move {
            let state = backend.state(&account)?;
            backend.run_move_message(state, &message_id, &dest).await
        })
    }

    fn delete_message(&self, account: &str, message_id: &str) -> Result<(), EmailSyncError> {
        let backend = self.clone();
        let account = account.to_string();
        let message_id = message_id.to_string();
        self.runtime.block_on(async move {
            let state = backend.state(&account)?;
            backend.run_delete_message(state, &message_id).await
        })
    }

    fn append_draft(&self, account: &str, draft: Draft) -> Result<String, EmailSyncError> {
        let backend = self.clone();
        let account = account.to_string();
        self.runtime.block_on(async move {
            let state = backend.state(&account)?;
            backend.run_append_draft(state, draft).await
        })
    }

    fn send(&self, account: &str, draft: Draft) -> Result<String, EmailSyncError> {
        let backend = self.clone();
        let account = account.to_string();
        self.runtime.block_on(async move {
            let state = backend.state(&account)?;
            backend.run_send(state, draft).await
        })
    }
}

fn map_connect_err(e: ConnectError) -> EmailSyncError {
    match e {
        ConnectError::Tcp(s) | ConnectError::Greeting(s) => EmailSyncError::Network(s),
        ConnectError::Tls(s) => EmailSyncError::Network(format!("tls: {s}")),
        ConnectError::Login(_) => EmailSyncError::Auth,
        ConnectError::StarttlsUnsupported => {
            EmailSyncError::Unsupported("starttls not yet implemented".into())
        }
        ConnectError::PlaintextRefused => EmailSyncError::Unsupported("plaintext refused".into()),
    }
}

fn infer_role(name: &str) -> Option<email_proto::FolderRole> {
    let lower = name.to_ascii_lowercase();
    let leaf = lower.rsplit(['/', '.']).next().unwrap_or(&lower);
    match leaf {
        "inbox" => Some(email_proto::FolderRole::Inbox),
        "drafts" | "draft" => Some(email_proto::FolderRole::Drafts),
        "sent" | "sent items" | "sent mail" | "sent messages" => {
            Some(email_proto::FolderRole::Sent)
        }
        "trash" | "deleted" | "deleted items" | "bin" => Some(email_proto::FolderRole::Trash),
        "junk" | "spam" => Some(email_proto::FolderRole::Junk),
        "archive" | "archives" | "all mail" => Some(email_proto::FolderRole::Archive),
        "outbox" => Some(email_proto::FolderRole::Outbox),
        "flagged" | "starred" => Some(email_proto::FolderRole::Flagged),
        _ => None,
    }
}

/// The `#[subscribe]` backend contract: the hub the stream host
/// attaches subscriber sinks to. The IDLE watcher publishes into
/// it — an IMAP server breaking IDLE means "something changed",
/// which is exactly `EmailEvent::Resync`.
impl email_proto::EmailSyncStreamSource for Backend {
    fn changes_hub(&self) -> &architect::PubSub<email_proto::EmailChange> {
        &self.changes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infer_role_matches_common_names() {
        assert_eq!(infer_role("INBOX"), Some(email_proto::FolderRole::Inbox));
        assert_eq!(infer_role("Sent"), Some(email_proto::FolderRole::Sent));
        assert_eq!(
            infer_role("[Gmail]/Sent Mail"),
            Some(email_proto::FolderRole::Sent)
        );
        assert_eq!(infer_role("Lists.rust-users"), None);
    }
}
