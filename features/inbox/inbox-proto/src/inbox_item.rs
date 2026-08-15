//! The inbox capture entity.
//!
//! An [`InboxItem`] is one captured thought, highlight, or note that
//! has landed in the single trusted inbox and not yet been processed.
//! It is the atom of the FLAP **capture** step: get the idea out of
//! your head with near-zero friction, then read it again tomorrow (the
//! "temporal contract") and either process it into something durable —
//! a task or an atomic wiki note — snooze it, or let it go.
//!
//! Source of truth is a markdown file in the vault (`Records/inbox/<id>.md`):
//! the structured fields ride in frontmatter, `body` is the verbatim
//! capture. Times are RFC-3339 strings and the enums are stored as their
//! lowercase names so the proto stays serializer-agnostic and
//! `Facet`-encodable — same approach as `scheduling-proto::CalEvent`.

use facet::Facet;
use serde::{Deserialize, Serialize};

/// One captured item in the inbox.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
#[architect(table_name = "inbox_items", repo)]
pub struct InboxItem {
    /// Stable id (uuid string). PK — the vault file is
    /// `Records/inbox/<id>.md`.
    #[architect(primary_key, auto_increment = false)]
    pub id: String,
    /// The captured text, preserved **verbatim** — the paper trail
    /// back to the original thought even after it's been reworded
    /// into a task or atomic note. Markdown.
    #[architect(filterable, fulltext)]
    pub body: String,
    /// Note type — one of `"fleeting"` (your own passing thought,
    /// the default), `"literature"` (a highlight from something you
    /// read/watched), or `"lecture"` (notes taken live). Drives how
    /// the item is typically processed.
    #[architect(filterable)]
    pub kind: String,
    /// Triage state — `"open"` (unprocessed, sitting in the inbox;
    /// the default), `"processed"` (turned into a task / note and
    /// retired), or `"archived"` (consciously let go). The daily
    /// review reads `"open"` items oldest-first.
    #[architect(filterable)]
    pub status: String,
    /// Where the capture came from — `"cli"`, `"ui"`, `"telegram"`,
    /// `"readwise"`, … . Free-form so new capture surfaces don't
    /// need a proto change.
    #[architect(filterable)]
    pub source: String,
    /// RFC-3339 capture timestamp. Sortable so the daily review can
    /// surface the oldest unprocessed items first.
    #[architect(filterable, sortable)]
    pub created: String,
    /// Optional snooze: an ISO `YYYY-MM-DD` date before which the
    /// item is hidden from the default daily queue, then resurfaces.
    /// `None` = always visible while `open`.
    #[architect(filterable, sortable)]
    pub resurface_on: Option<String>,
    /// Provenance: the id of the task or wiki note this item was
    /// processed into, if any. Closes the temporal-contract loop —
    /// you can always walk from the durable artifact back to the
    /// raw capture.
    pub processed_into: Option<String>,
    /// SM-2 ease factor ×100 (obsidian-spaced-repetition `sr-ease`).
    /// `250` = 2.5, the plugin's `BASE_EASE`. Clamped to `MIN_EASE`
    /// (130) on repeated "hard" responses. Frontmatter key `sr-ease`.
    #[serde(default = "default_ease")]
    #[architect(filterable, sortable)]
    pub ease: i64,
    /// Current review interval in whole days (`sr-interval`). `0` =
    /// never reviewed (a brand-new capture); the first review treats
    /// it as `1`. Frontmatter key `sr-interval`.
    #[serde(default)]
    #[architect(filterable, sortable)]
    pub interval: i64,
    /// How many times this item has been reviewed (`sr-reviews`).
    /// `0` = new. Frontmatter key `sr-reviews`.
    #[serde(default)]
    #[architect(filterable, sortable)]
    pub reviews: i64,
}

/// serde default for [`InboxItem::ease`]: the plugin's `BASE_EASE`.
fn default_ease() -> i64 {
    crate::schedule::BASE_EASE
}

impl InboxItem {
    /// Status string for an unprocessed item in the review queue.
    pub const STATUS_OPEN: &'static str = "open";
    /// Status string for an item turned into a durable artifact.
    pub const STATUS_PROCESSED: &'static str = "processed";
    /// Status string for an item consciously let go.
    pub const STATUS_ARCHIVED: &'static str = "archived";
    /// Agent-proposed capture awaiting a one-tap accept/dismiss. Kept
    /// out of the open review queue until the user accepts it (→ open).
    /// Used by ingestion producers (email, …) so suggestions don't
    /// flood the trusted queue unreviewed.
    pub const STATUS_SUGGESTED: &'static str = "suggested";

    /// Default note kind — your own passing thought.
    pub const KIND_FLEETING: &'static str = "fleeting";

    /// A freshly captured fleeting note: `open`, `fleeting`, stamped
    /// now. `id` is a uuid string; `created` is an RFC-3339 string —
    /// both minted by the caller so the proto stays clock-agnostic.
    #[must_use]
    pub fn capture(
        id: impl Into<String>,
        body: impl Into<String>,
        source: impl Into<String>,
        created: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            body: body.into(),
            kind: Self::KIND_FLEETING.to_string(),
            status: Self::STATUS_OPEN.to_string(),
            source: source.into(),
            created: created.into(),
            resurface_on: None,
            processed_into: None,
            ease: 0,
            interval: 0,
            reviews: 0,
        }
    }

    /// True if the item is still awaiting processing.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.status == Self::STATUS_OPEN
    }

    /// Whether this item belongs in the daily review queue on `today`
    /// (ISO `YYYY-MM-DD`): open, and either never snoozed or past its
    /// `resurface_on` date. The one definition of "what surfaces
    /// today", shared by the service query and any caller — so the CLI
    /// brief and the web inbox can't drift on the snooze rule.
    #[must_use]
    pub fn in_review_queue(&self, today: &str) -> bool {
        let day = |s: &str| s.get(..10).unwrap_or(s).to_owned();
        self.is_open()
            && self
                .resurface_on
                .as_deref()
                .is_none_or(|d| day(d) <= day(today))
    }
}

/// Client-side optimistic cache identity (`architect::Store`): keyed by
/// the stable `id`.
#[cfg(feature = "atom")]
impl architect::StoreEntity for InboxItem {
    type Key = String;
    fn key(&self) -> String {
        self.id.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::InboxItem;

    fn item(status: &str, resurface: Option<&str>) -> InboxItem {
        let mut i = InboxItem::capture("id", "body", "cli", "2026-06-01T00:00:00Z");
        i.status = status.into();
        i.resurface_on = resurface.map(str::to_owned);
        i
    }

    #[test]
    fn review_queue_rule() {
        // open + never snoozed → in queue
        assert!(item("open", None).in_review_queue("2026-06-14"));
        // open + snoozed to the past → resurfaced
        assert!(item("open", Some("2026-06-10")).in_review_queue("2026-06-14"));
        // open + snoozed to the future → hidden
        assert!(!item("open", Some("2026-06-20")).in_review_queue("2026-06-14"));
        // not open → never in queue, even if resurfaced
        assert!(!item("processed", None).in_review_queue("2026-06-14"));
        assert!(!item("archived", Some("2026-06-01")).in_review_queue("2026-06-14"));
    }
}
