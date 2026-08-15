//! `EmailSync` impl backed by a JMAP server via `jmap-client`.
//!
//! Phase-1 scope (this crate): connect + auth + folder listing.
//! `Email/query` + `Email/get` shapes for the envelope/message
//! paths are the obvious next step but the `jmap-client` request
//! builder has a non-trivial type-level dance around result
//! references — that lands in a focused follow-up rather than
//! getting shoehorned into the first cut.

use std::collections::HashMap;
use std::sync::Arc;

use email_config::{BackendKind, FolderAliases};
use email_proto::{
    Account, Draft, EmailEvent, EmailSync, EmailSyncError, Envelope, FlagDelta, Folder, FolderRole,
    Message, SeqRange,
};
use jmap_client::client::{Client, Credentials};
use jmap_client::mailbox::Role as JmapRole;
use tokio::sync::{RwLock, broadcast};

struct AccountState {
    account: Account,
    session_url: String,
    credentials: email_secret::Secret,
    aliases: FolderAliases,
}

/// JMAP backend. Cheap to `Clone` — all internals are `Arc`'d.
#[derive(Clone, architect::HasDispatcher)]
pub struct Backend {
    accounts: Arc<HashMap<String, AccountState>>,
    channels: Arc<RwLock<HashMap<String, broadcast::Sender<EmailEvent>>>>,
    /// Fan-out hub behind the `#[subscribe] fn changes` stream.
    /// Every event that goes onto a per-account broadcast channel
    /// is published here too, wrapped with its `account` so
    /// subscribers — who see every account this backend serves —
    /// can filter. Sliding mailbox: a slow subscriber loses its
    /// oldest queued events and re-pulls on reconnect, which is
    /// what `EmailEvent::Resync` asks for anyway.
    changes: architect::PubSub<email_proto::EmailChange>,
    runtime: tokio::runtime::Handle,
}

impl Backend {
    /// Build a backend from one or more
    /// [`email_config::AccountConfig`] entries. Skips configs
    /// whose `BackendKind` isn't `Jmap`. Must be called from
    /// inside a tokio runtime.
    pub fn from_configs<I>(configs: I) -> Result<Self, &'static str>
    where
        I: IntoIterator<Item = email_config::AccountConfig>,
    {
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|_| "Backend::from_configs must be called from a tokio runtime")?;
        let mut accounts = HashMap::new();
        for cfg in configs {
            let BackendKind::Jmap {
                session_url,
                credentials,
            } = cfg.backend.clone()
            else {
                continue;
            };
            let account = cfg.to_account();
            accounts.insert(
                account.id.0.clone(),
                AccountState {
                    account,
                    session_url,
                    credentials,
                    aliases: cfg.folder_aliases.clone(),
                },
            );
        }
        Ok(Self {
            accounts: Arc::new(accounts),
            channels: Arc::new(RwLock::new(HashMap::new())),
            changes: architect::PubSub::sliding(256),
            runtime,
        })
    }

    fn state(&self, account: &str) -> Result<&AccountState, EmailSyncError> {
        self.accounts
            .get(account)
            .ok_or(EmailSyncError::UnknownAccount)
    }

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

    async fn open(&self, state: &AccountState) -> Result<Client, EmailSyncError> {
        let secret = state
            .credentials
            .resolve()
            .await
            .map_err(|_| EmailSyncError::Auth)?;
        Client::new()
            .credentials(Credentials::bearer(secret.as_str()))
            .connect(&state.session_url)
            .await
            .map_err(|e| EmailSyncError::Network(format!("jmap connect: {e}")))
    }

    async fn run_list_folders(&self, state: &AccountState) -> Result<Vec<Folder>, EmailSyncError> {
        let client = self.open(state).await?;
        // `mailbox_query(None, None)` returns every mailbox id.
        // `mailbox_get` then fetches the full row for each.
        // O(folders) round-trips — fine for v1; the next pass
        // batches both into one Request via result references.
        let mut query = client
            .mailbox_query(None::<jmap_client::mailbox::query::Filter>, None::<Vec<_>>)
            .await
            .map_err(|e| EmailSyncError::Protocol(format!("Mailbox/query: {e}")))?;
        let ids = query.take_ids();

        let mut out = Vec::with_capacity(ids.len());
        for id in &ids {
            let mb = match client.mailbox_get(id, None::<Vec<_>>).await {
                Ok(Some(m)) => m,
                Ok(None) => continue,
                Err(err) => {
                    tracing::warn!(%id, %err, "Mailbox/get failed, skipping");
                    continue;
                }
            };
            let backend_name = mb.name().unwrap_or("").to_string();
            let role = map_role(&mb.role());
            let ui_name = state
                .aliases
                .alias_for(&backend_name)
                .map_or_else(|| backend_name.clone(), str::to_string);
            out.push(Folder {
                id: ui_name.clone(),
                name: ui_name,
                delimiter: "/".into(),
                role,
                message_count: Some(mb.total_emails() as u32),
                unread_count: Some(mb.unread_emails() as u32),
            });
        }
        Ok(out)
    }
}

