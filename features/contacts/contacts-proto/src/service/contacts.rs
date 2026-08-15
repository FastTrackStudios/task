//! Contacts CRUD + CardDAV sync accounts.
//!
//! A flat surface like `recall_proto`: the directory is
//! `list_contacts`/`upsert_contact`/`delete_contact`; sync accounts get
//! their own list/upsert/delete; and `sync_account` runs a one-way pull
//! (principal discovery → addressbook-query → vCard parse → upsert),
//! returning a [`SyncReport`]. All sync methods; the backend does the
//! `spawn_blocking` for the network work.

use crate::account::{CardDavAccount, SyncReport};
use crate::contact::Contact;
use crate::error::ContactsError;

/// One directory change, broadcast to every [`Contacts`] subscriber
/// on each successful contact mutation.
///
/// ## What the stream carries
///
/// **Contacts only.** CardDAV account edits don't stream — accounts
/// are a settings surface read on demand. A `sync_account` run
/// surfaces as one `Upserted` per contact it actually wrote (adds +
/// updates; ETag-skipped cards publish nothing).
///
/// ## Subscriber contract (no snapshot variant, v1)
///
/// Changes only — no `Snapshot` variant. Fetch the directory once
/// via [`Contacts::list_contacts`] (after subscribing, so nothing is
/// missed in between), then fold:
///
/// - [`ContactsEvent::Upserted`] carries the **full post-write**
///   [`Contact`] — replace (or insert) the row with a matching `id`.
///   Idempotent re-application is harmless.
/// - [`ContactsEvent::Deleted`] — remove the row with that `id`.
#[derive(
    Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, facet::Facet,
)]
#[repr(u8)]
// Upserted carries the full Contact by design (idempotent full-state
// payloads) — same trade-off as `task_proto::TaskEvent`.
#[allow(clippy::large_enum_variant)]
pub enum ContactsEvent {
    /// A contact was authored, edited, or pulled in by sync — the
    /// payload is the complete state after the write.
    Upserted(Contact),
    /// The contact with this id (and its backing file) was removed.
    Deleted(String),
}

#[architect::rpc]
pub trait Contacts {
    /// Every contact in the directory, archived or not.
    fn list_contacts(&self) -> Result<Vec<Contact>, ContactsError>;

    /// One contact by id, or `None` if the file is gone.
    fn get_contact(&self, id: String) -> Result<Option<Contact>, ContactsError>;

    /// Create or replace a contact (keyed by `id`). Author, edit, link,
    /// and archive all flow through here.
    fn upsert_contact(&self, contact: &Contact) -> Result<(), ContactsError>;

    /// Permanently remove a contact from the vault.
    fn delete_contact(&self, id: &str) -> Result<(), ContactsError>;

    /// Every configured CardDAV sync account, with passwords blanked
    /// (see [`CardDavAccount::redacted`]).
    fn list_accounts(&self) -> Result<Vec<CardDavAccount>, ContactsError>;

    /// Create or replace a sync account (keyed by `id`). A blank
    /// `password` on an existing account keeps the stored credential.
    fn upsert_account(&self, account: &CardDavAccount) -> Result<(), ContactsError>;

    /// Remove a sync account. Contacts it imported are left in place.
    fn delete_account(&self, id: &str) -> Result<(), ContactsError>;

    /// Pull the account's addressbook and upsert its contacts (one-way,
    /// server → vault). Matches existing contacts by vCard `UID`, skips
    /// ETag-unchanged cards, and never touches `manual` contacts.
    fn sync_account(&self, id: String) -> Result<SyncReport, ContactsError>;

    /// Every contact change, as it happens — fires on each
    /// successful upsert / delete (including each contact a
    /// `sync_account` pull writes). See [`ContactsEvent`] for the
    /// fetch-once-then-fold subscriber contract.
    #[subscribe]
    fn events(&self) -> ContactsEvent;
}
