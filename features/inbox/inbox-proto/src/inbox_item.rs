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
    // t[impl process.capture.verbatim]
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
    // t[impl attribution.created-by]
    /// Who captured it: the authenticated principal behind the write,
    /// as a ref — `"user:<uuid>"` for a person, `"bot:<uuid>"` for a
    /// bot account (Atlas, Commander, …). `source` names the *surface*
    /// (cli / ui / mcp / watch); this names the *author*.
    ///
    /// Stamped by the server from the caller's credentials, never from
    /// the item a client sends, and write-once: the backend keeps the
    /// value already on disk on every later upsert, so an edit (or a
    /// stale copy) can't rewrite it. `None` on items captured before
    /// attribution existed, and on writes no principal stands behind
    /// (the server itself, a file edited in place).
    #[facet(default)]
    #[serde(default)]
    #[architect(filterable)]
    pub created_by: Option<String>,
    /// The creator's display name when the item was captured ("Atlas",
    /// "Cody") — what a surface shows without a directory lookup, the
    /// same snapshot `AgentRef::Human::display_name` keeps. Stamped and
    /// kept with [`Self::created_by`].
    #[facet(default)]
    #[serde(default)]
    pub created_by_name: Option<String>,
    // t[impl attribution.on-behalf-of]
    /// Whom the creator acted for, when it is a **personal** bot — an
    /// assistant like Atlas that works for one person: that person's
    /// ref (`"user:<uuid>"`), so the item reads "from Atlas (for
    /// Cody)". `None` for a person's own capture, for a team bot's
    /// (one with no personal owner), and on items from before.
    ///
    /// Stamped by the server from the bot key's owner, never from the
    /// item a client sends, and write-once with [`Self::created_by`].
    #[facet(default)]
    #[serde(default)]
    #[architect(filterable)]
    pub on_behalf_of: Option<String>,
    /// That person's display name at capture, kept with
    /// [`Self::on_behalf_of`].
    #[facet(default)]
    #[serde(default)]
    pub on_behalf_of_name: Option<String>,
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
            created_by: None,
            created_by_name: None,
            on_behalf_of: None,
            on_behalf_of_name: None,
            created: created.into(),
            resurface_on: None,
            processed_into: None,
            ease: 0,
            interval: 0,
            reviews: 0,
        }
    }

    /// Whether this item was created by `who`: a full ref
    /// (`"bot:<uuid>"`), a bare account uuid, or `"none"` for items
    /// with no recorded creator. See
    /// [`workflows_proto::creator::creator_matches`].
    #[must_use]
    pub fn created_by_matches(&self, who: &str) -> bool {
        workflows_proto::creator::creator_matches(self.created_by.as_deref(), who)
    }

    /// Whether this item was captured on behalf of `who` — the same
    /// rule as [`Self::created_by_matches`], over
    /// [`Self::on_behalf_of`].
    #[must_use]
    pub fn on_behalf_of_matches(&self, who: &str) -> bool {
        workflows_proto::creator::creator_matches(self.on_behalf_of.as_deref(), who)
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

    /// Whether this item is due for processing on `today`: in the
    /// review queue ([`Self::in_review_queue`]), or a suggestion
    /// awaiting a yes or no. What the Inbox list shows and what the
    /// Suggested slot (web and watch) counts as "captures to process".
    #[must_use]
    pub fn is_due(&self, today: &str) -> bool {
        self.status == Self::STATUS_SUGGESTED || self.in_review_queue(today)
    }
}

/// What is due for processing on `today`: every [`InboxItem::is_due`]
/// item, oldest capture first. The one queue behind the inbox deck, the
/// tab badge, Home's "captures to process", the watch's Suggested count
/// and the brief — so a count and the list under it can't disagree.
/// [`crate::Inbox::due_queue`] serves it; clients with a store call it
/// on theirs.
// t[impl process.due]
#[must_use]
pub fn due_queue<'a>(
    items: impl IntoIterator<Item = &'a InboxItem>,
    today: &str,
) -> Vec<InboxItem> {
    let mut out: Vec<InboxItem> = items
        .into_iter()
        .filter(|i| i.is_due(today))
        .cloned()
        .collect();
    out.sort_by(|a, b| a.created.cmp(&b.created).then_with(|| a.id.cmp(&b.id)));
    out
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
    fn creator_filter_rule() {
        let mut i = item("open", None);
        assert!(i.created_by_matches("none"));
        assert!(!i.created_by_matches("bot:b1"));
        i.created_by = Some("bot:b1".into());
        assert!(i.created_by_matches("bot:b1"));
        assert!(i.created_by_matches("b1"), "a bare uuid matches its ref");
        assert!(
            !i.created_by_matches("user:b1"),
            "the kind is part of a full ref"
        );
        assert!(!i.created_by_matches("none"));
        assert!(i.created_by_matches(""), "no filter keeps everything");
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
