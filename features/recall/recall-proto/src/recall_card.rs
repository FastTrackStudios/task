//! The learning-card entity.
//!
//! A [`RecallCard`] is one flashcard in the recall deck: a front/back
//! prompt of some [`CardType`], grouped under a `project` (the deck /
//! filter), plus the FSRS memory state that schedules it.
//!
//! Source of truth is a markdown file in the vault
//! (`Records/recall/<id>.md`): the structured fields ride in
//! frontmatter (the scheduler state under `sr-*` keys), and the body
//! holds the front/back verbatim. Dates are ISO `YYYY-MM-DD` strings so
//! the proto stays serializer-agnostic and `Facet`-encodable — the same
//! approach `inbox_proto::InboxItem` takes.

use chrono::NaiveDate;
use facet::Facet;
use serde::{Deserialize, Serialize};

use spaced_repetition::{FsrsCard, Rating};

/// The default retention FSRS schedules toward (≈90% recall at the due
/// date) — the value Anki ships. A per-deck override can come later.
pub const DEFAULT_RETENTION: f64 = 0.9;

/// The built-in card shapes. Free-form on the wire (`card_type` is a
/// `String`) so a new shape needs no proto change; these are the ones
/// the UI knows how to author + render.
pub struct CardType;

impl CardType {
    /// A question on the front, its answer on the back — the generic
    /// shape and the default for note-generated cards.
    pub const CONCEPT_QA: &'static str = "concept-qa";
    /// A passage with a blank; recall the missing span.
    pub const CLOZE: &'static str = "cloze";
    /// Prompt is a reference (e.g. "John 3:16"); recall the text.
    pub const REFERENCE_TO_VERSE: &'static str = "reference-to-verse";
    /// Prompt is the text; recall its reference.
    pub const VERSE_TO_REFERENCE: &'static str = "verse-to-reference";
    /// Front shows only the first letter of each word; recall the whole.
    pub const FIRST_LETTER: &'static str = "first-letter";
    /// Anything else — a plain author-defined front/back.
    pub const FREE: &'static str = "free";

    /// Every built-in type, for the quick-add type picker.
    #[must_use]
    pub fn all() -> &'static [&'static str] {
        &[
            Self::CONCEPT_QA,
            Self::CLOZE,
            Self::REFERENCE_TO_VERSE,
            Self::VERSE_TO_REFERENCE,
            Self::FIRST_LETTER,
            Self::FREE,
        ]
    }
}

/// One learning card on the FSRS scheduler.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Debug, Clone, PartialEq, Facet, Serialize, Deserialize)]
#[architect(table_name = "recall_cards", repo)]
pub struct RecallCard {
    /// Stable id (uuid string). PK — the vault file is
    /// `Records/recall/<id>.md`.
    #[architect(primary_key, auto_increment = false)]
    pub id: String,
    /// The deck this card belongs to — the project-level filter chips
    /// group by this. Free-form (`"Bible"`, `"Spanish"`, a project
    /// slug, …); empty string = the default / uncategorized deck.
    #[architect(filterable)]
    pub project: String,
    /// One of [`CardType`]'s constants. Drives how the front is
    /// rendered (e.g. first-letter masking) — but stored as a plain
    /// string so new shapes don't need a proto change.
    #[architect(filterable)]
    pub card_type: String,
    /// The prompt shown first.
    #[architect(fulltext)]
    pub front: String,
    /// The answer revealed on flip.
    #[architect(fulltext)]
    pub back: String,
    /// Provenance: the vault path of the note this card was generated
    /// from, if any. Lets a card walk back to its source.
    pub source_note: Option<String>,

    // ── FSRS memory state (frontmatter keys `sr-*`) ─────────────────
    /// Memory stability in days. `0.0` for a never-reviewed card.
    #[architect(filterable, sortable)]
    pub stability: f64,
    /// Difficulty `∈ [1, 10]`. `0.0` until first review.
    #[architect(filterable, sortable)]
    pub difficulty: f64,
    /// Total reviews.
    #[architect(filterable, sortable)]
    pub reps: i64,
    /// `Again` lapses.
    #[architect(filterable, sortable)]
    pub lapses: i64,
    /// Next review date, ISO `YYYY-MM-DD`. `None` = brand new (always
    /// due).
    #[architect(filterable, sortable)]
    pub due: Option<String>,
    /// Date of the last review, ISO `YYYY-MM-DD`. `None` = never
    /// reviewed.
    #[architect(filterable, sortable)]
    pub last_review: Option<String>,

