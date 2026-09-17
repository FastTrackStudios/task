//! `EmailLinks` — messages as linkable objects.
//!
//! A message can be attached to anything else in Task: a task, a
//! project, a note, a person. Many-to-many in both directions, so
//! "every email on this project" and "everything this email is about"
//! are both single lookups.
//!
//! The link is keyed on the **Message-ID**, not on a mailbox position.
//! That is what makes it survive the things that happen to mail: the
//! message can be archived, moved between folders, or re-synced into a
//! rebuilt index and the link still resolves. A link to `(folder, uid)`
//! would break the first time you filed something.
//!
//! Storage is `email-link`'s sqlite table, one per org.

use crate::EmailSyncError;
use facet::Facet;

/// What a message is linked to. `kind` is a free-form string rather
/// than an enum so a new entity type doesn't need a proto revision —
/// `"task"`, `"project"`, `"note"`, `"person"` today.
#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
pub struct LinkTarget {
    pub kind: String,
    /// The entity's id in its own service's terms — a task UUID, a
    /// project id, a vault path.
    pub id: String,
    /// Which organisation the entity belongs to. Empty means the org
    /// that holds the link store.
    ///
    /// Mail belongs to a person; projects belong to organisations.
    /// The two live in different vaults, so without this a link could
    /// only ever reach inside the org holding the store — forcing a
    /// choice between copying one mailbox into every org or not
    /// filing mail against shared work at all.
    ///
    /// Naming the org instead lets the links stay in the owner's own
    /// org, which is what keeps them private: other members of the
    /// org a project lives in have no access to the store the links
    /// are in, so "every email on this project" answers for you and
    /// is empty for them, with no per-person access rules to enforce.
    pub org: String,
}

/// One message↔entity link.
#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(C)]
pub struct MessageLink {
    /// RFC 2822 Message-ID, stored bare (no angle brackets).
    pub message_id: String,
    pub target: LinkTarget,
    /// Unix millis. `0` for rows predating the column.
    pub linked_at_ms: i64,
    /// Who made the link: `"user"`, `"rule"`, or an agent name.
    /// Distinguishing them is what lets you audit — or undo — a bulk
    /// auto-link without touching the ones you made yourself.
    pub linked_by: String,
    /// Free-form tags layered on top of the mail client's own.
    pub user_tags: Vec<String>,
}

#[architect::rpc]
pub trait EmailLinks {
    /// Attach `message_id` to `target`. Idempotent — re-linking the
    /// same pair updates the row rather than duplicating it, so a
    /// rule that runs twice is harmless.
    fn link(
        &self,
        message_id: &str,
        target: LinkTarget,
        linked_by: &str,
    ) -> Result<MessageLink, EmailSyncError>;

    /// Remove one link. Removing a link that isn't there succeeds.
    fn unlink(&self, message_id: &str, target: LinkTarget) -> Result<(), EmailSyncError>;

    /// Everything this message is linked to.
    fn links_for_message(&self, message_id: &str) -> Result<Vec<MessageLink>, EmailSyncError>;

    /// Every message linked to this entity — "all the mail on this
    /// project".
    fn links_for_target(&self, target: LinkTarget) -> Result<Vec<MessageLink>, EmailSyncError>;
}

#[cfg(feature = "vox")]
#[allow(unsafe_code)]
mod reborrow_impls {
    use super::{LinkTarget, MessageLink};
    unsafe impl vox_types::Reborrow for LinkTarget {
        type Ref<'a> = LinkTarget;
    }
    unsafe impl vox_types::Reborrow for MessageLink {
        type Ref<'a> = MessageLink;
    }
}
