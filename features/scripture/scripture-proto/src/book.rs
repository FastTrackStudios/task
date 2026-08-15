//! The 66-book Protestant canon.
//!
//! [`Book`] is a one-byte ordinal newtype (`1..=66`, Genesis = 1,
//! Revelation = 66) — the same numbering the `BBCCCVVV` verse key uses,
//! so a book's ordinal is also the high digits of every verse id in it.
//! Lookups (by `USFM` id, `OSIS` id, English name, or common
//! abbreviation) resolve against a static [`CANON`] table.

use serde::{Deserialize, Serialize};

/// One canon entry. `usfm` is the 3-letter Paratext/`USFM` id (`JHN`),
/// `osis` is the `OSIS` book abbreviation (`John`), `name` is the
/// display name, `chapters` is the chapter count in this versification.
#[derive(Debug, Clone, Copy)]
pub struct BookMeta {
    pub ordinal: u8,
    pub usfm: &'static str,
    pub osis: &'static str,
    pub name: &'static str,
    pub chapters: u8,
}

/// A book of the Bible, stored as its canonical ordinal (`1..=66`).
///
/// Construct via [`Book::from_ordinal`] or one of the `from_*` lookups;
/// the inner value is never out of range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(into = "u8", try_from = "u8")]
pub struct Book(u8);

impl Book {
    /// The book at the given canonical ordinal, or `None` outside
    /// `1..=66`.
    #[must_use]
    pub const fn from_ordinal(ordinal: u8) -> Option<Self> {
        if ordinal >= 1 && ordinal <= 66 {
            Some(Self(ordinal))
        } else {
            None
        }
    }

    /// Canonical ordinal, `1` (Genesis) through `66` (Revelation).
    #[must_use]
    pub const fn ordinal(self) -> u8 {
        self.0
    }

    /// Static metadata for this book.
    #[must_use]
    pub fn meta(self) -> &'static BookMeta {
        // `self.0` is always in `1..=66`, so the index is in bounds.
        &CANON[self.0 as usize - 1]
    }

    /// 3-letter `USFM`/Paratext id, e.g. `JHN`.
    #[must_use]
    pub fn usfm(self) -> &'static str {
        self.meta().usfm
    }

    /// `OSIS` book abbreviation, e.g. `John`.
    #[must_use]
    pub fn osis(self) -> &'static str {
        self.meta().osis
    }

    /// Display name, e.g. `John` or `1 Corinthians`.
    #[must_use]
    pub fn name(self) -> &'static str {
        self.meta().name
    }

    /// Number of chapters in this book.
    #[must_use]
    pub fn chapters(self) -> u8 {
        self.meta().chapters
    }

    /// Look up by 3-letter `USFM` id (case-insensitive).
    #[must_use]
    pub fn from_usfm(id: &str) -> Option<Self> {
        let id = id.trim();
        CANON
            .iter()
            .find(|b| b.usfm.eq_ignore_ascii_case(id))
            .and_then(|b| Self::from_ordinal(b.ordinal))
    }

    /// Look up by `OSIS` abbreviation (case-insensitive).
    #[must_use]
    pub fn from_osis(id: &str) -> Option<Self> {
        let id = id.trim();
        CANON
            .iter()
            .find(|b| b.osis.eq_ignore_ascii_case(id))
            .and_then(|b| Self::from_ordinal(b.ordinal))
    }

    /// Resolve a free-form book name: full name, `OSIS` id, `USFM` id,
    /// or a common abbreviation (`Gen`, `Ps`, `Jn`, `1 Cor`, `Phil`).
    ///
    /// Matching normalizes away whitespace, periods, and case, so
    /// `1 John`, `1John`, `1 jn`, and `1JN` all resolve.
    #[must_use]
    pub fn lookup(name: &str) -> Option<Self> {
        let norm = normalize(name);
        if norm.is_empty() {
            return None;
        }
        CANON
            .iter()
            .find(|b| {
                normalize(b.name) == norm || normalize(b.osis) == norm || normalize(b.usfm) == norm
            })
            .and_then(|b| Self::from_ordinal(b.ordinal))
            .or_else(|| {
                ALIASES
                    .iter()
                    .find(|(a, _)| normalize(a) == norm)
                    .map(|(_, o)| Self(*o))
            })
    }
}

impl From<Book> for u8 {
    fn from(b: Book) -> Self {
        b.0
    }
}