    /// Retired from the deck — hidden from review + the default stream.
    #[architect(filterable)]
    pub archived: bool,
    /// RFC-3339 creation timestamp.
    #[architect(filterable, sortable)]
    pub created: String,
}

impl RecallCard {
    /// A freshly authored card: never reviewed (blank FSRS state),
    /// not archived. `id` / `created` are minted by the caller so the
    /// proto stays clock-agnostic.
    #[must_use]
    pub fn create(
        id: impl Into<String>,
        project: impl Into<String>,
        card_type: impl Into<String>,
        front: impl Into<String>,
        back: impl Into<String>,
        created: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            project: project.into(),
            card_type: card_type.into(),
            front: front.into(),
            back: back.into(),
            source_note: None,
            stability: 0.0,
            difficulty: 0.0,
            reps: 0,
            lapses: 0,
            due: None,
            last_review: None,
            archived: false,
            created: created.into(),
        }
    }

    /// Parse an ISO `YYYY-MM-DD` (tolerant of an RFC-3339 prefix).
    fn parse_day(s: &str) -> Option<NaiveDate> {
        NaiveDate::parse_from_str(s.get(..10).unwrap_or(s), "%Y-%m-%d").ok()
    }

    /// This card's FSRS memory state, reconstructed from the `sr-*`
    /// fields — the scheduler input.
    #[must_use]
    pub fn fsrs(&self) -> FsrsCard {
        FsrsCard {
            stability: self.stability,
            difficulty: self.difficulty,
            reps: self.reps,
            lapses: self.lapses,
            last_review: self.last_review.as_deref().and_then(Self::parse_day),
            due: self.due.as_deref().and_then(Self::parse_day),
        }
    }

    /// Whether this card is due for review on `today` (ISO
    /// `YYYY-MM-DD`) and not archived — the shared "what surfaces in
    /// the review queue" rule, so the service query and the UI can't
    /// drift. A never-scheduled card is always due.
    #[must_use]
    pub fn in_review_queue(&self, today: &str) -> bool {
        if self.archived {
            return false;
        }
        match self.due.as_deref() {
            None => true,
            Some(due) => Self::parse_day(due).zip(Self::parse_day(today)).is_none_or(
                |(due, today)| due <= today,
            ),
        }
    }

    /// Fold an FSRS result back into this card's `sr-*` fields. The
    /// UI calls [`spaced_repetition::review`] with [`Self::fsrs`] then
    /// hands the result here before upserting.
    pub fn set_fsrs(&mut self, next: &FsrsCard) {
        self.stability = next.stability;
        self.difficulty = next.difficulty;
        self.reps = next.reps;
        self.lapses = next.lapses;
        self.last_review = next.last_review.map(|d| d.to_string());
        self.due = next.due.map(|d| d.to_string());
    }

    /// Review this card with `rating` on `today` and reschedule it in
    /// place, targeting [`DEFAULT_RETENTION`]. Convenience over
    /// [`Self::fsrs`] + [`spaced_repetition::review`] + [`Self::set_fsrs`].
    pub fn review(&mut self, rating: Rating, today: NaiveDate) {
        let next = spaced_repetition::review(&self.fsrs(), rating, today, DEFAULT_RETENTION);
        self.set_fsrs(&next);
    }
}

/// Client-side optimistic store identity (`architect::Store`): keyed
/// by the stable `id`.
#[cfg(feature = "atom")]
impl architect::StoreEntity for RecallCard {
    type Key = String;
    fn key(&self) -> String {
        self.id.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card() -> RecallCard {
        RecallCard::create(
            "id-1",
            "Bible",
            CardType::REFERENCE_TO_VERSE,
            "John 3:16",
            "For God so loved the world…",
            "2026-07-16T09:00:00Z",
        )
    }

    #[test]
    fn new_card_is_always_due() {
        let c = card();
        assert!(c.in_review_queue("2026-07-16"));
        assert!(c.fsrs().is_new());
    }

    #[test]
    fn archived_never_in_queue() {
        let mut c = card();
        c.archived = true;
        assert!(!c.in_review_queue("2999-01-01"));
    }

    #[test]
    fn review_advances_due_and_reps() {
        let mut c = card();
        c.review(Rating::Good, NaiveDate::from_ymd_opt(2026, 7, 16).unwrap());
        assert_eq!(c.reps, 1);
        assert!(c.stability > 0.0);
        assert!(c.due.is_some());
        // Not due again immediately.
        assert!(!c.in_review_queue("2026-07-16"));
    }
}
