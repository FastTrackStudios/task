//! Cross-references — verse↔verse links from the OpenBible.info dataset
//! (CC BY, ~340k refs, seeded from the Treasury of Scripture Knowledge).
//!
//! Each row is `From → To (Votes)`; `To` may be a range, and **votes are
//! signed** — a negative vote marks a downvoted/bad link, so callers can
//! filter on a threshold. The vote count is the built-in confidence
//! signal that feeds the knowledge graph.

use std::collections::BTreeMap;
use std::path::Path;

use scripture_proto::{VerseId, VerseRange};

use crate::bible::LoadError;

/// One cross-reference target with its vote weight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrossRefEntry {
    pub target: VerseRange,
    pub votes: i32,
}

/// `from-verse → [targets]`, each list sorted by votes (desc).
#[derive(Debug, Clone, Default)]
pub struct CrossRefs {
    by_from: BTreeMap<VerseId, Vec<CrossRefEntry>>,
}

impl CrossRefs {
    /// Parse the OpenBible `cross_references.txt` (TSV). Unparseable and
    /// header rows are skipped.
    #[must_use]
    pub fn parse(tsv: &str) -> Self {
        let mut by_from: BTreeMap<VerseId, Vec<CrossRefEntry>> = BTreeMap::new();
        for line in tsv.lines() {
            let mut cols = line.split('\t');
            let (Some(from), Some(to), Some(votes)) = (cols.next(), cols.next(), cols.next())
            else {
                continue;
            };
            let (Ok(from), Ok(target)) = (VerseId::parse(from), VerseRange::parse(to)) else {
                continue;
            };
            let votes: i32 = votes.trim().parse().unwrap_or(0);
            by_from
                .entry(from)
                .or_default()
                .push(CrossRefEntry { target, votes });
        }
        for list in by_from.values_mut() {
            list.sort_by(|a, b| b.votes.cmp(&a.votes));
        }
        Self { by_from }
    }

    /// Load from a `cross_references.txt` file (missing ⇒ empty).
    pub fn load(path: &Path) -> Result<Self, LoadError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(Self::parse(&text)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(LoadError::Io {
                path: path.display().to_string(),
                source,
            }),
        }
    }

    /// Cross-references from `verse`, votes-desc, with votes ≥ `min_votes`.
    #[must_use]
    pub fn from(&self, verse: VerseId, min_votes: i32) -> Vec<&CrossRefEntry> {
        self.by_from
            .get(&verse)
            .map(|l| l.iter().filter(|e| e.votes >= min_votes).collect())
            .unwrap_or_default()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_from.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_sorts_and_filters() {
        let tsv = "From Verse\tTo Verse\tVotes\t#header\n\
                   Gen.1.1\tJohn.1.1\t90\n\
                   Gen.1.1\tIsa.44.24\t95\n\
                   Gen.1.1\tProv.8.22-Prov.8.30\t-37\n";
        let cr = CrossRefs::parse(tsv);
        let refs = cr.from(VerseId::parse("Genesis 1:1").unwrap(), 1);
        // Sorted by votes desc, the negative one filtered out.
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].votes, 95);
        assert_eq!(
            refs[0].target.start,
            VerseId::parse("Isaiah 44:24").unwrap()
        );
        // Range target preserved; included only when threshold allows.
        let all = cr.from(VerseId::parse("Genesis 1:1").unwrap(), -100);
        assert!(all.iter().any(|e| !e.target.is_single()));
    }
}
