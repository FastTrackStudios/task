//! `EmailSync` impl backed by a Maildir tree on disk. Mirrors
//! the layered shape of `vault::sync::Backend`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use email_config::FolderAliases;
use email_proto::{
    Account, Draft, EmailEvent, EmailSync, EmailSyncError, Envelope, FlagDelta, Folder, Message,
    SeqRange,
};
use maildir::Maildir;
use tokio::sync::{RwLock, broadcast};

use crate::folder::{FolderName, infer_role};
use crate::parse;
use crate::submit::Submit;

/// Filesystem-backed `EmailSync` implementation. Cheap to
/// `Clone` — internals are `Arc`'d. One backend serves
/// one-or-more accounts; each account maps to a root directory
/// shaped like a Maildir++ tree (root = INBOX, sibling `.Foo`
/// dirs for sub-mailboxes).
#[derive(Clone, architect::HasDispatcher)]
pub struct Backend {
    accounts: Arc<HashMap<String, AccountState>>,
    /// Per-account broadcast sender, lazily created on first
    /// `subscribe`. Same shape + capacity as
    /// `vault::sync::Backend`.
    channels: Arc<RwLock<HashMap<String, broadcast::Sender<EmailEvent>>>>,
    /// Fan-out hub behind the `#[subscribe] fn changes` stream.
    /// Every event that goes onto a per-account broadcast channel
    /// is published here too, wrapped with its `account` so
    /// subscribers — who see every account this backend serves —
    /// can filter. Sliding mailbox: a slow subscriber loses its
    /// oldest queued events and re-pulls on reconnect, which is
    /// what `EmailEvent::Resync` asks for anyway.
    changes: architect::PubSub<email_proto::EmailChange>,
    /// Tokio runtime handle for the sync `send` method's hop into
    /// the async submitter (`Handle::block_on` is safe because
    /// the trait methods run on the blocking pool via
    /// `TokioBlockingDispatcher`). `None` when the backend was
    /// built outside a runtime — read paths don't need it, and
    /// `send` then reports `Internal`.
    runtime: Option<tokio::runtime::Handle>,
}

/// One account as [`Backend::with_configured_accounts`] consumes
/// it: identity + maildir root + folder aliases + an optional
/// outbound submitter (SMTP in production, a mock in tests). An
/// account without a submitter can't send.
pub struct AccountEntry {
    pub account: Account,
    pub root: PathBuf,
    pub aliases: FolderAliases,
    pub submit: Option<Arc<dyn Submit>>,
}

struct AccountState {
    account: Account,
    root: PathBuf,
    /// Wire-side ↔ backend-side folder name translation. Empty
    /// by default; populated from `AccountConfig::folder_aliases`.
    aliases: FolderAliases,
    /// Outbound transport for `send`. `None` = sending
    /// unsupported for this account.
    submit: Option<Arc<dyn Submit>>,
}

impl Backend {
    /// Build a backend serving a single account rooted at
    /// `root`. The directory + its `cur/new/tmp` subdirs are
    /// created on demand. No folder aliases — see
    /// [`Backend::single_with_aliases`] when the account config
    /// declares them.
    pub fn single(account: Account, root: PathBuf) -> std::io::Result<Self> {
        Self::single_with_aliases(account, root, FolderAliases::new())
    }

    /// Build a backend with an explicit folder-alias map.
    /// `aliases` is consulted in both directions: incoming
    /// folder names are translated to backend names before
    /// disk I/O; outgoing folder listings are translated back
    /// to alias names for the UI.
    pub fn single_with_aliases(
        account: Account,
        root: PathBuf,
        aliases: FolderAliases,
    ) -> std::io::Result<Self> {
        ensure_maildir(&root)?;
        let mut accounts = HashMap::with_capacity(1);
        let id = account.id.0.clone();
        accounts.insert(
            id,
            AccountState {
                account,
                root,
                aliases,
                submit: None,
            },
        );
        Ok(Self {
            accounts: Arc::new(accounts),
            channels: Arc::new(RwLock::new(HashMap::new())),
            changes: architect::PubSub::sliding(256),
            runtime: tokio::runtime::Handle::try_current().ok(),
        })
    }

    /// Build a backend from a pre-built `(Account, root, aliases)`
    /// set. All roots must exist as Maildir trees (use
    /// [`Backend::single_with_aliases`] to bootstrap). No
    /// submitters — accounts built this way can't send; use
    /// [`Backend::with_configured_accounts`] when the account
    /// config carries an SMTP submit endpoint.
    pub fn with_accounts<I>(entries: I) -> Self
    where
        I: IntoIterator<Item = (Account, PathBuf, FolderAliases)>,
    {
        Self::with_configured_accounts(entries.into_iter().map(|(account, root, aliases)| {
            AccountEntry {
                account,
                root,
                aliases,
                submit: None,
            }
        }))
    }

