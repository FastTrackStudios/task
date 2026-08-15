//! Find verse references inside markdown — the link side of the
//! vault↔verse graph.
//!
//! A vault note links a verse with an ordinary Obsidian wikilink whose
//! target happens to be a scripture reference: `[[John 3:16]]` (or the
//! OSIS form `[[John.3.16]]`, or with an alias `[[John 3:16|the verse]]`).
//! We scan for `[[…]]` spans and keep the ones whose target parses as a
//! [`VerseId`]. Everything else is a normal page link and ignored — so
//! scripture links and page links coexist with no special syntax.
//!
//! This is intentionally decoupled from `vault-obsidian`'s full ref
//! parser; it only needs the wikilink shape. (A future pass could adopt
//! the canonical extractor for exact parity with embeds / block refs.)

use std::sync::LazyLock;

use regex::Regex;
use scripture_proto::{ScriptureRef, VerseRange};

/// One verse reference found in a note, with the line it sits on.
///
/// `range` is a single verse (`[[John 3:16]]`) or a span
/// (`[[John 3:16-20]]`, `[[Genesis 4:3-Exodus 15:17]]`); `translation`
/// is the optional `@ESV` qualifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerseRefHit {
    pub range: VerseRange,
    /// Optional `@`-qualified edition (`ESV`), uppercased.
    pub translation: Option<String>,
    /// The whole line the link appears on, trimmed — the surrounding
    /// "inline thesis" that makes a backlink meaningful.
    pub excerpt: String,
}

/// Every verse reference (single or range) in `markdown`, in document
/// order. Duplicates are kept — callers dedupe per note if they want.
#[must_use]
pub fn extract_verse_refs(markdown: &str) -> Vec<VerseRefHit> {
    // `[[ target ]]` — capture the inner target, not greedy across `]]`.
    static WIKILINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[\[([^\]]+)\]\]").unwrap());

    let mut hits = Vec::new();
    for line in markdown.lines() {
        for cap in WIKILINK.captures_iter(line) {
            let inner = &cap[1];
            // Drop an alias (`John 3:16|label`); a verse target has no
            // heading/block anchor, so `#`/`^` forms simply won't parse.
            let target = inner.split('|').next().unwrap_or(inner).trim();
            if let Ok(sref) = ScriptureRef::parse(target) {
                hits.push(VerseRefHit {
                    range: sref.range,
                    translation: sref.translation,
                    excerpt: line.trim().to_string(),
                });
            }
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(s: &str) -> VerseRange {
        VerseRange::parse(s).unwrap()
    }

    #[test]
    fn finds_verse_links_and_ignores_page_links() {
        let md = "\
# Notes on grace
Paul argues from [[John 3:16]] that love precedes the gift.
See also [[Romans 5:8|Paul on the cross]] and my [[Project Plan]] note.
The OSIS form [[John.1.1]] works too.
Nothing here.";
        let ranges: Vec<_> = extract_verse_refs(md)
            .into_iter()
            .map(|h| h.range)
            .collect();
        assert_eq!(ranges, vec![r("John 3:16"), r("Romans 5:8"), r("John 1:1")]);
        assert!(
            extract_verse_refs(md)[0]
                .excerpt
                .contains("love precedes the gift")
        );
    }

    #[test]
    fn captures_ranges() {
        let md = "Span [[John 3:16-20]], cross-chapter [[John 3:20-7:26]], \
                  and cross-book [[Genesis 4:3-Exodus 15:17]].";
        let ranges: Vec<_> = extract_verse_refs(md)
            .into_iter()
            .map(|h| h.range)
            .collect();
        assert_eq!(
            ranges,
            vec![
                r("John 3:16-20"),
                r("John 3:20-7:26"),
                r("Genesis 4:3-Exodus 15:17")
            ]
        );
    }

    #[test]
    fn two_verses_on_one_line_both_captured() {
        let hits = extract_verse_refs("Compare [[Genesis 1:1]] with [[John 1:1]].");
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].range, r("Genesis 1:1"));
        assert_eq!(hits[1].range, r("John 1:1"));
    }

    #[test]
    fn non_verse_wikilinks_are_skipped() {
        assert!(
            extract_verse_refs("[[Daily Note]] [[John the Baptist]] [[Acts of Service]]")
                .is_empty()
        );
    }
}
