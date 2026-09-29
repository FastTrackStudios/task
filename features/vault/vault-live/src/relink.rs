//! Rewrite the wikilinks that named a page, after it was renamed.
//!
//! A wikilink names a page by its basename (`[[Ionian]]`) or by its path
//! without the extension (`[[Concepts/Ionian]]`), optionally with a
//! heading, a block, an alias, or as an embed (`![[…]]`). When a page's
//! name changes, every one of those that named the old page should name
//! the new one, and nothing else about the link should change: the
//! heading, the alias and the spelling of the rest of the line stay
//! byte-for-byte.
//!
//! Left alone:
//! - a qualified reference (`[[acme.test/music-theory::Page@…]]`) — it
//!   names a page of *another* wiki by identity, not by this vault's
//!   filenames (ADR 0002), and is not this vault's to rewrite;
//! - anything inside a fenced code block or an inline code span, where
//!   `[[` is text;
//! - links to other pages, however similar.

/// The page a link might name: its basename and its vault path, both
/// without the `.md` extension.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageName {
    basename: String,
    stem_path: String,
}

impl PageName {
    /// From a vault-relative path (`Concepts/Ionian.md`).
    #[must_use]
    pub fn of(path: &str) -> Self {
        let stem_path = path.strip_suffix(".md").unwrap_or(path).to_owned();
        let basename = stem_path
            .rsplit('/')
            .next()
            .unwrap_or(&stem_path)
            .to_owned();
        Self {
            basename,
            stem_path,
        }
    }

    /// Whether a link target names this page.
    fn named_by(&self, target: &str) -> bool {
        let t = target.trim();
        let t = t.strip_suffix(".md").unwrap_or(t);
        t.eq_ignore_ascii_case(&self.basename) || t.eq_ignore_ascii_case(&self.stem_path)
    }

    /// What a link that named `old` by `target` should say to name `self`:
    /// by path when it named the old one by path, else by basename.
    fn spelled_like(&self, old: &PageName, target: &str) -> String {
        let t = target.trim();
        let t = t.strip_suffix(".md").unwrap_or(t);
        if t.contains('/') && t.eq_ignore_ascii_case(&old.stem_path) {
            self.stem_path.clone()
        } else {
            self.basename.clone()
        }
    }
}

/// `text` with every wikilink that named `old` naming `new` instead, or
/// `None` when no link named it (so a caller writes nothing).
#[must_use]
pub fn relink(text: &str, old: &PageName, new: &PageName) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut changed = false;
    let mut in_fence = false;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            out.push_str(line);
            continue;
        }
        if in_fence {
            out.push_str(line);
            continue;
        }
        match relink_line(line, old, new) {
            Some(rewritten) => {
                changed = true;
                out.push_str(&rewritten);
            }
            None => out.push_str(line),
        }
    }
    // A link by basename to a page that only changed folders reads the
    // same afterwards: that is not a rewrite.
    (changed && out != text).then_some(out)
}

fn relink_line(line: &str, old: &PageName, new: &PageName) -> Option<String> {
    let mut out = String::with_capacity(line.len());
    let mut changed = false;
    let mut rest = line;
    let mut in_code = false;
    while !rest.is_empty() {
        // Inline code spans: copy through untouched.
        if rest.starts_with('`') {
            in_code = !in_code;
            out.push('`');
            rest = &rest[1..];
            continue;
        }
        if !in_code && rest.starts_with("[[") {
            if let Some(end) = rest[2..].find("]]") {
                let inner = &rest[2..2 + end];
                // Target is up to the first `#` or `|`.
                let cut = inner.find(['#', '|']).unwrap_or(inner.len());
                let (target, tail) = inner.split_at(cut);
                if !target.contains("::") && old.named_by(target) {
                    out.push_str("[[");
                    out.push_str(&new.spelled_like(old, target));
                    out.push_str(tail);
                    out.push_str("]]");
                    changed = true;
                } else {
                    out.push_str(&rest[..2 + end + 2]);
                }
                rest = &rest[2 + end + 2..];
                continue;
            }
        }
        let ch = rest.chars().next().expect("non-empty");
        out.push(ch);
        rest = &rest[ch.len_utf8()..];
    }
    changed.then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn go(text: &str) -> Option<String> {
        relink(
            text,
            &PageName::of("Concepts/Ionian.md"),
            &PageName::of("Concepts/Major Scale.md"),
        )
    }

    #[test]
    fn every_spelling_of_a_link_to_the_page_follows_it() {
        assert_eq!(
            go("See [[Ionian]], [[ionian|the major mode]], [[Ionian#In the room]] and ![[Ionian]].\n")
                .as_deref(),
            Some(
                "See [[Major Scale]], [[Major Scale|the major mode]], [[Major Scale#In the room]] and ![[Major Scale]].\n"
            )
        );
    }

    #[test]
    fn a_link_by_path_stays_a_link_by_path() {
        assert_eq!(
            go("[[Concepts/Ionian]] and [[Concepts/Ionian.md]]").as_deref(),
            Some("[[Concepts/Major Scale]] and [[Concepts/Major Scale]]")
        );
    }

    #[test]
    fn other_pages_code_and_qualified_references_are_left_alone() {
        let text = "[[Ionian mode]] `[[Ionian]]`\n```\n[[Ionian]]\n```\n[[acme.test/music-theory::Ionian@2026-09-01]]\n";
        assert_eq!(go(text), None);
    }

    #[test]
    fn a_folder_move_rewrites_links_by_path_and_only_those() {
        let moved = |t: &str| {
            relink(
                t,
                &PageName::of("Concepts/Ionian.md"),
                &PageName::of("Scales/Ionian.md"),
            )
        };
        assert_eq!(moved("[[Ionian]]"), None);
        assert_eq!(
            moved("[[Concepts/Ionian#Hearing it]]").as_deref(),
            Some("[[Scales/Ionian#Hearing it]]")
        );
    }

    #[test]
    fn nothing_to_rewrite_is_none() {
        assert_eq!(go("no links here\n"), None);
    }
}