    /// Build a backend from full [`AccountEntry`] descriptions —
    /// the constructor the server uses once account discovery has
    /// resolved each account's config (aliases + optional SMTP
    /// submitter).
    pub fn with_configured_accounts<I>(entries: I) -> Self
    where
        I: IntoIterator<Item = AccountEntry>,
    {
        let mut accounts = HashMap::new();
        for entry in entries {
            accounts.insert(
                entry.account.id.0.clone(),
                AccountState {
                    account: entry.account,
                    root: entry.root,
                    aliases: entry.aliases,
                    submit: entry.submit,
                },
            );
        }
        Self {
            accounts: Arc::new(accounts),
            channels: Arc::new(RwLock::new(HashMap::new())),
            changes: architect::PubSub::sliding(256),
            runtime: tokio::runtime::Handle::try_current().ok(),
        }
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

    /// Get-or-create the per-account broadcast sender.
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

    /// Announce a committed change on both paths: `account`'s
    /// in-process broadcast channel and the wire hub. Call only
    /// after the mailbox actually changed — subscribers re-read on
    /// the event.
    pub async fn emit(&self, account: &str, event: EmailEvent) {
        let _ = self.channel(account).await.send(event.clone());
        self.changes.publish(email_proto::EmailChange {
            account: account.to_string(),
            event,
        });
    }

    /// [`Self::emit`] from the synchronous `EmailSync` methods.
    ///
    /// The trait's mutations are sync, but `emit` needs the async
    /// channel map. When a runtime handle is available we hand the
    /// publish to it; a change that fails to announce must NOT fail the
    /// mutation, which already committed to disk — the worst case is a
    /// client that refreshes a beat later.
    fn emit_blocking(&self, account: &str, event: EmailEvent) {
        let Some(runtime) = self.runtime.clone() else {
            // No runtime (unit tests): the write still happened.
            self.changes.publish(email_proto::EmailChange {
                account: account.to_string(),
                event,
            });
            return;
        };
        let (this_changes, account, ev) = (self.changes.clone(), account.to_string(), event);
        // `block_on` from inside a runtime worker would panic, so this
        // is spawned rather than awaited.
        let channels = self.channels.clone();
        runtime.spawn(async move {
            if let Some(tx) = channels.read().await.get(&account) {
                let _ = tx.send(ev.clone());
            }
            this_changes.publish(email_proto::EmailChange { account, event: ev });
        });
    }

    /// Find a message by RFC-2822 Message-ID.
    ///
    /// Returns `(folder alias id, maildir entry id, is_in_new)`. The
    /// maildir crate addresses entries by their filename-derived id,
    /// and only within `cur` — hence the third element, which every
    /// mutation uses to promote out of `new` first.
    ///
    /// O(N) over the tree, same as [`EmailSync::fetch_message`]; the
    /// `email-store` SQLite index makes it O(1) when that is wired in.
    fn locate(
        &self,
        account: &str,
        message_id: &str,
    ) -> Result<(String, String, bool), EmailSyncError> {
        let state = self.state(account)?;
        let want = message_id.trim_matches(|c| c == '<' || c == '>');
        for folder in self.list_folders(account)? {
            let backend_name = state.aliases.resolve(&folder.id).to_string();
            let Some(path) = FolderName(backend_name).to_path(&state.root) else {
                continue;
            };
            if !is_maildir(&path) {
                continue;
            }
            let md = Maildir::from(path);
            for (entry, in_new) in md
                .list_cur()
                .map(|e| (e, false))
                .chain(md.list_new().map(|e| (e, true)))
            {
                let Ok(entry) = entry else { continue };
                let Ok(bytes) = std::fs::read(entry.path()) else {
                    continue;
                };
                let Ok(env) = parse::envelope_from_bytes(&bytes, &folder.id, Vec::new()) else {
                    continue;
                };
                if env.message_id.trim_matches(|c| c == '<' || c == '>') == want {
                    return Ok((folder.id.clone(), entry.id().to_owned(), in_new));
                }
            }
        }
        Err(EmailSyncError::NotFound)
    }

    /// The `Maildir` handle for one alias-side folder name.
    fn folder_maildir(
        &self,
        state: &AccountState,
        folder: &str,
    ) -> Result<Maildir, EmailSyncError> {
        let backend_name = state.aliases.resolve(folder).to_string();
        let path = FolderName(backend_name)
            .to_path(&state.root)
            .ok_or_else(|| EmailSyncError::Protocol(format!("bad folder name: {folder}")))?;
        Ok(Maildir::from(path))
    }
}

/// Proto flag string → the maildir filename flag letter.
///
/// The proto deliberately carries free-form strings so IMAP keywords
/// and JMAP labels pass through untranslated, and backends differ on
/// whether they emit the leading backslash — so both spellings map, as
/// does the bare maildir letter. Anything else is a custom keyword
/// maildir cannot represent in a filename; it is dropped rather than
/// failing the whole call.
fn flag_letter(flag: &str) -> Option<char> {
    match flag.trim_start_matches('\\') {
        "Seen" | "S" => Some('S'),
        "Flagged" | "F" => Some('F'),
        "Answered" | "Replied" | "R" => Some('R'),
        "Draft" | "D" => Some('D'),
        "Deleted" | "Trashed" | "T" => Some('T'),
        _ => None,
    }
}

impl EmailSync for Backend {
    fn accounts(&self) -> Result<Vec<Account>, EmailSyncError> {
        Ok(self.accounts.values().map(|s| s.account.clone()).collect())
    }

