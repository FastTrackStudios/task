//! Live change events streamed on the [`crate::EmailSync`]
//! `changes` stream. Mirrors the vault-proto `VaultChange` shape:
//! one hub for every account, the scope carried in the payload.

use facet::Facet;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Facet, Serialize, Deserialize)]
#[repr(u8)]
pub enum EmailEvent {
    NewMessage {
        folder: String,
        message_id: String,
    },
    FlagsChanged {
        message_id: String,
        flags: Vec<String>,
    },
    Moved {
        message_id: String,
        from_folder: String,
        to_folder: String,
    },
    Deleted {
        message_id: String,
    },
    FolderListChanged,
    /// An outbox entry changed state (staged / approved / sending
    /// / sent / failed / cancelled). Changes-only: the event
    /// names the entry and its new status; re-read via
    /// `EmailProduct::list_outbox`.
    OutboxChanged {
        id: u64,
        status: crate::OutboxStatus,
    },
    /// The derivation cache gained/updated rows for a message
    /// (urgency / tags / …). Re-read via
    /// `EmailProduct::derivations`.
    DerivationsUpdated {
        message_id: String,
    },
    /// Subscriber missed events because the broadcast lapped.
    /// Re-pull folder listings + envelopes to catch up.
    Resync,
}

/// One mailbox change, broadcast to every subscriber of the
/// [`crate::EmailSync`] `changes` stream.
///
/// ## Why the wrapper
///
/// `#[subscribe]` streams take no filter params, so the scope
/// travels with the event: one backend serves every configured
/// account, and subscribers keep the one they're reading —
/// server-side filtering by `account` (the shape the old
/// `subscribe(account, tx)` rpc had) is now a client-side `==`.
///
/// ## Subscriber contract (changes only, no snapshot variant)
///
/// The stream carries *changes only*. A mail view lists once
/// ([`crate::EmailSync::fetch_envelopes`], after subscribing so
/// nothing is missed in between) and then re-reads what an event
/// touches — the events name *what* changed, not the new value
/// (an [`crate::Envelope`] is a mailbox read, and IMAP's IDLE only
/// tells us "something did"). [`EmailEvent::Resync`] means re-pull
/// everything.
#[derive(Debug, Clone, PartialEq, Facet, Serialize, Deserialize)]
pub struct EmailChange {
    /// Account the change belongs to — subscribers filter on this.
    pub account: String,
    /// What happened.
    pub event: EmailEvent,
}

#[cfg(feature = "vox")]
#[allow(unsafe_code)]
mod reborrow_impls {
    use super::{EmailChange, EmailEvent};
    unsafe impl vox_types::Reborrow for EmailEvent {
        type Ref<'a> = EmailEvent;
    }
    unsafe impl vox_types::Reborrow for EmailChange {
        type Ref<'a> = EmailChange;
    }
}
