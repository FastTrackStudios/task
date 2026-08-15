//! Folder (mailbox) listing entry. One [`Folder`] per IMAP
//! mailbox / JMAP mailbox / Maildir subdir.

use facet::Facet;

#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(u8)]
pub enum FolderRole {
    Inbox,
    Drafts,
    Sent,
    Trash,
    Junk,
    Archive,
    Outbox,
    All,
    Flagged,
}

/// One mailbox. `id` is the path the backend uses to address
/// this folder (IMAP mailbox name, JMAP id, Maildir relative
/// path). `name` is the user-visible label — may include
/// hierarchy joined by `delimiter`.
#[derive(Debug, Clone, Facet)]
pub struct Folder {
    pub id: String,
    pub name: String,
    pub delimiter: String,
    pub role: Option<FolderRole>,
    pub message_count: Option<u32>,
    pub unread_count: Option<u32>,
}

#[cfg(feature = "vox")]
#[allow(unsafe_code)]
mod reborrow_impls {
    use super::{Folder, FolderRole};
    unsafe impl vox_types::Reborrow for Folder {
        type Ref<'a> = Folder;
    }
    unsafe impl vox_types::Reborrow for FolderRole {
        type Ref<'a> = FolderRole;
    }
}