    fn list_folders(&self, account: &str) -> Result<Vec<Folder>, EmailSyncError> {
        let state = self.state(account)?;
        let mut folders = Vec::new();

        // INBOX is the root itself.
        let (m, u) = folder_counts(&state.root);
        folders.push(Folder {
            id: FolderName::INBOX.into(),
            name: FolderName::INBOX.into(),
            delimiter: ".".into(),
            role: Some(email_proto::FolderRole::Inbox),
            message_count: Some(m),
            unread_count: Some(u),
        });

        // Sibling `.Foo` dirs are sub-mailboxes.
        let dir = match std::fs::read_dir(&state.root) {
            Ok(d) => d,
            Err(_) => return Ok(folders),
        };
        for entry in dir.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = match entry.file_name().into_string() {
                Ok(s) => s,
                Err(_) => continue,
            };
            let Some(backend_name) = FolderName::from_maildir_dir_name(&name) else {
                continue;
            };
            if !is_maildir(&path) {
                continue;
            }
            let (m, u) = folder_counts(&path);
            // Translate backend → UI/alias before reporting. If
            // the user has aliased `[Gmail]/Sent Mail` to
            // `Sent`, the UI sees `Sent` here.
            let ui_name = state
                .aliases
                .alias_for(&backend_name.0)
                .map_or_else(|| backend_name.0.clone(), str::to_string);
            folders.push(Folder {
                id: ui_name.clone(),
                name: ui_name.clone(),
                delimiter: ".".into(),
                role: infer_role(&ui_name),
                message_count: Some(m),
                unread_count: Some(u),
            });
        }

        Ok(folders)
    }

    fn fetch_envelopes(
        &self,
        account: &str,
        folder: &str,
        range: SeqRange,
    ) -> Result<Vec<Envelope>, EmailSyncError> {
        let state = self.state(account)?;
        // Translate UI/alias → backend name before resolving on
        // disk. Pass-through when no alias is registered.
        let resolved = state.aliases.resolve(folder);
        let path = FolderName(resolved.to_string())
            .to_path(&state.root)
            .ok_or(EmailSyncError::NotFound)?;
        if !is_maildir(&path) {
            return Err(EmailSyncError::NotFound);
        }
        let md = Maildir::from(path);
        let mut envs: Vec<Envelope> = Vec::new();
        for entry in md.list_cur().chain(md.list_new()) {
            let entry = match entry {
                Ok(e) => e,
                Err(err) => {
                    tracing::warn!(error = %err, "skipping unreadable maildir entry");
                    continue;
                }
            };
            let bytes = match std::fs::read(entry.path()) {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!(error = %e, "read failed");
                    continue;
                }
            };
            let flags = entry.flags().chars().map(|c| c.to_string()).collect();
            match parse::envelope_from_bytes(&bytes, folder, flags) {
                Ok(env) => envs.push(env),
                Err(err) => tracing::warn!(error = %err, "parse failed"),
            }
        }