fn map_role(role: &JmapRole) -> Option<FolderRole> {
    use JmapRole as R;
    match role {
        R::Inbox => Some(FolderRole::Inbox),
        R::Drafts => Some(FolderRole::Drafts),
        R::Sent => Some(FolderRole::Sent),
        R::Trash => Some(FolderRole::Trash),
        R::Junk => Some(FolderRole::Junk),
        R::Archive => Some(FolderRole::Archive),
        _ => None,
    }
}

impl EmailSync for Backend {
    fn accounts(&self) -> Result<Vec<Account>, EmailSyncError> {
        Ok(self.accounts.values().map(|s| s.account.clone()).collect())
    }

    fn list_folders(&self, account: &str) -> Result<Vec<Folder>, EmailSyncError> {
        let backend = self.clone();
        let account = account.to_string();
        self.runtime.block_on(async move {
            let state = backend.state(&account)?;
            backend.run_list_folders(state).await
        })
    }

    fn fetch_envelopes(
        &self,
        _account: &str,
        _folder: &str,
        _range: SeqRange,
    ) -> Result<Vec<Envelope>, EmailSyncError> {
        Err(EmailSyncError::Unsupported(
            "jmap: fetch_envelopes lands next (Email/query + Email/get with result-ref)".into(),
        ))
    }

    fn fetch_message(&self, _account: &str, _message_id: &str) -> Result<Message, EmailSyncError> {
        Err(EmailSyncError::Unsupported(
            "jmap: fetch_message lands next (Email/get with bodyValues)".into(),
        ))
    }

    fn fetch_attachment(
        &self,
        _account: &str,
        _message_id: &str,
        _part: &str,
    ) -> Result<Vec<u8>, EmailSyncError> {
        Err(EmailSyncError::Unsupported(
            "jmap: fetch_attachment lands next (Blob/download)".into(),
        ))
    }

    fn set_flags(
        &self,
        _account: &str,
        _message_id: &str,
        _delta: FlagDelta,
    ) -> Result<(), EmailSyncError> {
        Err(EmailSyncError::Unsupported(
            "jmap: set_flags lands in phase 3 (Email/set keywords/*)".into(),
        ))
    }

    fn move_message(
        &self,
        _account: &str,
        _message_id: &str,
        _dest_folder: &str,
    ) -> Result<(), EmailSyncError> {
        Err(EmailSyncError::Unsupported(
            "jmap: move_message lands in phase 3 (Email/set mailboxIds/*)".into(),
        ))
    }

    fn delete_message(&self, _account: &str, _message_id: &str) -> Result<(), EmailSyncError> {
        Err(EmailSyncError::Unsupported(
            "jmap: delete_message lands in phase 3 (Email/set + role:trash)".into(),
        ))
    }

    fn append_draft(&self, _account: &str, _draft: Draft) -> Result<String, EmailSyncError> {
        Err(EmailSyncError::Unsupported(
            "jmap: append_draft lands in phase 3 (Email/import)".into(),
        ))
    }

    fn send(&self, _account: &str, _draft: Draft) -> Result<String, EmailSyncError> {
        Err(EmailSyncError::Unsupported(
            "jmap: send lands in phase 3 (EmailSubmission/set)".into(),
        ))
    }
}

/// The `#[subscribe]` backend contract: the hub the stream host
/// attaches subscriber sinks to. Nothing publishes yet — JMAP push
/// (EventSource / WebSocket `StateChange`) is still unwired, so the
/// stream is correct and silent until that lands.
impl email_proto::EmailSyncStreamSource for Backend {
    fn changes_hub(&self) -> &architect::PubSub<email_proto::EmailChange> {
        &self.changes
    }
}
