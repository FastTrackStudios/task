//! Parse `compare` blocks out of markdown.
//!
//! A note declares a translation comparison with a fenced code block
//! tagged `compare`:
//!
//! ````text
//! ```compare
//! Genesis 4:3-Exodus 15:17
//! WEB, BSB
//! ```
//! ````
//!
//! First line is the reference (single verse, or a range up to
//! cross-book); the second line lists the translations to compare
//! (comma- or whitespace-separated). Omit the translation line to
//! compare every installed edition. A one-line form also works:
//! `John 3:16 | WEB, BSB`.
//!
//! This module only *finds and parses* the spec; resolving it to text is
//! `Store::compare`, and rendering is the consumer's job.

use std::sync::LazyLock;

use regex::Regex;
use scripture_proto::VerseRange;

/// A parsed `compare` block: what to compare, and across which editions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompareSpec {
    pub range: VerseRange,
    /// Requested translation ids (uppercased), in order. Empty ⇒ all
    /// installed editions.
    pub translations: Vec<String>,
}

impl CompareSpec {
    /// Parse the body (inside the fences) of a `compare` block.
    pub fn parse(body: &str) -> Result<Self, scripture_proto::RefError> {
        let mut ref_str: Option<String> = None;
        let mut tx_raw = String::new();
        for line in body.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if ref_str.is_none() {
                // Allow `ref | translations` on one line.
                if let Some((r, t)) = line.split_once('|') {
                    ref_str = Some(r.trim().to_string());
                    tx_raw = t.trim().to_string();
                } else {
                    ref_str = Some(line.to_string());
                }
            } else {
                if !tx_raw.is_empty() {
                    tx_raw.push(',');
                }
                tx_raw.push_str(line);
            }
        }
        let range = VerseRange::parse(ref_str.ok_or(scripture_proto::RefError::Empty)?.trim())?;
        let mut translations = Vec::new();
        for tok in tx_raw.split([',', ' ', '\t']).filter(|s| !s.is_empty()) {
            let up = tok.to_ascii_uppercase();
            if !translations.contains(&up) {
                translations.push(up);
            }
        }
        Ok(Self {
            range,
            translations,
        })
    }
}

/// Every `compare` block in `markdown`, in document order. Malformed
/// blocks (bad reference) are skipped.
#[must_use]
pub fn extract_compare_specs(markdown: &str) -> Vec<CompareSpec> {
    // ```compare … ``` — capture the body between the fences.
    static BLOCK: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?ms)^```compare[ \t]*\r?\n(.*?)^```").unwrap());
    BLOCK
        .captures_iter(markdown)
        .filter_map(|c| CompareSpec::parse(&c[1]).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_two_line_form() {
        let spec = CompareSpec::parse("John 3:16\nWEB, BSB, ESV").unwrap();
        assert_eq!(spec.range, VerseRange::parse("John 3:16").unwrap());
        assert_eq!(spec.translations, ["WEB", "BSB", "ESV"]);
    }

    #[test]
    fn parses_one_line_and_ranges_and_default_translations() {
        let spec = CompareSpec::parse("Genesis 4:3-Exodus 15:17 | web bsb").unwrap();
        assert_eq!(
            spec.range,
            VerseRange::parse("Genesis 4:3-Exodus 15:17").unwrap()
        );
        assert_eq!(spec.translations, ["WEB", "BSB"]);

        // No translation line ⇒ empty (means "all installed").
        let spec = CompareSpec::parse("John 3:16").unwrap();
        assert!(spec.translations.is_empty());
    }

    #[test]
    fn extracts_blocks_from_markdown() {
        let md = "\
Intro text.

```compare
John 3:16
WEB, BSB
```

More text, and another:

```compare
Romans 5:8 | WEB
```
";
        let specs = extract_compare_specs(md);
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].range, VerseRange::parse("John 3:16").unwrap());
        assert_eq!(specs[1].translations, ["WEB"]);
    }
}
