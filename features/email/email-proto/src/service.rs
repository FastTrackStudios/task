//! `EmailSync` — the canonical email-sync trait, decorated with
//! `#[architect::rpc]`.
//!
//! The macro derives the async vox face from this sync trait:
//! backends impl `EmailSync` directly (zero-cost in-process
//! call sites), and remote callers reach the same surface via
//! the auto-emitted [`EmailSyncClient`] over vox.
//!
//! Backends carry whatever state they need (an IMAP connection
//! pool, a JMAP session, a Maildir root + watcher, etc.) and
//! additionally implement [`architect::HasDispatcher`] so the
//! bridge knows how to marshal sync method calls onto the
//! right thread — `TokioBlockingDispatcher` for backends that
//! block on network IO, `CurrentThread` for tests +
//! in-process callers.

use crate::{
    Account, Draft, EmailChange, EmailSyncError, Envelope, FlagDelta, Folder, Message, SeqRange,
};

#[architect::rpc]
pub trait EmailSync {
    /// Accounts this backend serves. Empty list means the
    /// backend is operational but unconfigured.
    fn accounts(&self) -> Result<Vec<Account>, EmailSyncError>;

    /// List every folder on `account`. Empty mailbox = empty
    /// list, not an error.
    fn list_folders(&self, account: &str) -> Result<Vec<Folder>, EmailSyncError>;

    /// Header-only summaries for messages in `folder`.
    fn fetch_envelopes(
        &self,
        account: &str,
        folder: &str,
        range: SeqRange,
    ) -> Result<Vec<Envelope>, EmailSyncError>;

    /// Full message body + headers. Attachments are returned
    /// as metadata only — pull bytes via
    /// [`Self::fetch_attachment`].
    fn fetch_message(&self, account: &str, message_id: &str) -> Result<Message, EmailSyncError>;

    /// One attachment's raw bytes. `part` comes from the
    /// matching [`crate::AttachmentMeta::part`].
    fn fetch_attachment(
        &self,
        account: &str,
        message_id: &str,
        part: &str,
    ) -> Result<Vec<u8>, EmailSyncError>;

    /// Add/remove flags or keyword labels on one message.
    fn set_flags(
        &self,
        account: &str,
        message_id: &str,
        delta: FlagDelta,
    ) -> Result<(), EmailSyncError>;

    /// Move a message to another folder on the same account.
    fn move_message(
        &self,
        account: &str,
        message_id: &str,
        dest_folder: &str,
    ) -> Result<(), EmailSyncError>;

    /// Delete a message. Idempotent: deleting a missing
    /// message succeeds.
    fn delete_message(&self, account: &str, message_id: &str) -> Result<(), EmailSyncError>;

    /// Save a draft into the backend's drafts folder.
    /// Returns the freshly-assigned Message-ID.
    fn append_draft(&self, account: &str, draft: Draft) -> Result<String, EmailSyncError>;

    /// Submit a message for delivery. Returns the
    /// Message-ID of the sent copy (typically appended to
    /// `Sent`).
    fn send(&self, account: &str, draft: Draft) -> Result<String, EmailSyncError>;

    /// Every mailbox change, as it happens — new mail, flag
    /// changes, moves, deletions, folder-list changes. Unfiltered
    /// across accounts; each [`EmailChange`] carries its `account`
    /// so a reader keeps the mailbox it's showing. See
    /// [`EmailChange`] for the fetch-once-then-re-read subscriber
    /// contract.
    #[subscribe]
    fn changes(&self) -> EmailChange;
}