impl TryFrom<u8> for Book {
    type Error = &'static str;
    fn try_from(v: u8) -> Result<Self, Self::Error> {
        Self::from_ordinal(v).ok_or("book ordinal out of range 1..=66")
    }
}

impl std::fmt::Display for Book {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// Lower-case, strip every non-alphanumeric char (spaces, periods,
/// hyphens) so abbreviation spellings collapse to one key.
fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Common abbreviations that aren't the `USFM`/`OSIS` id or full name.
/// Keyed to the book ordinal. Kept small and high-traffic on purpose.
const ALIASES: &[(&str, u8)] = &[
    ("gen", 1),
    ("ex", 2),
    ("lev", 3),
    ("num", 4),
    ("deut", 5),
    ("dt", 5),
    ("josh", 6),
    ("jdg", 7),
    ("ps", 19),
    ("psalm", 19),
    ("psalms", 19),
    ("prov", 20),
    ("eccl", 21),
    ("song", 22),
    ("songofsongs", 22),
    ("isa", 23),
    ("jer", 24),
    ("ezek", 26),
    ("matt", 40),
    ("mk", 41),
    ("lk", 42),
    ("jn", 43),
    ("rom", 45),
    ("phil", 50),
    ("philem", 57),
    ("rev", 66),
];

/// The 66 books in canonical order. Index `n - 1` is ordinal `n`.
pub const CANON: [BookMeta; 66] = [
    BookMeta {
        ordinal: 1,
        usfm: "GEN",
        osis: "Gen",
        name: "Genesis",
        chapters: 50,
    },
    BookMeta {
        ordinal: 2,
        usfm: "EXO",
        osis: "Exod",
        name: "Exodus",
        chapters: 40,
    },
    BookMeta {
        ordinal: 3,
        usfm: "LEV",
        osis: "Lev",
        name: "Leviticus",
        chapters: 27,
    },
    BookMeta {
        ordinal: 4,
        usfm: "NUM",
        osis: "Num",
        name: "Numbers",
        chapters: 36,
    },
    BookMeta {
        ordinal: 5,
        usfm: "DEU",
        osis: "Deut",
        name: "Deuteronomy",
        chapters: 34,
    },
    BookMeta {
        ordinal: 6,
        usfm: "JOS",
        osis: "Josh",
        name: "Joshua",
        chapters: 24,
    },
    BookMeta {
        ordinal: 7,
        usfm: "JDG",
        osis: "Judg",
        name: "Judges",
        chapters: 21,
    },
    BookMeta {
        ordinal: 8,
        usfm: "RUT",
        osis: "Ruth",
        name: "Ruth",
        chapters: 4,
    },
    BookMeta {
        ordinal: 9,
        usfm: "1SA",
        osis: "1Sam",
        name: "1 Samuel",
        chapters: 31,
    },
    BookMeta {
        ordinal: 10,
        usfm: "2SA",
        osis: "2Sam",
        name: "2 Samuel",
        chapters: 24,
    },
    BookMeta {
        ordinal: 11,
        usfm: "1KI",
        osis: "1Kgs",
        name: "1 Kings",
        chapters: 22,
    },
    BookMeta {
        ordinal: 12,
        usfm: "2KI",
        osis: "2Kgs",
        name: "2 Kings",
        chapters: 25,
    },
    BookMeta {
        ordinal: 13,
        usfm: "1CH",
        osis: "1Chr",
        name: "1 Chronicles",
        chapters: 29,
    },
    BookMeta {
        ordinal: 14,
        usfm: "2CH",
        osis: "2Chr",
        name: "2 Chronicles",
        chapters: 36,
    },
    BookMeta {
        ordinal: 15,
        usfm: "EZR",
        osis: "Ezra",
        name: "Ezra",
        chapters: 10,
    },
    BookMeta {
        ordinal: 16,
        usfm: "NEH",
        osis: "Neh",
        name: "Nehemiah",
        chapters: 13,
    },
    BookMeta {
        ordinal: 17,
        usfm: "EST",
        osis: "Esth",
        name: "Esther",
        chapters: 10,
    },
    BookMeta {
        ordinal: 18,
        usfm: "JOB",
        osis: "Job",
        name: "Job",
        chapters: 42,
    },
    BookMeta {
        ordinal: 19,
        usfm: "PSA",
        osis: "Ps",
        name: "Psalms",
        chapters: 150,
    },
    BookMeta {
        ordinal: 20,
        usfm: "PRO",
        osis: "Prov",
        name: "Proverbs",
        chapters: 31,
    },
    BookMeta {
        ordinal: 21,
        usfm: "ECC",
        osis: "Eccl",
        name: "Ecclesiastes",
        chapters: 12,
    },
    BookMeta {
        ordinal: 22,
        usfm: "SNG",
        osis: "Song",
        name: "Song of Solomon",
        chapters: 8,
    },
    BookMeta {
        ordinal: 23,
        usfm: "ISA",
        osis: "Isa",
        name: "Isaiah",
        chapters: 66,
    },
    BookMeta {
        ordinal: 24,
        usfm: "JER",
        osis: "Jer",
        name: "Jeremiah",
        chapters: 52,
    },
    BookMeta {
        ordinal: 25,
        usfm: "LAM",
        osis: "Lam",
        name: "Lamentations",
        chapters: 5,
    },
    BookMeta {
        ordinal: 26,
        usfm: "EZK",
        osis: "Ezek",
        name: "Ezekiel",
        chapters: 48,
    },
    BookMeta {
        ordinal: 27,
        usfm: "DAN",
        osis: "Dan",
        name: "Daniel",
        chapters: 12,
    },
    BookMeta {
        ordinal: 28,
        usfm: "HOS",
        osis: "Hos",
        name: "Hosea",
        chapters: 14,
    },
    BookMeta {
        ordinal: 29,
        usfm: "JOL",
        osis: "Joel",
        name: "Joel",
        chapters: 3,
    },
    BookMeta {
        ordinal: 30,
        usfm: "AMO",
        osis: "Amos",
        name: "Amos",
        chapters: 9,
    },
    BookMeta {
        ordinal: 31,
        usfm: "OBA",
        osis: "Obad",
        name: "Obadiah",
        chapters: 1,
    },
    BookMeta {
        ordinal: 32,
        usfm: "JON",
        osis: "Jonah",
        name: "Jonah",
        chapters: 4,
    },
    BookMeta {
        ordinal: 33,
        usfm: "MIC",
        osis: "Mic",
        name: "Micah",
        chapters: 7,
    },
    BookMeta {
        ordinal: 34,
        usfm: "NAM",
        osis: "Nah",
        name: "Nahum",
        chapters: 3,
    },
    BookMeta {
        ordinal: 35,
        usfm: "HAB",
        osis: "Hab",
        name: "Habakkuk",
        chapters: 3,
    },
    BookMeta {
        ordinal: 36,
        usfm: "ZEP",
        osis: "Zeph",
        name: "Zephaniah",
        chapters: 3,
    },
    BookMeta {
        ordinal: 37,
        usfm: "HAG",
        osis: "Hag",
        name: "Haggai",
        chapters: 2,
    },
    BookMeta {
        ordinal: 38,
        usfm: "ZEC",
        osis: "Zech",
        name: "Zechariah",
        chapters: 14,
    },
    BookMeta {
        ordinal: 39,
        usfm: "MAL",
        osis: "Mal",
        name: "Malachi",
        chapters: 4,
    },
    BookMeta {
        ordinal: 40,
        usfm: "MAT",
        osis: "Matt",
        name: "Matthew",
        chapters: 28,
    },
    BookMeta {
        ordinal: 41,
        usfm: "MRK",
        osis: "Mark",
        name: "Mark",
        chapters: 16,
    },
    BookMeta {
        ordinal: 42,
        usfm: "LUK",
        osis: "Luke",
        name: "Luke",
        chapters: 24,
    },
    BookMeta {
        ordinal: 43,
        usfm: "JHN",
        osis: "John",
        name: "John",
        chapters: 21,
    },
    BookMeta {
        ordinal: 44,
        usfm: "ACT",
        osis: "Acts",
        name: "Acts",
        chapters: 28,
    },
    BookMeta {
        ordinal: 45,
        usfm: "ROM",
        osis: "Rom",
        name: "Romans",
        chapters: 16,
    },
    BookMeta {
        ordinal: 46,
        usfm: "1CO",
        osis: "1Cor",
        name: "1 Corinthians",
        chapters: 16,
    },
    BookMeta {
        ordinal: 47,
        usfm: "2CO",
        osis: "2Cor",
        name: "2 Corinthians",
        chapters: 13,
    },
    BookMeta {
        ordinal: 48,
        usfm: "GAL",
        osis: "Gal",
        name: "Galatians",
        chapters: 6,
    },
    BookMeta {
        ordinal: 49,
        usfm: "EPH",
        osis: "Eph",
        name: "Ephesians",
        chapters: 6,
    },
    BookMeta {
        ordinal: 50,
        usfm: "PHP",
        osis: "Phil",
        name: "Philippians",
        chapters: 4,
    },
    BookMeta {
        ordinal: 51,
        usfm: "COL",
        osis: "Col",
        name: "Colossians",
        chapters: 4,
    },
    BookMeta {
        ordinal: 52,
        usfm: "1TH",
        osis: "1Thess",
        name: "1 Thessalonians",
        chapters: 5,
    },
    BookMeta {
        ordinal: 53,
        usfm: "2TH",
        osis: "2Thess",
        name: "2 Thessalonians",
        chapters: 3,
    },
    BookMeta {
        ordinal: 54,
        usfm: "1TI",
        osis: "1Tim",
        name: "1 Timothy",
        chapters: 6,
    },
    BookMeta {
        ordinal: 55,
        usfm: "2TI",
        osis: "2Tim",
        name: "2 Timothy",
        chapters: 4,
    },
    BookMeta {
        ordinal: 56,
        usfm: "TIT",
        osis: "Titus",
        name: "Titus",
        chapters: 3,
    },
    BookMeta {
        ordinal: 57,
        usfm: "PHM",
        osis: "Phlm",
        name: "Philemon",
        chapters: 1,
    },
    BookMeta {
        ordinal: 58,
        usfm: "HEB",
        osis: "Heb",
        name: "Hebrews",
        chapters: 13,
    },
    BookMeta {
        ordinal: 59,
        usfm: "JAS",
        osis: "Jas",
        name: "James",
        chapters: 5,
    },
    BookMeta {
        ordinal: 60,
        usfm: "1PE",
        osis: "1Pet",
        name: "1 Peter",
        chapters: 5,
    },
    BookMeta {
        ordinal: 61,
        usfm: "2PE",
        osis: "2Pet",
        name: "2 Peter",
        chapters: 3,
    },
    BookMeta {
        ordinal: 62,
        usfm: "1JN",
        osis: "1John",
        name: "1 John",
        chapters: 5,
    },
    BookMeta {
        ordinal: 63,
        usfm: "2JN",
        osis: "2John",
        name: "2 John",
        chapters: 1,
    },
    BookMeta {
        ordinal: 64,
        usfm: "3JN",
        osis: "3John",
        name: "3 John",
        chapters: 1,
    },
    BookMeta {
        ordinal: 65,
        usfm: "JUD",
        osis: "Jude",
        name: "Jude",
        chapters: 1,
    },
    BookMeta {
        ordinal: 66,
        usfm: "REV",
        osis: "Rev",
        name: "Revelation",
        chapters: 22,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canon_is_complete_and_ordered() {
        assert_eq!(CANON.len(), 66);
        for (i, b) in CANON.iter().enumerate() {
            assert_eq!(b.ordinal as usize, i + 1, "ordinal matches position");
        }
    }

    #[test]
    fn ordinal_round_trips() {
        let john = Book::from_ordinal(43).unwrap();
        assert_eq!(john.name(), "John");
        assert_eq!(john.usfm(), "JHN");
        assert_eq!(john.osis(), "John");
        assert_eq!(john.chapters(), 21);
        assert_eq!(Book::from_ordinal(0), None);
        assert_eq!(Book::from_ordinal(67), None);
    }

    #[test]
    fn lookups_resolve_many_spellings() {
        let john = Book::from_ordinal(43).unwrap();
        for s in ["John", "john", "JHN", "Jn", "jn"] {
            assert_eq!(Book::lookup(s), Some(john), "{s} resolves to John");
        }
        let first_john = Book::from_ordinal(62).unwrap();
        for s in ["1 John", "1John", "1 jn", "1JN"] {
            assert_eq!(Book::lookup(s), Some(first_john), "{s} resolves to 1 John");
        }
        assert_eq!(Book::lookup("Song of Solomon"), Book::from_ordinal(22));
        assert_eq!(Book::lookup("Ps"), Book::from_ordinal(19));
        assert_eq!(Book::lookup("Nope"), None);
    }
}