        // Newest-first; slice per requested range.
        envs.sort_by(|a, b| b.date_ms.cmp(&a.date_ms));
        let envs = match range {
            SeqRange::All => envs,
            SeqRange::Recent(n) => envs.into_iter().take(n as usize).collect(),
            SeqRange::Range { from, to } => {
                let from = from as usize;
                let to = to as usize;
                envs.into_iter()
                    .enumerate()
                    .filter(|(i, _)| *i >= from && *i <= to)
                    .map(|(_, e)| e)
                    .collect()
            }
        };
        Ok(envs)
    }

    fn fetch_message(&self, account: &str, message_id: &str) -> Result<Message, EmailSyncError> {
        let state = self.state(account)?;
        // Scan INBOX + sub-mailboxes for a matching Message-ID.
        // O(N) — acceptable for the read-only walker; the SQLite
        // index in `email-store` makes this O(1) later.
        let folders = self.list_folders(account)?;
        for folder in folders {
            // `folder.id` is the alias-side name; resolve to
            // the backend name before touching disk.
            let backend_name = state.aliases.resolve(&folder.id).to_string();
            let Some(path) = FolderName(backend_name).to_path(&state.root) else {
                continue;
            };
            if !is_maildir(&path) {
                continue;
            }
            let md = Maildir::from(path);
            for entry in md.list_cur().chain(md.list_new()) {
                let entry = match entry {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                let bytes = match std::fs::read(entry.path()) {
                    Ok(b) => b,
                    Err(_) => continue,
                };
                let flags: Vec<String> = entry.flags().chars().map(|c| c.to_string()).collect();
                let Ok(env) = parse::envelope_from_bytes(&bytes, &folder.id, flags.clone()) else {
                    continue;
                };
                if env.message_id == message_id
                    || env.message_id.trim_matches(|c| c == '<' || c == '>')
                        == message_id.trim_matches(|c| c == '<' || c == '>')
                {
                    return parse::message_from_bytes(&bytes, &folder.id, flags);
                }
            }
        }
        Err(EmailSyncError::NotFound)
    }

    fn fetch_attachment(
        &self,
        account: &str,
        message_id: &str,
        part: &str,
    ) -> Result<Vec<u8>, EmailSyncError> {
        // Re-resolve to bytes via the same scan. Future: index
        // by message-id in email-store.
        let state = self.state(account)?;
        let folders = self.list_folders(account)?;
        for folder in folders {
            // `folder.id` is the alias-side name; resolve to
            // the backend name before touching disk.
            let backend_name = state.aliases.resolve(&folder.id).to_string();
            let Some(path) = FolderName(backend_name).to_path(&state.root) else {
                continue;
            };
            if !is_maildir(&path) {
                continue;
            }
            let md = Maildir::from(path);
            for entry in md.list_cur().chain(md.list_new()) {
                let entry = match entry {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                let bytes = match std::fs::read(entry.path()) {
                    Ok(b) => b,
                    Err(_) => continue,
                };
                let Ok(env) = parse::envelope_from_bytes(&bytes, &folder.id, Vec::new()) else {
                    continue;
                };
                if env.message_id == message_id {
                    return parse::attachment_bytes(&bytes, part);
                }
            }
        }
        Err(EmailSyncError::NotFound)
    }

    fn set_flags(
        &self,
        account: &str,
        message_id: &str,
        delta: FlagDelta,
    ) -> Result<(), EmailSyncError> {
        let state = self.state(account)?;
        let (folder, id, in_new) = self.locate(account, message_id)?;
        let md = self.folder_maildir(state, &folder)?;

        // Maildir keeps flags in the filename, and only for messages
        // in `cur` — a message still in `new` has nowhere to put them.
        // Promote first; that is also exactly what "mark as read"
        // means in maildir terms.
        if in_new {
            md.move_new_to_cur(&id)
                .map_err(|e| EmailSyncError::Io(format!("move new→cur: {e}")))?;
        }

        let add: String = delta.add.iter().filter_map(|f| flag_letter(f)).collect();
        let remove: String = delta.remove.iter().filter_map(|f| flag_letter(f)).collect();
        if !add.is_empty() {
            md.add_flags(&id, &add)
                .map_err(|e| EmailSyncError::Io(format!("add flags: {e}")))?;
        }
        if !remove.is_empty() {
            md.remove_flags(&id, &remove)
                .map_err(|e| EmailSyncError::Io(format!("remove flags: {e}")))?;
        }

        // Report the resulting flag set, not the delta — subscribers
        // re-read anyway, and the absolute set is what the event type
        // promises.
        let flags = md
            .find(&id)
            .map(|e| e.flags().chars().map(|c| c.to_string()).collect())
            .unwrap_or_default();
        self.emit_blocking(
            account,
            EmailEvent::FlagsChanged {
                message_id: message_id.to_owned(),
                flags,
            },
        );
        Ok(())
    }

    fn move_message(
        &self,
        account: &str,
        message_id: &str,
        dest_folder: &str,
    ) -> Result<(), EmailSyncError> {
        let state = self.state(account)?;
        let (folder, id, in_new) = self.locate(account, message_id)?;
        if folder == dest_folder {
            return Ok(());
        }
        let src = self.folder_maildir(state, &folder)?;
        // Create the destination on demand: Archive / Trash usually do
        // not exist yet in a fixture mailbox, and failing the first
        // Archive click because of that would be silly.
        let dest_backend = state.aliases.resolve(dest_folder).to_string();
        let dest_path = FolderName(dest_backend)
            .to_path(&state.root)
            .ok_or_else(|| EmailSyncError::Protocol(format!("bad folder name: {dest_folder}")))?;
        ensure_maildir(&dest_path).map_err(|e| EmailSyncError::Io(e.to_string()))?;

        // `move_to` addresses the entry by maildir id, which only
        // resolves in `cur` — promote a still-unread message first.
        if in_new {
            src.move_new_to_cur(&id)
                .map_err(|e| EmailSyncError::Io(format!("move new→cur: {e}")))?;
        }
        src.move_to(&id, &Maildir::from(dest_path))
            .map_err(|e| EmailSyncError::Io(format!("move message: {e}")))?;

        self.emit_blocking(
            account,
            EmailEvent::Moved {
                message_id: message_id.to_owned(),
                from_folder: folder,
                to_folder: dest_folder.to_owned(),
            },
        );
        Ok(())
    }

    fn delete_message(&self, account: &str, message_id: &str) -> Result<(), EmailSyncError> {
        let state = self.state(account)?;
        // Idempotent per the proto contract: deleting a message that
        // is already gone succeeds.
        let (folder, id, in_new) = match self.locate(account, message_id) {
            Ok(found) => found,
            Err(EmailSyncError::NotFound) => return Ok(()),
            Err(e) => return Err(e),
        };
        let md = self.folder_maildir(state, &folder)?;
        if in_new {
            md.move_new_to_cur(&id)
                .map_err(|e| EmailSyncError::Io(format!("move new→cur: {e}")))?;
        }
        md.delete(&id)
            .map_err(|e| EmailSyncError::Io(format!("delete message: {e}")))?;

        self.emit_blocking(
            account,
            EmailEvent::Deleted {
                message_id: message_id.to_owned(),
            },
        );
        Ok(())
    }

    fn append_draft(&self, _account: &str, _draft: Draft) -> Result<String, EmailSyncError> {
        Err(EmailSyncError::Unsupported(
            "maildir: append_draft lands in phase 3".into(),
        ))
    }

    /// Submit `draft` through the account's configured
    /// [`Submit`] transport, write the sent copy into the
    /// account's `Sent` maildir, and publish
    /// `EmailEvent::NewMessage { folder: "Sent" }` on the changes
    /// stream — publish-after-write, so subscribers re-reading on
    /// the event see the sent copy.
    fn send(&self, account: &str, draft: Draft) -> Result<String, EmailSyncError> {
        let state = self.state(account)?;
        let submit = state.submit.clone().ok_or_else(|| {
            EmailSyncError::Unsupported(
                "maildir: no SMTP submitter configured for this account \
                 (set `submit` in the account config)"
                    .into(),
            )
        })?;
        let runtime = self.runtime.clone().ok_or_else(|| {
            EmailSyncError::Internal("maildir: send needs a tokio runtime".into())
        })?;

        let (bytes, message_id) = email_smtp::build_message(&draft)
            .map_err(|e| EmailSyncError::Protocol(format!("draft build: {e}")))?;
        let recipients: Vec<String> = draft
            .to
            .iter()
            .chain(&draft.cc)
            .chain(&draft.bcc)
            .map(|a| a.email.clone())
            .collect();
        if recipients.is_empty() {
            return Err(EmailSyncError::Protocol("draft has no recipients".into()));
        }

        // Submit first — the message hitting the wire is the
        // thing that must not happen twice, so any failure below
        // (sent-copy write) is reported but doesn't retract the
        // submission.
        let message_id = runtime
            .block_on(submit.submit_raw(&draft.from.email, &recipients, &bytes, message_id))
            .map_err(|e| EmailSyncError::Protocol(format!("submit: {e}")))?;

        // Sent copy into the maildir. Alias-resolve the
        // conventional `Sent` name; create the folder on demand.
        let sent_backend = state.aliases.resolve("Sent").to_string();
        let path = FolderName(sent_backend)
            .to_path(&state.root)
            .ok_or_else(|| EmailSyncError::Internal("bad Sent folder name".into()))?;
        ensure_maildir(&path).map_err(|e| EmailSyncError::Io(e.to_string()))?;
        Maildir::from(path)
            .store_cur_with_flags(&bytes, "S")
            .map_err(|e| EmailSyncError::Io(format!("sent copy: {e}")))?;

        runtime.block_on(self.emit(
            account,
            EmailEvent::NewMessage {
                folder: "Sent".into(),
                message_id: message_id.clone(),
            },
        ));

        Ok(message_id)
    }
}

/// The `#[subscribe]` backend contract: the hub the stream host
/// attaches subscriber sinks to. Publishing happens in
/// [`Backend::emit`].
///
/// Publishers today: `send` (a `NewMessage` for the Sent copy),
/// the mutations (`set_flags` → `FlagsChanged`, `move_message` →
/// `Moved`, `delete_message` → `Deleted`), and the
/// `email-product` backend (outbox / derivation events, via a
/// clone of this hub). There is still no filesystem watcher on
/// the mail root, so mail delivered *externally* into the
/// maildir raises no event until that lands. `email-imap`
/// already publishes from its IDLE loop.
impl email_proto::EmailSyncStreamSource for Backend {
    fn changes_hub(&self) -> &architect::PubSub<email_proto::EmailChange> {
        &self.changes
    }
}

/// Create `cur` / `new` / `tmp` under `root` if missing.
fn ensure_maildir(root: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(root.join("cur"))?;
    std::fs::create_dir_all(root.join("new"))?;
    std::fs::create_dir_all(root.join("tmp"))?;
    Ok(())
}

fn is_maildir(path: &std::path::Path) -> bool {
    path.join("cur").is_dir() && path.join("new").is_dir()
}

/// Quick counts by directory listing — no parse needed. The
/// `cur`/Seen-flag check is approximate (any file whose
/// info-section lacks `S` is treated as unread).
fn folder_counts(path: &std::path::Path) -> (u32, u32) {
    let mut total = 0u32;
    let mut unread = 0u32;
    for sub in ["new", "cur"] {
        let dir = path.join(sub);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if !entry.path().is_file() {
                continue;
            }
            total += 1;
            let name = entry.file_name().to_string_lossy().into_owned();
            // Maildir info section: `…:2,FLAGS` — `S` means
            // Seen. Files in `new/` are never seen yet.
            let seen = sub == "cur"
                && name
                    .split(":2,")
                    .nth(1)
                    .is_some_and(|info| info.contains('S'));
            if !seen {
                unread += 1;
            }
        }
    }
    (total, unread)
}

