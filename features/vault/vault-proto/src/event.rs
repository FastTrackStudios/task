//! Live change events streamed to subscribers of the
//! [`crate::VaultSync`] `#[subscribe] fn changes` stream.
//!
//! Every successful PUT or DELETE on the vault publishes one
//! [`VaultChange`] to every connected subscriber. If a subscriber
//! falls behind (mailbox cap exceeded), the hub's sliding strategy
//! drops that subscriber's *oldest* queued events — the client
//! re-pulls the manifest when its stream re-establishes, which is
//! the same recovery [`VaultEvent::Resync`] asks for explicitly.

use facet::Facet;

/// Single change-event on a vault.
#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(u8)]
pub enum VaultEvent {
    /// File was created or modified. Clients can skip the pull
    /// when their local sha already matches (echo from their
    /// own push).
    Put {
        path: String,
        sha256: String,
        mtime_ms: i64,
        size: u64,
    },
    /// File was removed.
    Delete { path: String },
    /// Server hint after a broadcast-lag — re-pull the manifest
    /// to catch missed events. Sent in lieu of replay; the
    /// connection itself stays open.
    Resync,
}

/// One vault change, broadcast to every subscriber of the
/// [`crate::VaultSync`] `changes` stream.
///
/// ## Why the wrapper
///
/// `#[subscribe]` streams take no filter params, so the *scope* of
/// the change has to travel with it: one process can serve several
/// vault ids (`Layout::UnderParent`), and every subscriber sees all
/// of them. Clients keep the id they browse and drop everything
/// else — server-side filtering by `vault_id` (the shape the old
/// `subscribe(vault_id, tx)` RPC had) is now a client-side `==`.
///
/// ## Subscriber contract (changes only, no snapshot variant)
///
/// The stream carries *changes only*. A subscriber that wants vault
/// state fetches it once — [`crate::VaultSync::manifest`] or
/// [`crate::VaultSync::folder_index`], after subscribing so nothing
/// is missed in between — then folds:
///
/// - [`VaultEvent::Put`] — the file at `path` now has that
///   `sha256` / `size` / `mtime_ms`. A client whose local sha
///   already matches is seeing the echo of its own write and can
///   skip the pull. Re-applying is harmless.
/// - [`VaultEvent::Delete`] — drop `path`.
/// - [`VaultEvent::Resync`] — re-pull; state was skipped.
///
/// Derived views the client can't recompute from a `Put` alone (the
/// frontmatter-parsed [`crate::FolderIndex`], say) re-fetch on the
/// event rather than folding it — the event is the *trigger*, the
/// rpc is still the source of truth.
#[derive(Debug, Clone, PartialEq, Eq, Facet)]
pub struct VaultChange {
    /// Which vault changed — subscribers filter on this.
    pub vault_id: String,
    /// What happened.
    pub event: VaultEvent,
}

#[cfg(feature = "vox")]
#[allow(unsafe_code)]
mod reborrow_impls {
    use super::{VaultChange, VaultEvent};
    unsafe impl vox_types::Reborrow for VaultEvent {
        type Ref<'a> = VaultEvent;
    }
    unsafe impl vox_types::Reborrow for VaultChange {
        type Ref<'a> = VaultChange;
    }
}
