//! [`VerseId`] — the stable address every layer links to.
//!
//! A verse carries two canonical keys:
//! - the `OSIS` string, `John.3.16` — the human / interchange form, and
//! - the `BBCCCVVV` integer, `43003016` — `book*1_000_000 +
//!   chapter*1000 + verse`, which sorts in canonical order.
//!
//! [`VerseId::parse`] accepts both the human form (`John 3:16`,
//! `1 John 2:3`, `Song of Solomon 1:1`) and the dotted `OSIS` form
//! (`John.3.16`).

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::book::Book;

/// Why a reference string failed to parse.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RefError {
    #[error("empty reference")]
    Empty,
    #[error("unknown book: {0:?}")]
    UnknownBook(String),
    #[error("malformed chapter:verse in {0:?}")]
    BadNumbers(String),
}

/// A single verse: book + chapter + verse.
///
/// Field order is `(book, chapter, verse)` so the derived ordering is
/// canonical reading order, matching [`VerseId::numeric`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct VerseId {
    pub book: Book,
    pub chapter: u16,
    pub verse: u16,
}

impl VerseId {
    /// Construct directly. No range validation against the canon's
    /// chapter/verse counts — that's the ingest layer's job.
    #[must_use]
    pub const fn new(book: Book, chapter: u16, verse: u16) -> Self {
        Self {
            book,
            chapter,
            verse,
        }
    }

    /// The sortable `BBCCCVVV` integer key, e.g. `John 3:16` →
    /// `43_003_016`.
    #[must_use]
    pub fn numeric(self) -> u32 {
        u32::from(self.book.ordinal()) * 1_000_000
            + u32::from(self.chapter) * 1_000
            + u32::from(self.verse)
    }

    /// Rebuild a [`VerseId`] from its `BBCCCVVV` integer key.
    #[must_use]
    pub fn from_numeric(n: u32) -> Option<Self> {
        let book = Book::from_ordinal((n / 1_000_000) as u8)?;
        let chapter = ((n / 1_000) % 1_000) as u16;
        let verse = (n % 1_000) as u16;
        Some(Self {
            book,
            chapter,
            verse,
        })
    }

    /// The `OSIS` reference string, e.g. `John.3.16`.
    #[must_use]
    pub fn osis(self) -> String {
        format!("{}.{}.{}", self.book.osis(), self.chapter, self.verse)
    }

    /// Parse a human (`John 3:16`) or `OSIS` (`John.3.16`) reference.
    pub fn parse(s: &str) -> Result<Self, RefError> {
        let s = s.trim();
        if s.is_empty() {
            return Err(RefError::Empty);
        }

        // Split the book name from the trailing chapter:verse numbers.
        let (book_part, ch, v) = if let Some((book_part, tail)) = s.rsplit_once(' ') {
            // Human form: "<book> <chapter>:<verse>".
            let (ch, v) =
                parse_chapter_verse(tail).ok_or_else(|| RefError::BadNumbers(s.into()))?;
            (book_part.to_string(), ch, v)
        } else {
            // No space: dotted OSIS form "<book>.<chapter>.<verse>".
            let mut parts = s.rsplitn(3, '.');
            let v = parts.next();
            let ch = parts.next();
            let book_part = parts.next();
            match (book_part, ch, v) {
                (Some(b), Some(c), Some(vv)) => {
                    let ch = c.parse().map_err(|_| RefError::BadNumbers(s.into()))?;
                    let v = vv.parse().map_err(|_| RefError::BadNumbers(s.into()))?;
                    (b.to_string(), ch, v)
                }
                _ => return Err(RefError::BadNumbers(s.into())),
            }
        };

        let book = Book::lookup(book_part.trim()).ok_or(RefError::UnknownBook(book_part))?;
        Ok(Self {
            book,
            chapter: ch,
            verse: v,
        })
    }
}

impl std::fmt::Display for VerseId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}:{}", self.book.name(), self.chapter, self.verse)
    }
}

/// Parse a `chapter:verse` or `chapter.verse` tail into `(chapter,
/// verse)`.
fn parse_chapter_verse(tail: &str) -> Option<(u16, u16)> {
    let (c, v) = tail.split_once(':').or_else(|| tail.split_once('.'))?;
    Some((c.trim().parse().ok()?, v.trim().parse().ok()?))
}

/// An inclusive span of verses — `John 3:16`, `John 3:16-20`,
/// `John 3:20-7:26`, or `Genesis 4:3-Exodus 15:17`.
///
/// A single verse is the degenerate range with `start == end`. Ordering
/// uses the global `BBCCCVVV` key, so [`VerseRange::contains`] works
/// across chapter and book boundaries — handy for structural analysis of
/// a passage that spans books.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerseRange {
    pub start: VerseId,
    pub end: VerseId,
}