#[cfg(test)]
mod tests {
    use super::*;
    use email_proto::{Account, AccountId};

    fn write_msg(path: &std::path::Path, name: &str, body: &str) {
        std::fs::write(path.join(name), body).unwrap();
    }

    fn fixture() -> (tempfile::TempDir, Backend, Account) {
        let dir = tempfile::tempdir().unwrap();
        let account = Account {
            id: AccountId("scratch".into()),
            name: "scratch".into(),
            address: "you@example.com".into(),
            display_name: None,
        };
        let backend = Backend::single(account.clone(), dir.path().to_path_buf()).unwrap();
        // INBOX message.
        write_msg(
            &dir.path().join("new"),
            "1700000000.M1P1.host",
            "Message-ID: <a@example.com>\r\n\
             From: Alice <alice@example.com>\r\n\
             To: you@example.com\r\n\
             Subject: Hello\r\n\
             Date: Mon, 14 Nov 2023 12:00:00 +0000\r\n\
             \r\n\
             Body text.\r\n",
        );
        // Sent sub-mailbox.
        let sent = dir.path().join(".Sent");
        std::fs::create_dir_all(sent.join("cur")).unwrap();
        std::fs::create_dir_all(sent.join("new")).unwrap();
        std::fs::create_dir_all(sent.join("tmp")).unwrap();
        write_msg(
            &sent.join("cur"),
            "1700000001.M1P2.host:2,S",
            "Message-ID: <b@example.com>\r\n\
             From: you@example.com\r\n\
             To: Bob <bob@example.com>\r\n\
             Subject: Re: Hello\r\n\
             In-Reply-To: <a@example.com>\r\n\
             Date: Mon, 14 Nov 2023 13:00:00 +0000\r\n\
             \r\n\
             Reply.\r\n",
        );
        (dir, backend, account)
    }

