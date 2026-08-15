//! The `Notify` RPC surface + its `#[subscribe]` stream.

use uuid::Uuid;

use crate::error::NotifyError;
use crate::model::{Notification, NotifyListFilter};

/// One notification change, broadcast to every [`Notify`] subscriber
/// on each successful mutation (a notifier push, a mark-read, a
/// delete).
///
/// ## Subscriber contract (changes only, v1)
///
/// No `Snapshot` variant. Fetch the list once via [`Notify::list`]
/// (after subscribing, so nothing is missed in between), then fold:
///
/// - [`NotifyEvent::Upserted`] carries the **full post-write**
///   [`Notification`] — replace (or insert) the row with a matching
///   `id`. Idempotent re-application is harmless. New notifications,
///   `mark_read`, and each row of `mark_all_read` all surface here.
/// - [`NotifyEvent::Deleted`] — remove the row with that `id`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, facet::Facet)]
#[repr(u8)]
// Upserted carries the full Notification by design (idempotent
// full-state payloads) — same trade-off as `task_proto::TaskEvent`.
#[allow(clippy::large_enum_variant)]
pub enum NotifyEvent {
    /// A notification appeared or changed — the payload is the
    /// complete state after the write.
    Upserted(Notification),
    /// The notification with this id was permanently removed.
    Deleted(Uuid),
}

#[architect::rpc]
pub trait Notify {
    /// Notifications, newest first, windowed by
    /// [`NotifyListFilter::limit`] / `offset` (`unread_only` drops
    /// read rows first). The bell fetches one default page.
    async fn list(&self, filter: NotifyListFilter) -> Result<Vec<Notification>, NotifyError>;

    /// Mark one row read (idempotent — an already-read row keeps its
    /// original `read_at`). Returns the post-write row.
    async fn mark_read(&self, id: Uuid) -> Result<Notification, NotifyError>;

    /// Mark every unread row read. Returns how many flipped; each
    /// flipped row is also published on [`Notify::events`].
    async fn mark_all_read(&self) -> Result<u64, NotifyError>;

    /// Permanently delete a row. Idempotent — deleting an
    /// already-gone id is `Ok(())`.
    async fn delete(&self, id: Uuid) -> Result<(), NotifyError>;

    /// Every notification change, as it happens. See [`NotifyEvent`]
    /// for the fetch-once-then-fold subscriber contract.
    #[subscribe]
    fn events(&self) -> NotifyEvent;
}
