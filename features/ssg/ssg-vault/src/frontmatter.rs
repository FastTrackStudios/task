//! The `---` block at the top of a note.
//!
//! Deliberately not a YAML parser. Every FTS vault uses the same handful
//! of scalar keys (`title`, `order`, `stage`, `type`, `summary`/`blurb`)
//! plus the occasional inline list of tags, and pulling a YAML crate into
//! a build dependency to read `title: Chords` would cost every consuming
//! workspace a parser it never exercises. What is supported is what the
//! vaults contain; anything structured is left in the raw text, where a
//! caller that needs it can still find it.

use std::collections::BTreeMap;

/// A note's parsed frontmatter.
///
/// `BTreeMap` rather than a hash map so iteration order is stable —
/// codegen output has to be byte-identical between builds or every
/// rebuild churns the generated file and invalidates the cache.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Frontmatter {
    fields: BTreeMap<String, String>,
    /// The block verbatim, without its fences.
    raw: String,
}

impl Frontmatter {
    /// Split a note into its frontmatter and its body.
    ///
    /// A note without a leading `---` fence has empty frontmatter and is
    /// returned whole — that is a valid note, not an error.
    #[must_use]
    pub fn split(note: &str) -> (Self, &str) {
        let Some(rest) = note.strip_prefix("---\n") else {
            return (Self::default(), note);
        };
        let Some((block, body)) = rest.split_once("\n---") else {
            // An opening fence with no closing one: treat the whole file
            // as body rather than swallowing it as metadata. A note that
            // renders wrong is fixable; a note that renders as nothing
            // looks like the build lost it.
            return (Self::default(), note);
        };

        let mut fields = BTreeMap::new();
        for line in block.lines() {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            // Indented lines belong to a structured value we do not
            // parse; taking them as top-level keys would invent fields.
            if key.starts_with([' ', '\t', '-']) {
                continue;
            }
            let value = unquote(value.trim());
            if value.is_empty() {
                continue;
            }
            fields.insert(key.trim().to_owned(), value.to_owned());
        }

        let frontmatter = Self {
            fields,
            raw: block.to_owned(),
        };
        (frontmatter, body.trim_start_matches('\n'))
    }

    /// One scalar field.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields.get(key).map(String::as_str)
    }

    /// The first of `keys` that is present.
    ///
    /// Vaults disagree about the name of the one-line description —
    /// Signal writes `summary:`, Ignition writes `blurb:` — and neither
    /// is worth a migration, so a caller names both.
    #[must_use]
    pub fn any(&self, keys: &[&str]) -> Option<&str> {
        keys.iter().find_map(|k| self.get(k))
    }

    /// A field parsed as an integer, if it is one.
    #[must_use]
    pub fn number(&self, key: &str) -> Option<u32> {
        self.get(key).and_then(|v| v.parse().ok())
    }

    /// An inline `[a, b]` or comma-separated list.
    #[must_use]
    pub fn list(&self, key: &str) -> Vec<String> {
        let Some(raw) = self.get(key) else {
            return Vec::new();
        };
        raw.trim_start_matches('[')
            .trim_end_matches(']')
            .split(',')
            .map(|item| unquote(item.trim()).to_owned())
            .filter(|item| !item.is_empty())
            .collect()
    }

    /// The block verbatim, without its `---` fences.
    #[must_use]
    pub fn raw(&self) -> &str {
        &self.raw
    }

    /// Whether the note carried a frontmatter block at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }
}

/// Strip one layer of matching quotes.
///
/// Matching, not any: `"it's"` keeps its apostrophe, and a value that
/// opens with a quote it never closes is left alone rather than
/// half-eaten.
fn unquote(value: &str) -> &str {
    for quote in ['"', '\''] {
        if let Some(inner) = value
            .strip_prefix(quote)
            .and_then(|v| v.strip_suffix(quote))
        {
            return inner;
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_frontmatter_from_body() {
        let (fm, body) = Frontmatter::split("---\ntitle: Chords\norder: 3\n---\n\n# Chords\n");
        assert_eq!(fm.get("title"), Some("Chords"));
        assert_eq!(fm.number("order"), Some(3));
        assert_eq!(body, "# Chords\n");
    }

    #[test]
    fn a_note_without_frontmatter_is_all_body() {
        let (fm, body) = Frontmatter::split("# Chords\n");
        assert!(fm.is_empty());
        assert_eq!(body, "# Chords\n");
    }

    #[test]
    fn an_unclosed_fence_keeps_the_whole_note_as_body() {
        let note = "---\ntitle: Chords\n\n# Chords\n";
        let (fm, body) = Frontmatter::split(note);
        assert!(fm.is_empty());
        assert_eq!(body, note);
    }

    #[test]
    fn quotes_are_stripped_only_in_matching_pairs() {
        let (fm, _) = Frontmatter::split("---\na: \"Chords\"\nb: it's\nc: \"unclosed\n---\n");
        assert_eq!(fm.get("a"), Some("Chords"));
        assert_eq!(fm.get("b"), Some("it's"));
        assert_eq!(fm.get("c"), Some("\"unclosed"));
    }

    #[test]
    fn indented_lines_do_not_become_fields() {
        let (fm, _) = Frontmatter::split("---\ntags:\n  - one: two\ntitle: Chords\n---\n");
        assert_eq!(fm.get("one"), None);
        assert_eq!(fm.get("title"), Some("Chords"));
    }

    #[test]
    fn reads_an_inline_list() {
        let (fm, _) = Frontmatter::split("---\ntags: [chords, \"key changes\"]\n---\n");
        assert_eq!(fm.list("tags"), vec!["chords", "key changes"]);
    }

    #[test]
    fn falls_back_across_alternative_keys() {
        let (fm, _) = Frontmatter::split("---\nblurb: Where cues come from\n---\n");
        assert_eq!(fm.any(&["summary", "blurb"]), Some("Where cues come from"));
    }
}