    #[test]
    fn list_folders_returns_inbox_and_sent() {
        let (_dir, backend, account) = fixture();
        let folders = backend.list_folders(&account.id.0).unwrap();
        let names: Vec<_> = folders.iter().map(|f| f.id.as_str()).collect();
        assert!(names.contains(&"INBOX"), "got: {names:?}");
        assert!(names.contains(&"Sent"), "got: {names:?}");
    }

    #[test]
    fn fetch_envelopes_parses_headers() {
        let (_dir, backend, account) = fixture();
        let envs = backend
            .fetch_envelopes(&account.id.0, "INBOX", SeqRange::All)
            .unwrap();
        assert_eq!(envs.len(), 1);
        assert_eq!(envs[0].subject, "Hello");
        assert_eq!(envs[0].from[0].email, "alice@example.com");
        assert!(envs[0].message_id.contains("a@example.com"));
    }

    #[test]
    fn fetch_message_by_id_finds_across_folders() {
        let (_dir, backend, account) = fixture();
        let msg = backend
            .fetch_message(&account.id.0, "<b@example.com>")
            .unwrap();
        assert_eq!(msg.envelope.subject, "Re: Hello");
        assert_eq!(msg.envelope.folder, "Sent");
        assert!(msg.references.iter().any(|r| r.contains("a@example.com")));
    }

    #[test]
    fn folder_aliases_translate_in_both_directions() {
        // Backend folder name is `Gmail-Sent` (simulating a
        // server-side weird name). The user aliases it to
        // `Sent`, expects `Sent` everywhere in the UI.
        let dir = tempfile::tempdir().unwrap();
        let account = Account {
            id: AccountId("scratch".into()),
            name: "scratch".into(),
            address: "you@example.com".into(),
            display_name: None,
        };
        let mut aliases = FolderAliases::new();
        aliases.insert("Sent", "Gmail-Sent");
        let backend =
            Backend::single_with_aliases(account.clone(), dir.path().to_path_buf(), aliases)
                .unwrap();

        // Build the backend-named folder on disk.
        let weird = dir.path().join(".Gmail-Sent");
        std::fs::create_dir_all(weird.join("cur")).unwrap();
        std::fs::create_dir_all(weird.join("new")).unwrap();
        std::fs::create_dir_all(weird.join("tmp")).unwrap();
        std::fs::write(
            weird.join("cur").join("1700000010.M.host:2,S"),
            "Message-ID: <c@example.com>\r\n\
             From: you@example.com\r\n\
             To: c@example.com\r\n\
             Subject: Aliased send\r\n\
             Date: Mon, 14 Nov 2023 14:00:00 +0000\r\n\
             \r\n\
             body\r\n",
        )
        .unwrap();

        // list_folders reports the alias name, not the backend name.
        let folders = backend.list_folders(&account.id.0).unwrap();
        let ids: Vec<_> = folders.iter().map(|f| f.id.as_str()).collect();
        assert!(ids.contains(&"Sent"), "got: {ids:?}");
        assert!(!ids.contains(&"Gmail-Sent"), "got: {ids:?}");

        // fetch_envelopes accepts the alias name.
        let envs = backend
            .fetch_envelopes(&account.id.0, "Sent", SeqRange::All)
            .unwrap();
        assert_eq!(envs.len(), 1);
        assert_eq!(envs[0].subject, "Aliased send");
        // Envelope echoes the caller's folder name (alias), not
        // the backend name.
        assert_eq!(envs[0].folder, "Sent");

        // fetch_message also finds it via the aliased listing.
        let msg = backend
            .fetch_message(&account.id.0, "<c@example.com>")
            .unwrap();
        assert_eq!(msg.envelope.folder, "Sent");
    }

