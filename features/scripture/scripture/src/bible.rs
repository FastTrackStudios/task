//! [`Bible`] — an in-memory, ordered store of one translation's verses.
//!
//! Keyed by [`VerseId`] in a `BTreeMap`, so iteration and chapter slices
//! come out in canonical order for free. Read-only after ingest.
//!
//! The verses are loaded from the **resource library** on disk
//! ([`Bible::load_dir`]) — a translation directory of per-book USFM
//! files, e.g. `<org>/resources/bible/WEB/JHN.usfm`. The corpus is not
//! bundled in the repo; it's primary-source data that the vault and wiki
//! link *into*. See [`crate::install`] for putting USFM there.

use std::collections::BTreeMap;
use std::path::Path;

use scripture_proto::{Book, VerseId};

use crate::usfm::{UsfmError, Word, parse_book};

/// One translation's verses, addressable by [`VerseId`].
#[derive(Debug, Clone, Default)]
pub struct Bible {
    /// Translation id, e.g. `WEB`.
    pub translation: String,
    verses: BTreeMap<VerseId, String>,
    /// Strong's-tagged words per verse (empty for untagged editions).
    words: BTreeMap<VerseId, Vec<Word>>,
    /// Concordance: Strong's code → verses that use it, in order.
    concordance: BTreeMap<String, Vec<VerseId>>,
}

/// Failure loading a translation directory.
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("io reading {path:?}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("parsing {path:?}: {source}")]
    Usfm { path: String, source: UsfmError },
}

impl Bible {
    /// An empty store for the given translation id.
    #[must_use]
    pub fn new(translation: impl Into<String>) -> Self {
        Self {
            translation: translation.into(),
            verses: BTreeMap::new(),
            words: BTreeMap::new(),
            concordance: BTreeMap::new(),
        }
    }

    /// Parse a USFM book and merge its verses in. Returns the number of
    /// verses added. Also indexes each Strong's-tagged word into the
    /// concordance (a word tagged with several codes lands under each).
    pub fn insert_usfm_book(&mut self, src: &str) -> Result<usize, UsfmError> {
        let parsed = parse_book(src)?;
        let n = parsed.len();
        for v in parsed {
            for word in &v.words {
                for code in word.strongs.split_whitespace() {
                    let entry = self.concordance.entry(code.to_string()).or_default();
                    if entry.last() != Some(&v.id) {
                        entry.push(v.id);
                    }
                }
            }
            if !v.words.is_empty() {
                self.words.insert(v.id, v.words);
            }
            self.verses.insert(v.id, v.text);
        }
        Ok(n)
    }

    /// The Strong's-tagged words of a verse, in order (empty if the verse
    /// is absent or the edition is untagged).
    #[must_use]
    pub fn words_of(&self, id: VerseId) -> &[Word] {
        self.words.get(&id).map_or(&[], Vec::as_slice)
    }

    /// Every verse using a Strong's code, in canonical order.
    #[must_use]
    pub fn occurrences(&self, strongs: &str) -> &[VerseId] {
        self.concordance.get(strongs).map_or(&[], Vec::as_slice)
    }

    /// Whether this edition carries Strong's tags.
    #[must_use]
    pub fn is_tagged(&self) -> bool {
        !self.concordance.is_empty()
    }

    /// Load every `*.usfm` file in a translation directory (e.g.
    /// `<org>/resources/bible/WEB/`) into a fresh store. `translation`
    /// is the id to tag the store with (typically the directory name).
    pub fn load_dir(dir: &Path, translation: impl Into<String>) -> Result<Self, LoadError> {
        let mut bible = Self::new(translation);
        let entries = std::fs::read_dir(dir).map_err(|source| LoadError::Io {
            path: dir.display().to_string(),
            source,
        })?;
        // Collect + sort so ingest order is deterministic.
        let mut files: Vec<_> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.extension()
                    .is_some_and(|x| x.eq_ignore_ascii_case("usfm"))
            })
            .collect();
        files.sort();
        for path in files {
            let src = std::fs::read_to_string(&path).map_err(|source| LoadError::Io {
                path: path.display().to_string(),
                source,
            })?;
            bible
                .insert_usfm_book(&src)
                .map_err(|source| LoadError::Usfm {
                    path: path.display().to_string(),
                    source,
                })?;
        }
        Ok(bible)
    }

    /// Clean reading text for one verse, if present.
    #[must_use]
    pub fn get(&self, id: VerseId) -> Option<&str> {
        self.verses.get(&id).map(String::as_str)
    }

    /// Every verse whose id falls in `start..=end` (inclusive), in order
    /// — may span chapters and books. Powers range comparisons.
    #[must_use]
    pub fn verses_in_range(&self, start: VerseId, end: VerseId) -> Vec<(VerseId, &str)> {
        self.verses
            .range(start..=end)
            .map(|(id, text)| (*id, text.as_str()))
            .collect()
    }

    /// All verses of a chapter, in order, as `(verse_number, text)`.
    #[must_use]
    pub fn chapter(&self, book: Book, chapter: u16) -> Vec<(u16, &str)> {
        let lo = VerseId::new(book, chapter, 0);
        let hi = VerseId::new(book, chapter, u16::MAX);
        self.verses
            .range(lo..=hi)
            .map(|(id, text)| (id.verse, text.as_str()))
            .collect()
    }

    /// Total verses loaded.
    #[must_use]
    pub fn len(&self) -> usize {
        self.verses.len()
    }

    /// Whether any verses are loaded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.verses.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_resolve_a_verse() {
        let mut bible = Bible::new("WEB");
        bible.insert_usfm_book(crate::usfm::tests::SAMPLE).unwrap();
        let id = VerseId::parse("John 3:16").unwrap();
        assert!(
            bible
                .get(id)
                .unwrap()
                .starts_with("For God so loved the world")
        );
        assert!(bible.get(VerseId::parse("Genesis 1:1").unwrap()).is_none());
    }

    #[test]
    fn chapter_slice_is_ordered() {
        let mut bible = Bible::new("WEB");
        bible.insert_usfm_book(crate::usfm::tests::SAMPLE).unwrap();
        let john = Book::lookup("John").unwrap();
        let ch3 = bible.chapter(john, 3);
        assert_eq!(
            ch3.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
            vec![16, 17]
        );
    }

    #[test]
    fn load_dir_reads_a_translation_folder() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("JHN.usfm"), crate::usfm::tests::SAMPLE).unwrap();
        let bible = Bible::load_dir(dir.path(), "WEB").unwrap();
        assert_eq!(bible.translation, "WEB");
        assert_eq!(bible.len(), 3);
        assert!(bible.get(VerseId::parse("John 4:1").unwrap()).is_some());
    }
}
