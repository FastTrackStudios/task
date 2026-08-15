//! Heading outline of a single page. Mirrors Obsidian's `outline` CLI.

use crate::vault::VaultPage;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Heading {
    /// 1..=6
    pub level: u8,
    /// Heading text, no leading `#`s.
    pub text: String,
    /// 1-based line number in the raw file.
    pub line: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PageOutline {
    pub headings: Vec<Heading>,
}

/// Build an outline for one page.
///
/// Walks `page.parsed.blocks` for `kind="heading"` entries; reconstructs
/// line numbers by scanning `page.raw` for each heading occurrence in
/// order, so multiple headings with the same text get distinct lines.
#[must_use]
pub fn outline(page: &VaultPage) -> PageOutline {
    // Pre-collect (line_no, level, text) candidates from the raw file
    // so we can match parsed headings to a real source line. We scan
    // for ATX headings (`#` ... `######` followed by space). Setext
    // headings are uncommon in Obsidian; the parser still emits them
    // as kind="heading", and they'll fall through to a best-effort
    // text match below.
    let candidates: Vec<(usize, u8, String)> = page
        .raw
        .split('\n')
        .enumerate()
        .filter_map(|(idx, line)| parse_atx(line).map(|(lvl, txt)| (idx + 1, lvl, txt)))
        .collect();

    // Walking cursor so duplicates resolve in document order.
    let mut cursor = 0usize;
    let mut headings = Vec::new();

    for block in &page.parsed.blocks {
        if block.kind != "heading" {
            continue;
        }
        let level = block
            .heading_level
            .and_then(|n| {
                if (1..=6).contains(&n) {
                    Some(n as u8)
                } else {
                    None
                }
            })
            .unwrap_or(1);
        let text = block.content.trim().to_string();

        // Find the next candidate whose level+text matches. Fall back
        // to text-only, then to bumping the cursor and using whatever
        // line we have, so we always emit *something* sensible.
        let mut found_line = None;
        // Need `i` to advance the cursor on match — natural
        // shape is the index loop.
        #[allow(clippy::needless_range_loop)]
        for i in cursor..candidates.len() {
            let (ln, lvl, ctxt) = &candidates[i];
            if *lvl == level && ctxt.trim() == text {
                found_line = Some(*ln);
                cursor = i + 1;
                break;
            }
        }
        if found_line.is_none() {
            #[allow(clippy::needless_range_loop)]
            for i in cursor..candidates.len() {
                let (ln, _, ctxt) = &candidates[i];
                if ctxt.trim() == text {
                    found_line = Some(*ln);
                    cursor = i + 1;
                    break;
                }
            }
        }

        headings.push(Heading {
            level,
            text,
            line: found_line.unwrap_or(0),
        });
    }

    PageOutline { headings }
}

/// Parse a single line as an ATX heading. Returns `(level, text)` or
/// `None`. Allows up to three leading spaces per `CommonMark`; strips
/// any trailing `#`s and surrounding whitespace.
fn parse_atx(line: &str) -> Option<(u8, String)> {
    let trimmed = line.trim_start_matches([' ', '\t']);
    // Limit to 3 leading spaces per CommonMark.
    let leading = line.len() - trimmed.len();
    if leading > 3 {
        return None;
    }
    let mut hashes = 0u8;
    for c in trimmed.chars() {
        if c == '#' {
            hashes += 1;
            if hashes > 6 {
                return None;
            }
        } else {
            break;
        }
    }
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &trimmed[hashes as usize..];
    // Must be followed by a space or end-of-line (else it's `#tag`).
    if !(rest.is_empty() || rest.starts_with(' ') || rest.starts_with('\t')) {
        return None;
    }
    let text = rest.trim().trim_end_matches('#').trim_end().to_string();
    Some((hashes, text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::Vault;
    use std::fs;
    use std::path::Path;

    fn touch(p: &Path, body: &str) {
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, body).unwrap();
    }

    #[test]
    fn outline_extracts_headings_with_line_numbers() {
        let dir = tempfile::tempdir().unwrap();
        touch(
            &dir.path().join("N.md"),
            "# Top\n\nbody\n\n## Sub A\n\n## Sub B\n\n### Deep\n",
        );
        let v = Vault::open(dir.path()).unwrap();
        let p = v.page("N.md").unwrap();
        let o = outline(p);
        assert_eq!(o.headings.len(), 4);
        assert_eq!(
            o.headings[0],
            Heading {
                level: 1,
                text: "Top".into(),
                line: 1
            }
        );
        assert_eq!(
            o.headings[1],
            Heading {
                level: 2,
                text: "Sub A".into(),
                line: 5
            }
        );
        assert_eq!(
            o.headings[2],
            Heading {
                level: 2,
                text: "Sub B".into(),
                line: 7
            }
        );
        assert_eq!(
            o.headings[3],
            Heading {
                level: 3,
                text: "Deep".into(),
                line: 9
            }
        );
    }

    #[test]
    fn duplicate_headings_get_distinct_lines() {
        let dir = tempfile::tempdir().unwrap();
        touch(
            &dir.path().join("D.md"),
            "## Same\n\ntext\n\n## Same\n\nmore\n\n## Same\n",
        );
        let v = Vault::open(dir.path()).unwrap();
        let p = v.page("D.md").unwrap();
        let o = outline(p);
        assert_eq!(o.headings.len(), 3);
        assert_eq!(o.headings[0].line, 1);
        assert_eq!(o.headings[1].line, 5);
        assert_eq!(o.headings[2].line, 9);
        assert!(o.headings.iter().all(|h| h.text == "Same"));
    }

    #[test]
    fn no_headings_yields_empty_outline() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("P.md"), "just a paragraph\n\n- bullet\n");
        let v = Vault::open(dir.path()).unwrap();
        let p = v.page("P.md").unwrap();
        let o = outline(p);
        assert!(o.headings.is_empty());
    }

    #[test]
    fn outline_ignores_inline_tag_lookalikes() {
        let dir = tempfile::tempdir().unwrap();
        // `#tag` at start of line is not a heading (no space after #).
        touch(&dir.path().join("T.md"), "#tag not heading\n\n# Real\n");
        let v = Vault::open(dir.path()).unwrap();
        let p = v.page("T.md").unwrap();
        let o = outline(p);
        assert_eq!(o.headings.len(), 1);
        assert_eq!(o.headings[0].text, "Real");
        assert_eq!(o.headings[0].line, 3);
    }
}