    /// Recording mock transport — captures the envelope + raw
    /// bytes `send` would have put on the wire.
    struct MockSubmit {
        calls: std::sync::Mutex<Vec<(String, Vec<String>, Vec<u8>)>>,
    }

    impl crate::submit::Submit for MockSubmit {
        fn submit_raw<'a>(
            &'a self,
            from: &'a str,
            recipients: &'a [String],
            raw: &'a [u8],
            message_id: String,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>>
        {
            self.calls
                .lock()
                .unwrap()
                .push((from.to_string(), recipients.to_vec(), raw.to_vec()));
            Box::pin(async move { Ok(message_id) })
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn send_submits_writes_sent_copy_and_publishes() {
        let dir = tempfile::tempdir().unwrap();
        let account = Account {
            id: AccountId("scratch".into()),
            name: "scratch".into(),
            address: "you@example.com".into(),
            display_name: None,
        };
        let mock = Arc::new(MockSubmit {
            calls: std::sync::Mutex::new(Vec::new()),
        });
        let backend = Backend::with_configured_accounts([AccountEntry {
            account: account.clone(),
            root: dir.path().to_path_buf(),
            aliases: FolderAliases::new(),
            submit: Some(mock.clone()),
        }]);

        // Subscribe before sending so the event can't be missed.
        let mut rx = backend.channel("scratch").await.subscribe();

        let draft = Draft {
            from: email_proto::Addr {
                name: None,
                email: "you@example.com".into(),
            },
            to: vec![email_proto::Addr {
                name: Some("Bob".into()),
                email: "bob@example.com".into(),
            }],
            cc: vec![],
            bcc: vec![],
            subject: "Wired send".into(),
            body_text: "hello from the maildir backend".into(),
            body_html: None,
            in_reply_to: None,
            references: vec![],
            attachments: vec![],
        };

        // The sync trait method blocks on the runtime handle, so
        // hop onto the blocking pool like the dispatcher does.
        let b = backend.clone();
        let mid = tokio::task::spawn_blocking(move || b.send("scratch", draft))
            .await
            .unwrap()
            .unwrap();
        assert!(!mid.is_empty());

        // Transport saw the envelope.
        let calls = mock.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "you@example.com");
        assert_eq!(calls[0].1, vec!["bob@example.com".to_string()]);
        drop(calls);

        // Sent copy is on disk and readable through the trait.
        let envs = backend
            .fetch_envelopes("scratch", "Sent", SeqRange::All)
            .unwrap();
        assert_eq!(envs.len(), 1);
        assert_eq!(envs[0].subject, "Wired send");
        assert!(envs[0].flags.iter().any(|f| f == "S"));

        // Publish-after-write: the NewMessage event names the
        // Sent copy.
        let event = rx.try_recv().unwrap();
        match event {
            EmailEvent::NewMessage { folder, message_id } => {
                assert_eq!(folder, "Sent");
                assert_eq!(message_id, mid);
            }
            other => panic!("expected NewMessage, got {other:?}"),
        }
    }

    // ── mutations ─────────────────────────────────────────────
    //
    // These were `Unsupported` stubs; the `/email` page's read/flag/
    // file/delete actions all go through them.

    #[test]
    fn set_flags_marks_seen_and_promotes_out_of_new() {
        let (_dir, backend, acct) = fixture();
        // The fixture's INBOX message lives in `new/` — unread, and
        // with nowhere to record a flag until it moves to `cur/`.
        let before = backend
            .fetch_envelopes(&acct.id.0, "INBOX", SeqRange::Recent(10))
            .unwrap();
        assert!(!before[0].flags.iter().any(|f| f == "S"), "starts unread");

        backend
            .set_flags(
                &acct.id.0,
                "a@example.com",
                FlagDelta {
                    add: vec!["\\Seen".into()],
                    remove: Vec::new(),
                },
            )
            .unwrap();

        let after = backend
            .fetch_envelopes(&acct.id.0, "INBOX", SeqRange::Recent(10))
            .unwrap();
        assert!(
            after[0].flags.iter().any(|f| f == "S"),
            "now seen: {:?}",
            after[0].flags
        );
    }

    #[test]
    fn set_flags_round_trips_flagged() {
        let (_dir, backend, acct) = fixture();
        let id = "a@example.com";
        backend
            .set_flags(
                &acct.id.0,
                id,
                FlagDelta {
                    add: vec!["\\Flagged".into()],
                    remove: Vec::new(),
                },
            )
            .unwrap();
        let seen = |b: &Backend| {
            b.fetch_envelopes(&acct.id.0, "INBOX", SeqRange::Recent(10))
                .unwrap()[0]
                .flags
                .iter()
                .any(|f| f == "F")
        };
        assert!(seen(&backend), "starred");
        backend
            .set_flags(
                &acct.id.0,
                id,
                FlagDelta {
                    add: Vec::new(),
                    remove: vec!["\\Flagged".into()],
                },
            )
            .unwrap();
        assert!(!seen(&backend), "unstarred");
    }

