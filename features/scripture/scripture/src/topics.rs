//! Topical tags — verse↔topic from OpenBible.info (CC BY, vote-weighted):
//! "where does the Bible talk about money / pain / forgiveness".
//!
//! Each row is `Topic → verse(-range) (Votes)` with `bbcccvvv` verse ids.
//! We index both directions: topic → verses (the "verses about X" query)
//! and verse → topics (the tags on a verse), votes as the weight. Topic
//! names are interned so the verse→topic index stays compact.

use std::collections::BTreeMap;
use std::path::Path;

use scripture_proto::{VerseId, VerseRange};

use crate::bible::LoadError;

/// A verse(-range) tagged with a topic, and its vote weight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopicVerse {
    pub range: VerseRange,
    pub votes: i32,
}

/// Bidirectional topic index.
#[derive(Debug, Clone, Default)]
pub struct Topics {
    /// Interned topic display names.
    names: Vec<String>,
    /// Lower-cased topic name → topic id.
    ids: BTreeMap<String, usize>,
    /// topic id → verses (votes-desc).
    by_topic: Vec<Vec<TopicVerse>>,
    /// verse → (topic id, votes).
    by_verse: BTreeMap<VerseId, Vec<(usize, i32)>>,
}

impl Topics {
    /// Parse the OpenBible `topic-votes.txt` (TSV, `bbcccvvv` ids).
    #[must_use]
    pub fn parse(tsv: &str) -> Self {
        let mut t = Self::default();
        for line in tsv.lines() {
            let mut cols = line.split('\t');
            let (Some(topic), Some(start), end, Some(votes)) =
                (cols.next(), cols.next(), cols.next(), cols.next())
            else {
                continue;
            };
            let Some(range) = parse_range(start, end.unwrap_or("")) else {
                continue;
            };
            let votes: i32 = votes.trim().parse().unwrap_or(0);

            let id = t.intern(topic.trim());
            t.by_topic[id].push(TopicVerse { range, votes });
            for v in expand(range) {
                t.by_verse.entry(v).or_default().push((id, votes));
            }
        }
        for list in &mut t.by_topic {
            list.sort_by(|a, b| b.votes.cmp(&a.votes));
        }
        for list in t.by_verse.values_mut() {
            list.sort_by(|a, b| b.1.cmp(&a.1));
        }
        t
    }

    /// Load from a `topic-votes.txt` file (missing ⇒ empty).
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

    /// Verses tagged with `topic` (case-insensitive), votes-desc.
    #[must_use]
    pub fn verses_for(&self, topic: &str) -> &[TopicVerse] {
        self.ids
            .get(&topic.trim().to_lowercase())
            .map(|&id| self.by_topic[id].as_slice())
            .unwrap_or_default()
    }

    /// Topics on a verse as `(name, votes)`, votes-desc.
    #[must_use]
    pub fn of_verse(&self, verse: VerseId) -> Vec<(&str, i32)> {
        self.by_verse
            .get(&verse)
            .map(|l| {
                l.iter()
                    .map(|&(id, v)| (self.names[id].as_str(), v))
                    .collect()
            })
            .unwrap_or_default()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    fn intern(&mut self, name: &str) -> usize {
        let key = name.to_lowercase();
        if let Some(&id) = self.ids.get(&key) {
            return id;
        }
        let id = self.names.len();
        self.names.push(name.to_string());
        self.by_topic.push(Vec::new());
        self.ids.insert(key, id);
        id
    }
}

/// `bbcccvvv` start + optional end → a [`VerseRange`].
fn parse_range(start: &str, end: &str) -> Option<VerseRange> {
    let s = VerseId::from_numeric(start.trim().parse().ok()?)?;
    let end = end.trim();
    if end.is_empty() {
        return Some(VerseRange::single(s));
    }
    let e = VerseId::from_numeric(end.parse().ok()?)?;
    Some(VerseRange { start: s, end: e })
}

/// Verses of a within-chapter range (topic ranges almost always are).
fn expand(r: VerseRange) -> Vec<VerseId> {
    if r.start.book == r.end.book && r.start.chapter == r.end.chapter {
        (r.start.verse..=r.end.verse)
            .map(|v| VerseId::new(r.start.book, r.start.chapter, v))
            .collect()
    } else {
        vec![r.start, r.end]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexes_both_directions() {
        // money → Heb.13.5 (single) and 1Tim.6.10; plus a range.
        let tsv = "Topic\tStart\tEnd\tVotes\t#header\n\
                   money\t58013005\t\t10203\n\
                   money\t54006010\t\t9987\n\
                   10 commandments\t02020001\t02020026\t302\n";
        let t = Topics::parse(tsv);

        // "verses about money", votes-desc.
        let money = t.verses_for("MONEY"); // case-insensitive
        assert_eq!(money.len(), 2);
        assert_eq!(money[0].votes, 10203);
        assert_eq!(
            money[0].range.start,
            VerseId::parse("Hebrews 13:5").unwrap()
        );

        // topics on a verse (range expanded → Exodus 20:1 tagged).
        let tags = t.of_verse(VerseId::parse("Exodus 20:1").unwrap());
        assert_eq!(tags, vec![("10 commandments", 302)]);
        assert!(
            t.of_verse(VerseId::parse("Genesis 1:1").unwrap())
                .is_empty()
        );
    }
}
