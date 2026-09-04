//! `[[target|alias]]` — the vault's own cross-reference syntax.
//!
//! Wikilinks are found on the raw markdown before it reaches the parser
//! rather than after, because `[[x]]` is not markdown: pulldown-cmark
//! sees a literal `[` followed by a link that has no destination, and
//! what comes out the far side is unrecoverable. Rewriting first means
//! the parser only ever sees ordinary links, and a wikilink inside a
//! code fence — where it is text, not a link — is skipped by the same
//! pass that finds the fences.

/// One `[[target|alias]]` occurrence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wikilink {
    /// Byte range of the whole `[[…]]` in the source.
    pub span: (usize, usize),
    /// The link target — a page slug, before resolution.
    pub target: String,
    /// The text to display. Equals `target` when no `|alias` was given.
    pub alias: String,
}

/// Every wikilink in `markdown`, in document order, skipping fenced and
/// inline code.
#[must_use]
pub fn wikilinks(markdown: &str) -> Vec<Wikilink> {
    let bytes = markdown.as_bytes();
    let mut links = Vec::new();
    let mut i = 0;

    while i < bytes.len() {
        // Code spans and fences are verbatim text. A guide that
        // *documents* wikilink syntax has `[[like this]]` inside a
        // fence, and turning that into a link would be a lie about
        // what the reader typed.
        if let Some(end) = skip_code(markdown, i) {
            i = end;
            continue;
        }

        if bytes[i] == b'[' && bytes.get(i + 1) == Some(&b'[') {
            if let Some(close) = markdown[i + 2..].find("]]") {
                let inner = &markdown[i + 2..i + 2 + close];
                // A newline inside the brackets means they were never a
                // link — an unclosed `[[` earlier in the paragraph, most
                // likely. Bail on this one and keep scanning.
                if !inner.contains('\n') {
                    let (target, alias) = match inner.split_once('|') {
                        Some((t, a)) => (t.trim(), a.trim()),
                        None => (inner.trim(), inner.trim()),
                    };
                    if !target.is_empty() {
                        links.push(Wikilink {
                            span: (i, i + 4 + close),
                            target: target.to_owned(),
                            alias: alias.to_owned(),
                        });
                    }
                    i += 4 + close;
                    continue;
                }
            }
        }

        // Advance one *character*, not one byte — a multi-byte char in
        // the prose would otherwise land the next read mid-sequence and
        // panic on the slice.
        i += utf8_len(bytes[i]);
    }

    links
}

/// If a code span or fence starts at `at`, the byte index just past its
/// end. `None` when `at` is not the start of one.
//
// t[impl ssg.render.code-verbatim]
fn skip_code(markdown: &str, at: usize) -> Option<usize> {
    let bytes = markdown.as_bytes();
    if bytes[at] != b'`' {
        return None;
    }

    // The opening run length is the closing run length: ``` closes ```,
    // and ` closes `. That is markdown's rule for both fences and spans,
    // so one branch handles both.
    let ticks = bytes[at..].iter().take_while(|&&b| b == b'`').count();
    let fence = &markdown[at..at + ticks];
    let after = at + ticks;

    match markdown[after..].find(fence) {
        Some(offset) => Some(after + offset + ticks),
        // Unterminated: the rest of the note is inside it. Consuming to
        // the end matches how a markdown renderer treats it.
        None => Some(markdown.len()),
    }
}

/// Byte length of the UTF-8 sequence starting with `first`.
const fn utf8_len(first: u8) -> usize {
    match first {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn targets(md: &str) -> Vec<String> {
        wikilinks(md).into_iter().map(|l| l.target).collect()
    }

    #[test]
    fn finds_plain_and_aliased_links() {
        let links = wikilinks("see [[chords]] and [[rhythm|the rhythm page]]");
        assert_eq!(links[0].target, "chords");
        assert_eq!(links[0].alias, "chords");
        assert_eq!(links[1].target, "rhythm");
        assert_eq!(links[1].alias, "the rhythm page");
    }

    #[test]
    fn spans_cover_the_whole_bracket_pair() {
        let md = "a [[chords]] b";
        let link = &wikilinks(md)[0];
        assert_eq!(&md[link.span.0..link.span.1], "[[chords]]");
    }

    #[test]
    fn skips_inline_code() {
        assert!(targets("write `[[chords]]` to link").is_empty());
    }

    #[test]
    fn skips_fenced_code() {
        assert!(targets("```\n[[chords]]\n```\n").is_empty());
    }

    #[test]
    fn finds_links_after_a_fence() {
        assert_eq!(targets("```\ncode\n```\n\n[[chords]]"), vec!["chords"]);
    }

    #[test]
    fn ignores_brackets_spanning_a_newline() {
        assert!(targets("[[chords\nrhythm]]").is_empty());
    }

    #[test]
    fn handles_multibyte_prose() {
        assert_eq!(targets("mötley — crüe [[chords]]"), vec!["chords"]);
    }

    #[test]
    fn an_unterminated_fence_swallows_the_rest() {
        assert!(targets("```\n[[chords]]").is_empty());
    }
}