    #[test]
    fn set_flags_ignores_keywords_maildir_cannot_store() {
        // A custom IMAP keyword has no filename letter. Dropping it
        // must not fail the call — the `\Seen` alongside it still
        // applies.
        let (_dir, backend, acct) = fixture();
        backend
            .set_flags(
                &acct.id.0,
                "a@example.com",
                FlagDelta {
                    add: vec!["$Important".into(), "\\Seen".into()],
                    remove: Vec::new(),
                },
            )
            .unwrap();
        let env = backend
            .fetch_envelopes(&acct.id.0, "INBOX", SeqRange::Recent(10))
            .unwrap();
        assert!(env[0].flags.iter().any(|f| f == "S"));
    }

    #[test]
    fn move_message_files_into_a_folder_created_on_demand() {
        let (_dir, backend, acct) = fixture();
        // Archive does not exist in the fixture — the first Archive
        // click has to create it rather than error.
        backend
            .move_message(&acct.id.0, "a@example.com", "Archive")
            .unwrap();

        let inbox = backend
            .fetch_envelopes(&acct.id.0, "INBOX", SeqRange::Recent(10))
            .unwrap();
        assert!(inbox.is_empty(), "left the inbox");
        let archived = backend
            .fetch_envelopes(&acct.id.0, "Archive", SeqRange::Recent(10))
            .unwrap();
        assert_eq!(archived.len(), 1);
        assert_eq!(archived[0].subject, "Hello");
        // And it is still reachable by id from its new home.
        assert!(backend.fetch_message(&acct.id.0, "a@example.com").is_ok());
    }

    #[test]
    fn move_message_to_its_current_folder_is_a_no_op() {
        let (_dir, backend, acct) = fixture();
        backend
            .move_message(&acct.id.0, "a@example.com", "INBOX")
            .unwrap();
        let inbox = backend
            .fetch_envelopes(&acct.id.0, "INBOX", SeqRange::Recent(10))
            .unwrap();
        assert_eq!(inbox.len(), 1, "still there, not duplicated or lost");
    }

    #[test]
    fn delete_message_removes_it_and_is_idempotent() {
        let (_dir, backend, acct) = fixture();
        backend.delete_message(&acct.id.0, "a@example.com").unwrap();
        assert!(
            backend
                .fetch_envelopes(&acct.id.0, "INBOX", SeqRange::Recent(10))
                .unwrap()
                .is_empty()
        );
        // The proto contract says deleting a missing message succeeds.
        backend.delete_message(&acct.id.0, "a@example.com").unwrap();
        backend.delete_message(&acct.id.0, "never@existed").unwrap();
    }

    #[test]
    fn mutations_on_an_unknown_message_report_not_found() {
        let (_dir, backend, acct) = fixture();
        assert!(matches!(
            backend.set_flags(
                &acct.id.0,
                "nope@example.com",
                FlagDelta {
                    add: vec!["\\Seen".into()],
                    remove: Vec::new()
                },
            ),
            Err(EmailSyncError::NotFound)
        ));
        assert!(matches!(
            backend.move_message(&acct.id.0, "nope@example.com", "Archive"),
            Err(EmailSyncError::NotFound)
        ));
    }

    #[test]
    fn flag_letter_accepts_both_spellings() {
        assert_eq!(flag_letter("\\Seen"), Some('S'));
        assert_eq!(flag_letter("Seen"), Some('S'));
        assert_eq!(flag_letter("S"), Some('S'));
        assert_eq!(flag_letter("\\Answered"), Some('R'));
        assert_eq!(flag_letter("$Custom"), None);
    }

    #[test]
    fn send_without_submitter_is_unsupported() {
        let (_dir, backend, account) = fixture();
        let draft = Draft {
            from: email_proto::Addr {
                name: None,
                email: "you@example.com".into(),
            },
            to: vec![email_proto::Addr {
                name: None,
                email: "bob@example.com".into(),
            }],
            cc: vec![],
            bcc: vec![],
            subject: "nope".into(),
            body_text: String::new(),
            body_html: None,
            in_reply_to: None,
            references: vec![],
            attachments: vec![],
        };
        let err = backend.send(&account.id.0, draft).unwrap_err();
        assert!(matches!(err, EmailSyncError::Unsupported(_)), "{err:?}");
    }

    #[test]
    fn folder_counts_track_seen_flag() {
        let (_dir, backend, account) = fixture();
        let folders = backend.list_folders(&account.id.0).unwrap();
        let inbox = folders.iter().find(|f| f.id == "INBOX").unwrap();
        assert_eq!(inbox.message_count, Some(1));
        assert_eq!(inbox.unread_count, Some(1));
        let sent = folders.iter().find(|f| f.id == "Sent").unwrap();
        assert_eq!(sent.message_count, Some(1));
        assert_eq!(sent.unread_count, Some(0));
    }
}
