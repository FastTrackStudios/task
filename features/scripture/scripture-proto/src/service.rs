//! `ScriptureService` — the read-only wire surface the reader binds to.
//!
//! Decorated with `#[architect::rpc]`: the backend (`scripture::Store`)
//! implements the sync trait directly for zero-cost in-process calls, and
//! the web reader reaches the same surface via the auto-emitted vox
//! client. All wire types are plain `Facet` DTOs with primitive fields —
//! the pure [`crate::VerseId`] / [`crate::Book`] logic types stay
//! server-side; the wire carries strings and numbers.

use facet::Facet;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet, Error)]
#[repr(u8)]
pub enum ScriptureError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    /// An API-backed edition (ESV / NIV) failed upstream — network
    /// error, bad/missing key, rate limit, or the edition isn't on the
    /// configured key.
    #[error("upstream: {0}")]
    Upstream(String),
}

/// One installed translation, for the picker.
#[derive(Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
pub struct TranslationInfo {
    /// Short id, e.g. `WEB`.
    pub id: String,
    /// Full name, e.g. `World English Bible`.
    pub name: String,
    /// Short license summary.
    pub license: String,
    /// Bundled offline (vs API-fetched).
    pub bundled: bool,
}

/// One verse line in a rendered chapter.
#[derive(Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
pub struct VerseLine {
    pub verse: u16,
    /// Stable OSIS id, e.g. `John.3.16` — the link/permalink key.
    pub osis: String,
    /// Clean reading text.
    pub text: String,
}

/// One chapter of one translation, ready to render.
#[derive(Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
pub struct ChapterView {
    pub translation: String,
    /// OSIS book abbreviation, e.g. `John`.
    pub book_osis: String,
    /// Display name, e.g. `John`.
    pub book_name: String,
    /// Canonical book ordinal, `1..=66`.
    pub book_ordinal: u8,
    pub chapter: u16,
    /// Total chapters in this book (for prev/next nav).
    pub chapter_count: u8,
    pub verses: Vec<VerseLine>,
}

/// One vault note that links to a verse, with context.
#[derive(Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
pub struct VerseBacklink {
    /// Vault-relative path of the linking note.
    pub note_path: String,
    /// Display title (first heading, else file stem).
    pub note_title: String,
    /// The line the link appears on, trimmed — the "inline thesis".
    pub excerpt: String,
}

/// Backlinks for one verse — every vault note that references it.
///
/// Translation-independent: keyed by the verse address, so a note that
/// links `John 3:16` shows up no matter which edition you're reading.
#[derive(Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
pub struct VerseBacklinks {
    pub verse: u16,
    pub osis: String,
    pub notes: Vec<VerseBacklink>,
}

/// One verse across the compared translations — a row in a parallel
/// view. `cells` aligns with [`ComparisonView::translations`]; a missing
/// verse in some edition is an empty string.
#[derive(Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
pub struct ComparisonRow {
    /// Display reference, e.g. `John 3:16`.
    pub reference: String,
    pub osis: String,
    pub cells: Vec<String>,
}

/// A resolved translation comparison — the data a `compare` block
/// renders. Row-oriented (verse × translation) so a parallel table drops
/// straight in.
#[derive(Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
pub struct ComparisonView {
    /// Display reference for the whole span, e.g. `John 3:1–21`.
    pub reference: String,
    pub osis: String,
    /// Column order — the translations actually compared.
    pub translations: Vec<String>,
    pub rows: Vec<ComparisonRow>,
}

/// A Strong's lexicon entry — what a Strong's number means.
#[derive(Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
pub struct LexiconEntry {
    /// Strong's code, e.g. `G25`.
    pub strongs: String,
    /// Original-language headword, e.g. `ἀγάπη`.
    pub lemma: String,
    /// Transliteration, e.g. `agápē`.
    pub translit: String,
    /// Strong's definition.
    pub definition: String,
    /// KJV translation glosses.
    pub kjv_def: String,
    /// Etymology / derivation note.
    pub derivation: String,
}

/// One word of a verse, broken out for interlinear / word study. The
/// lexicon fields are filled in when the Strong's code is known.
#[derive(Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
pub struct WordToken {
    /// Surface form as printed in the translation.
    pub surface: String,
    /// Strong's code(s), e.g. `G25`.
    pub strongs: String,
    pub lemma: String,
    pub translit: String,
    /// Short gloss (KJV glosses, else the definition).
    pub gloss: String,
}

/// One concordance hit — a verse using a Strong's code.
#[derive(Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
pub struct Occurrence {
    pub osis: String,
    /// Display reference, e.g. `John 3:16`.
    pub reference: String,
    pub text: String,
}

/// One installed original-language edition, for the picker.
#[derive(Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
pub struct OrigEditionInfo {
    pub id: String,
    pub name: String,
    pub language: String,
}

/// One word in an interlinear line — the original word with its lemma,
/// parsing, and gloss (filled from the lexicon where the edition itself
/// doesn't carry them).
#[derive(Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
pub struct InterlinearWord {
    /// Surface form (Greek / pointed Hebrew).
    pub word: String,
    pub translit: String,
    pub lemma: String,
    /// Source-faithful Strong's (`G0025`, `H0376G`), or empty.
    pub strong: String,
    /// Morphology / parsing code.
    pub morph: String,
    pub gloss: String,
}