impl VerseRange {
    /// A range covering a single verse.
    #[must_use]
    pub fn single(verse: VerseId) -> Self {
        Self {
            start: verse,
            end: verse,
        }
    }

    /// Does this range cover `verse`? Inclusive, by canonical order.
    #[must_use]
    pub fn contains(&self, verse: VerseId) -> bool {
        let n = verse.numeric();
        self.start.numeric() <= n && n <= self.end.numeric()
    }

    /// Whether the range is a single verse.
    #[must_use]
    pub fn is_single(&self) -> bool {
        self.start == self.end
    }

    /// Parse a reference that may be a single verse or a range. The left
    /// side is a full reference (`John 3:16`); the right side of a `-`
    /// (or `–`) may be a bare verse (`20`, same book+chapter), a
    /// `chapter:verse` (`7:26`, same book), or a full reference
    /// (`Exodus 15:17`).
    pub fn parse(s: &str) -> Result<Self, RefError> {
        let s = s.trim();
        let Some((left, right)) = s.split_once(['-', '\u{2013}']) else {
            return Ok(Self::single(VerseId::parse(s)?));
        };
        let start = VerseId::parse(left.trim())?;
        let end = parse_range_end(right.trim(), start)?;
        if end.numeric() < start.numeric() {
            return Err(RefError::BadNumbers(s.to_string()));
        }
        Ok(Self { start, end })
    }

    /// `OSIS` form: `John.3.16` for a single verse, else
    /// `John.3.16-John.3.20`.
    #[must_use]
    pub fn osis(&self) -> String {
        if self.is_single() {
            self.start.osis()
        } else {
            format!("{}-{}", self.start.osis(), self.end.osis())
        }
    }
}

/// A verse range with an optional translation qualifier — the parsed
/// form of a `[[John 3:16@ESV]]` link. The translation is a *display*
/// choice; the [`range`](Self::range) is still translation-independent,
/// so backlinks and comparisons key on the range and the qualifier just
/// says "show me this edition".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptureRef {
    pub range: VerseRange,
    /// Uppercased translation id (`ESV`), or `None` for the reader's
    /// current edition.
    pub translation: Option<String>,
}

impl ScriptureRef {
    /// Parse `John 3:16`, `John 3:16-20`, or a translation-qualified
    /// `John 3:16@ESV`.
    pub fn parse(s: &str) -> Result<Self, RefError> {
        let s = s.trim();
        if let Some((ref_part, tx)) = s.rsplit_once('@') {
            let tx = tx.trim();
            // A bare `@` with no edition is just a malformed ref.
            let translation = (!tx.is_empty()).then(|| tx.to_ascii_uppercase());
            Ok(Self {
                range: VerseRange::parse(ref_part.trim())?,
                translation,
            })
        } else {
            Ok(Self {
                range: VerseRange::parse(s)?,
                translation: None,
            })
        }
    }
}

/// Resolve the right-hand side of a range against the start reference,
/// filling in an omitted book and/or chapter.
fn parse_range_end(right: &str, start: VerseId) -> Result<VerseId, RefError> {
    if right.chars().any(|c| c.is_ascii_alphabetic()) {
        // Full reference, e.g. `Exodus 15:17` or `Exod.15.17`.
        VerseId::parse(right)
    } else if let Some((c, v)) = parse_chapter_verse(right) {
        // `chapter:verse` in the start's book.
        Ok(VerseId::new(start.book, c, v))
    } else if let Ok(v) = right.parse::<u16>() {
        // Bare verse number in the start's book + chapter.
        Ok(VerseId::new(start.book, start.chapter, v))
    } else {
        Err(RefError::BadNumbers(right.to_string()))
    }
}

