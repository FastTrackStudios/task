//! Inbox CRUD — persistence for captured items.
//!
//! Deliberately a flat CRUD surface: capture, daily-read, snooze,
//! mark-processed, and archive are all just an `upsert_inbox_item`
//! with a mutated item (same way `scheduling-proto`'s
//! `upsert_event` covers both create and update). The caller owns
//! the state transitions; the backend just round-trips markdown.

use crate::error::InboxError;
use crate::inbox_item::InboxItem;

/// One inbox change, broadcast to every [`Inbox`] subscriber on each
/// successful mutation.
///
/// ## Subscriber contract (no snapshot variant, v1)
///
/// The stream carries *changes only* — there is no `Snapshot`
/// variant. A subscriber that wants the full queue fetches it once
/// via [`Inbox::list_inbox`] (after subscribing, so nothing is
/// missed in between) and then folds events into that local copy:
///
/// - [`InboxEvent::Upserted`] carries the **full post-write**
///   [`InboxItem`] — replace (or insert) the row with a matching
///   `id`. Re-applying an event already reflected in the fetched
///   list is harmless (idempotent re-application). Capture, snooze,
///   process, and archive all surface as upserts (they are all
///   `upsert_inbox_item` on the wire).
/// - [`InboxEvent::Deleted`] — remove the row with that `id`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, facet::Facet)]
#[repr(u8)]
// Upserted carries the full InboxItem by design (idempotent
// full-state payloads) — same trade-off as `task_proto::TaskEvent`.
#[allow(clippy::large_enum_variant)]
pub enum InboxEvent {
    /// An item was captured or modified — the payload is the
    /// complete state after the write.
    Upserted(InboxItem),
    /// The item with this id (and its backing file) was removed.
    Deleted(String),
}

#[architect::rpc]
pub trait Inbox {
    /// Every item in the inbox, processed or not.
    fn list_inbox(&self) -> Result<Vec<InboxItem>, InboxError>;

    /// The daily-review queue for `today` (ISO `YYYY-MM-DD`): open
    /// items not snoozed past today, oldest capture first. The
    /// "what surfaces today" rule (see [`InboxItem::in_review_queue`])
    /// lives here so every surface — CLI brief, web inbox — gets the
    /// same queue without re-implementing the snooze filter.
    fn review_queue(&self, today: String) -> Result<Vec<InboxItem>, InboxError>;
    /// One item by id.
    fn get_inbox_item(&self, id: &str) -> Result<InboxItem, InboxError>;
    /// Create or replace an item (keyed by `id`). Capture, snooze,
    /// process, and archive all flow through here.
    fn upsert_inbox_item(&self, item: &InboxItem) -> Result<(), InboxError>;
    /// Permanently remove an item from the vault.
    fn delete_inbox_item(&self, id: &str) -> Result<(), InboxError>;

    /// Every inbox change, as it happens — fires on each successful
    /// upsert / delete. See [`InboxEvent`] for the
    /// fetch-once-then-fold subscriber contract.
    #[subscribe]
    fn events(&self) -> InboxEvent;
}