/// The full word study for a Strong's number — lexicon entry plus the
/// complete concordance (every occurrence), drawn from the fully-tagged
/// original-language text and shown with English verse text.
#[derive(Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
pub struct WordStudyReport {
    /// The code as queried (`G25`, `G0025`, `H0376G`).
    pub strongs: String,
    /// Canonical lexicon key (`G25`).
    pub normalized: String,
    pub lemma: String,
    pub translit: String,
    pub definition: String,
    pub kjv_def: String,
    pub derivation: String,
    /// Total verses where the lemma occurs (before any cap).
    pub total_occurrences: u32,
    /// Each occurrence (capped), with English verse text where available.
    pub occurrences: Vec<Occurrence>,
}

/// A weighted verse reference — a cross-reference target or a topic
/// member. `votes` is the OpenBible weight (negative = downvoted), the
/// confidence signal for the graph.
#[derive(Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
pub struct WeightedRef {
    /// OSIS id or range (`John.3.16`, `Prov.8.22-Prov.8.30`).
    pub osis: String,
    /// Display reference.
    pub reference: String,
    pub votes: i32,
}

/// A topic a verse is tagged with, and its weight.
#[derive(Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
pub struct TopicTag {
    pub topic: String,
    pub votes: i32,
}

#[architect::rpc]
pub trait ScriptureService {
    /// Installed translations, bundled editions first.
    fn translations(&self) -> Result<Vec<TranslationInfo>, ScriptureError>;

    /// One chapter. `book` accepts any spelling (name / OSIS / USFM id /
    /// common abbreviation); `BadRequest` if it doesn't resolve to a
    /// canonical book, `NotFound` if the translation or chapter is absent.
    /// Async because API-backed editions (ESV / NIV) fetch over HTTP.
    async fn chapter(
        &self,
        translation: &str,
        book: &str,
        chapter: u16,
    ) -> Result<ChapterView, ScriptureError>;

    /// A single verse. `reference` is a human or OSIS reference such as
    /// `John 3:16` or `John.3.16`.
    async fn verse(&self, translation: &str, reference: &str) -> Result<VerseLine, ScriptureError>;

    /// Compare a verse or range across translations — the engine behind a
    /// `compare` block. `reference` may span chapters or books
    /// (`Genesis 4:3-Exodus 15:17`). An empty `translations` list compares
    /// every installed edition (bundled-first); otherwise only the listed
    /// ids that are installed, in the given order. Mixed bundled + API
    /// editions are fine.
    async fn compare(
        &self,
        reference: &str,
        translations: Vec<String>,
    ) -> Result<ComparisonView, ScriptureError>;

    /// Per-verse backlinks for a whole chapter: every vault note that
    /// links a verse in `book` `chapter` via a `[[John 3:16]]`-style
    /// wikilink. Only verses with at least one backlink are returned.
    /// Translation-independent (keyed by verse address).
    fn chapter_backlinks(
        &self,
        book: &str,
        chapter: u16,
    ) -> Result<Vec<VerseBacklinks>, ScriptureError>;

    /// The lexicon entry for a Strong's code (`G25` / `H1`).
    fn lexicon(&self, strongs: &str) -> Result<LexiconEntry, ScriptureError>;

    /// Word-by-word breakdown of a verse from a Strong's-tagged edition —
    /// each tagged word with its lemma, transliteration, and gloss.
    fn word_study(
        &self,
        translation: &str,
        reference: &str,
    ) -> Result<Vec<WordToken>, ScriptureError>;

    /// Every verse using a Strong's code (the concordance), drawn from
    /// `translation`'s tagged text, capped at `limit` (0 ⇒ a default cap).
    fn occurrences(
        &self,
        strongs: &str,
        translation: &str,
        limit: u32,
    ) -> Result<Vec<Occurrence>, ScriptureError>;

    /// Installed original-language editions (TAGNT / TAHOT / SBLGNT / OSHB).
    fn original_editions(&self) -> Result<Vec<OrigEditionInfo>, ScriptureError>;

    /// The interlinear breakdown of a verse in an original-language
    /// edition — each original word with lemma, parsing and gloss, the
    /// gaps filled from the lexicon.
    fn interlinear(
        &self,
        edition: &str,
        reference: &str,
    ) -> Result<Vec<InterlinearWord>, ScriptureError>;

    /// The full word study for a Strong's code: lexicon entry + the
    /// complete concordance (drawn from the fully-tagged original-language
    /// text, so coverage is complete even where the English edition's tags
    /// are sparse), capped at `limit` (0 ⇒ a default cap).
    fn study(&self, strongs: &str, limit: u32) -> Result<WordStudyReport, ScriptureError>;

    /// Cross-references from a verse (OpenBible, votes-desc), keeping only
    /// links with votes ≥ `min_votes` (use 1 to drop downvoted links).
    fn cross_refs(
        &self,
        reference: &str,
        min_votes: i32,
    ) -> Result<Vec<WeightedRef>, ScriptureError>;

    /// Topics a verse is tagged with (OpenBible, votes-desc).
    fn topics_of(&self, reference: &str) -> Result<Vec<TopicTag>, ScriptureError>;

    /// Verses about a topic (OpenBible, votes-desc), capped at `limit`
    /// (0 ⇒ a default cap). Case-insensitive topic name.
    fn verses_for_topic(&self, topic: &str, limit: u32)
    -> Result<Vec<WeightedRef>, ScriptureError>;
}