impl std::fmt::Display for VerseRange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (s, e) = (self.start, self.end);
        if self.is_single() {
            write!(f, "{s}")
        } else if s.book == e.book && s.chapter == e.chapter {
            write!(
                f,
                "{} {}:{}\u{2013}{}",
                s.book.name(),
                s.chapter,
                s.verse,
                e.verse
            )
        } else if s.book == e.book {
            write!(
                f,
                "{} {}:{}\u{2013}{}:{}",
                s.book.name(),
                s.chapter,
                s.verse,
                e.chapter,
                e.verse
            )
        } else {
            write!(f, "{s}\u{2013}{e}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn john_3_16() -> VerseId {
        VerseId::new(Book::from_ordinal(43).unwrap(), 3, 16)
    }

    #[test]
    fn numeric_key_matches_spec() {
        assert_eq!(john_3_16().numeric(), 43_003_016);
        assert_eq!(
            VerseId::new(Book::from_ordinal(1).unwrap(), 1, 1).numeric(),
            1_001_001
        );
    }

    #[test]
    fn numeric_round_trips() {
        let v = john_3_16();
        assert_eq!(VerseId::from_numeric(v.numeric()), Some(v));
    }

    #[test]
    fn osis_string() {
        assert_eq!(john_3_16().osis(), "John.3.16");
    }

    #[test]
    fn parse_human_and_osis_forms() {
        for s in ["John 3:16", "john 3:16", "Jn 3:16", "John.3.16"] {
            assert_eq!(VerseId::parse(s).unwrap(), john_3_16(), "{s}");
        }
        // Multi-word and numbered books.
        let first_cor = VerseId::parse("1 Corinthians 13:4").unwrap();
        assert_eq!(first_cor.book.name(), "1 Corinthians");
        assert_eq!((first_cor.chapter, first_cor.verse), (13, 4));
        let song = VerseId::parse("Song of Solomon 1:1").unwrap();
        assert_eq!(song.book.name(), "Song of Solomon");
        assert_eq!(VerseId::parse("1John.2.3").unwrap().book.name(), "1 John");
    }

    #[test]
    fn parse_rejects_garbage() {
        assert_eq!(VerseId::parse(""), Err(RefError::Empty));
        assert!(matches!(
            VerseId::parse("Nope 1:1"),
            Err(RefError::UnknownBook(_))
        ));
        assert!(matches!(
            VerseId::parse("John x:y"),
            Err(RefError::BadNumbers(_))
        ));
    }

    #[test]
    fn range_parses_all_four_shapes() {
        // single verse
        let r = VerseRange::parse("John 3:16").unwrap();
        assert!(r.is_single());
        assert_eq!(r.start, john_3_16());

        // same chapter: John 3:16-20
        let r = VerseRange::parse("John 3:16-20").unwrap();
        assert_eq!(r.start, VerseId::parse("John 3:16").unwrap());
        assert_eq!(r.end, VerseId::parse("John 3:20").unwrap());

        // cross-chapter, same book: John 3:20-7:26
        let r = VerseRange::parse("John 3:20-7:26").unwrap();
        assert_eq!(r.start, VerseId::parse("John 3:20").unwrap());
        assert_eq!(r.end, VerseId::parse("John 7:26").unwrap());

        // cross-book: Genesis 4:3-Exodus 15:17
        let r = VerseRange::parse("Genesis 4:3-Exodus 15:17").unwrap();
        assert_eq!(r.start, VerseId::parse("Genesis 4:3").unwrap());
        assert_eq!(r.end, VerseId::parse("Exodus 15:17").unwrap());
    }

    #[test]
    fn range_contains_across_boundaries() {
        let r = VerseRange::parse("Genesis 4:3-Exodus 15:17").unwrap();
        assert!(r.contains(VerseId::parse("Genesis 50:1").unwrap()));
        assert!(r.contains(VerseId::parse("Exodus 15:17").unwrap()));
        assert!(!r.contains(VerseId::parse("Exodus 15:18").unwrap()));
        assert!(!r.contains(VerseId::parse("Genesis 4:2").unwrap()));
    }

    #[test]
    fn range_display_and_rejects_backwards() {
        assert_eq!(
            VerseRange::parse("John 3:16-20").unwrap().to_string(),
            "John 3:16\u{2013}20"
        );
        assert_eq!(
            VerseRange::parse("John 3:20-7:26").unwrap().to_string(),
            "John 3:20\u{2013}7:26"
        );
        assert_eq!(
            VerseRange::parse("Genesis 4:3-Exodus 15:17")
                .unwrap()
                .to_string(),
            "Genesis 4:3\u{2013}Exodus 15:17"
        );
        assert!(
            VerseRange::parse("John 3:20-3:16").is_err(),
            "backwards range rejected"
        );
    }

    #[test]
    fn scripture_ref_parses_translation_qualifier() {
        let r = ScriptureRef::parse("John 3:16@ESV").unwrap();
        assert_eq!(r.range, VerseRange::parse("John 3:16").unwrap());
        assert_eq!(r.translation.as_deref(), Some("ESV"));

        let r = ScriptureRef::parse("John 3:1-21@web").unwrap();
        assert_eq!(r.range, VerseRange::parse("John 3:1-21").unwrap());
        assert_eq!(
            r.translation.as_deref(),
            Some("WEB"),
            "translation upper-cased"
        );

        let r = ScriptureRef::parse("John 3:16").unwrap();
        assert_eq!(r.translation, None);
    }

    #[test]
    fn ordering_is_canonical() {
        let mut v = [
            VerseId::parse("John 3:16").unwrap(),
            VerseId::parse("Genesis 1:1").unwrap(),
            VerseId::parse("John 3:1").unwrap(),
            VerseId::parse("Revelation 22:21").unwrap(),
        ];
        v.sort();
        assert_eq!(v[0], VerseId::parse("Genesis 1:1").unwrap());
        assert_eq!(v[1], VerseId::parse("John 3:1").unwrap());
        assert_eq!(v[2], VerseId::parse("John 3:16").unwrap());
        assert_eq!(v[3], VerseId::parse("Revelation 22:21").unwrap());
    }
}
