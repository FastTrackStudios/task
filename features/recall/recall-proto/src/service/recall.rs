//! Recall CRUD — persistence for the learning deck.
//!
//! A flat CRUD surface like `inbox_proto`: create/author, edit,
//! review-reschedule, and archive all flow through `upsert_card` with
//! a mutated card. The caller owns the FSRS state transitions (via
//! [`crate::RecallCard::review`]); the backend just round-trips
//! markdown.

use crate::error::RecallError;
use crate::recall_card::RecallCard;

/// One deck change, broadcast to every [`Recall`] subscriber on each
/// successful mutation.
///
/// ## Subscriber contract (no snapshot variant, v1)
///
/// The stream carries *changes only* — there is no `Snapshot`
/// variant. A subscriber that wants the full deck fetches it once
/// via [`Recall::list_cards`] (after subscribing, so nothing is
/// missed in between) and then folds events into that local copy:
///
/// - [`RecallEvent::Upserted`] carries the **full post-write**
///   [`RecallCard`] — replace (or insert) the row with a matching
///   `id`. Idempotent re-application is harmless. Author, edit,
///   review-reschedule, and archive all surface as upserts (they are
///   all `upsert_card` on the wire).
/// - [`RecallEvent::Deleted`] — remove the row with that `id`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, facet::Facet)]
#[repr(u8)]
// Upserted carries the full RecallCard by design (idempotent
// full-state payloads) — same trade-off as `task_proto::TaskEvent`.
#[allow(clippy::large_enum_variant)]
pub enum RecallEvent {
    /// A card was authored or modified — the payload is the complete
    /// state after the write.
    Upserted(RecallCard),
    /// The card with this id (and its backing file) was removed.
    Deleted(String),
}

#[architect::rpc]
pub trait Recall {
    /// Every card in the deck, archived or not.
    fn list_cards(&self) -> Result<Vec<RecallCard>, RecallError>;

    /// The review queue for `today` (ISO `YYYY-MM-DD`): due,
    /// non-archived cards. The "what surfaces today" rule (see
    /// [`RecallCard::in_review_queue`]) lives here so every surface
    /// gets the same queue without re-implementing the due filter.
    fn review_queue(&self, today: String) -> Result<Vec<RecallCard>, RecallError>;

    /// Create or replace a card (keyed by `id`). Author, edit,
    /// review-reschedule, and archive all flow through here.
    fn upsert_card(&self, card: &RecallCard) -> Result<(), RecallError>;

    /// Permanently remove a card from the vault.
    fn delete_card(&self, id: &str) -> Result<(), RecallError>;

    /// Every deck change, as it happens — fires on each successful
    /// upsert / delete. See [`RecallEvent`] for the
    /// fetch-once-then-fold subscriber contract.
    #[subscribe]
    fn events(&self) -> RecallEvent;
}
