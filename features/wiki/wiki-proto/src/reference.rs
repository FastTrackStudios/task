//! Parsing and rendering a reference into another source.
//!
//! The grammar is ADR 0002's, and the shape it settles is:
//!
//! ```text
//! [[acme.test/music-theory::Ionian@2026-09-01#^partials|Ionian]]
//!   └─ domain ─┘ └─ slug ──┘  └ page ┘ └─ stamp ─┘└ anchor ┘└ display ┘
//! ```
//!
//! Everything after the page is optional, and so is the domain. Three
//! forms exist, and they mean genuinely different things:
//!
//! - **Qualified** — `domain/slug::Page`. Names exactly one source in
//!   the federation. Two orgs cannot collide on a domain, so this text
//!   means the same page in every vault that holds it.
//! - **Short** — `slug::Page`. Names a source by slug alone, resolved
//!   against the *reader's* subscription set. Convenient for the
//!   handful of sources everyone holds (scripture), and ambiguous the
//!   moment a reader subscribes to two `theory` wikis — which is
//!   reported rather than guessed (`wiki.ref.format`).
//! - **Local** — `Page`. The reader's own wiki or vault, and never
//!   anything subscribed.
//!
//! Parsing is deliberately total: anything that does not fit is a
//! [`Reference::Local`] with the whole text as its page, because the
//! alternative is an editor that refuses to render a link somebody
//! typed by hand.

use serde::{Deserialize, Serialize};

/// One parsed `[[…]]` reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reference {
    /// The publishing org's federation domain, when the reference
    /// carries one. `None` for short and local forms.
    pub domain: Option<String>,
    /// The source's slug — a wiki (`music-theory`) or a Resource
    /// (`bible`). `None` for a local reference.
    pub source: Option<String>,
    /// The page or, for a Resource, its canonical address
    /// (`John.3.16`).
    pub target: String,
    /// When the reference was made, verbatim as written. Never used to
    /// pin — see [`Self::stamp`] and `wiki.ref.stamp`.
    pub stamp: Option<String>,
    /// A block anchor (`^partials`), without the caret.
    pub anchor: Option<String>,
    /// Display text, when the writer overrode it.
    pub display: Option<String>,
}

impl Reference {
    /// Parse the inside of a `[[…]]`.
    ///
    /// Total: unparseable input becomes a local reference to the whole
    /// string rather than an error, so a hand-typed link still renders.
    #[must_use]
    pub fn parse(inner: &str) -> Self {
        let inner = inner.trim();
        // Display first: everything after the last `|` is presentation
        // and must not be searched for structure.
        let (body, display) = match inner.rsplit_once('|') {
            Some((b, d)) => (b.trim(), Some(d.trim().to_owned())),
            None => (inner, None),
        };
        // Anchor next: `#^id`. A bare `#heading` is left in the target,
        // because heading references are not anchors and this type does
        // not pretend to resolve them.
        // t[impl wiki.ref.block] — a section is referenced by its block
        // anchor, the one form an outside markdown editor also resolves,
        // so retitling or moving the section breaks nothing.
        let (body, anchor) = match body.rsplit_once("#^") {
            Some((b, a)) if !a.is_empty() => (b.trim(), Some(a.trim().to_owned())),
            _ => (body, None),
        };
        // Source, then stamp. `::` splits source from target; the stamp
        // is looked for only in the target half so a domain containing
        // an `@` cannot be mistaken for one.
        let (source_part, target_part) = match body.split_once("::") {
            Some((s, t)) => (Some(s.trim()), t.trim()),
            None => (None, body),
        };
        let (target, stamp) = match target_part.rsplit_once('@') {
            Some((t, s)) if !t.is_empty() && !s.is_empty() => {
                (t.trim().to_owned(), Some(s.trim().to_owned()))
            }
            _ => (target_part.to_owned(), None),
        };
        let (domain, source) = match source_part {
            None => (None, None),
            Some(s) => match s.split_once('/') {
                Some((d, slug)) => (Some(d.trim().to_owned()), Some(slug.trim().to_owned())),
                None => (None, Some(s.to_owned())),
            },
        };
        Self {
            domain,
            source,
            target,
            stamp,
            anchor,
            display,
        }
    }

    /// Whether this names a source outside the reader's own tree.
    #[must_use]
    pub const fn is_local(&self) -> bool {
        self.source.is_none()
    }

    /// Whether the source is named unambiguously, domain and all.
    #[must_use]
    pub const fn is_qualified(&self) -> bool {
        self.domain.is_some() && self.source.is_some()
    }

    /// What a reader sees: the writer's display text, else the target.
    #[must_use]
    pub fn label(&self) -> &str {
        self.display.as_deref().unwrap_or(&self.target)
    }

    /// Render back to the inside of a `[[…]]`.
    ///
    /// Round-trips [`Self::parse`] for every reference this type
    /// produces, which is what lets a rename rewrite one component
    /// without reflowing the rest of the line.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        if let Some(source) = &self.source {
            if let Some(domain) = &self.domain {
                out.push_str(domain);
                out.push('/');
            }
            out.push_str(source);
            out.push_str("::");
        }
        out.push_str(&self.target);
        if let Some(stamp) = &self.stamp {
            out.push('@');
            out.push_str(stamp);
        }
        if let Some(anchor) = &self.anchor {
            out.push_str("#^");
            out.push_str(anchor);
        }
        if let Some(display) = &self.display {
            out.push('|');
            out.push_str(display);
        }
        out
    }
}

/// Every `[[…]]` in a page, in the order they appear, with the byte
/// range each occupies.
///
/// The range is what a rewrite needs: replacing a reference in place
/// must not re-render the rest of the document, because a page is
/// somebody's prose and a repair should touch only the link
/// (`wiki.link.repair`).
#[must_use]
pub fn scan(markdown: &str) -> Vec<(std::ops::Range<usize>, Reference)> {
    let bytes = markdown.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'[' && bytes[i + 1] == b'[' {
            if let Some(end) = markdown[i + 2..].find("]]") {
                let inner_start = i + 2;
                let inner_end = inner_start + end;
                let inner = &markdown[inner_start..inner_end];
                // A newline inside means the `[[` was never a link.
                if !inner.contains('\n') {
                    out.push((i..inner_end + 2, Reference::parse(inner)));
                    i = inner_end + 2;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fully_qualified_reference_parses_every_part() {
        let r = Reference::parse("acme.test/music-theory::Ionian@2026-09-01#^partials|Ionian");
        assert_eq!(r.domain.as_deref(), Some("acme.test"));
        assert_eq!(r.source.as_deref(), Some("music-theory"));
        assert_eq!(r.target, "Ionian");
        assert_eq!(r.stamp.as_deref(), Some("2026-09-01"));
        assert_eq!(r.anchor.as_deref(), Some("partials"));
        assert_eq!(r.display.as_deref(), Some("Ionian"));
        assert!(r.is_qualified());
    }

    #[test]
    fn the_three_forms_are_distinguishable() {
        let local = Reference::parse("Modes");
        assert!(local.is_local() && !local.is_qualified());

        let short = Reference::parse("bible::John.3.16");
        assert!(!short.is_local() && !short.is_qualified());
        assert_eq!(short.source.as_deref(), Some("bible"));
        assert_eq!(short.target, "John.3.16");

        let qualified = Reference::parse("alice.test/cooking::Sourdough Starter");
        assert!(qualified.is_qualified());
    }

    /// A page title with spaces, a Resource address with dots, and a
    /// display alias are all ordinary. None of them may be mangled.
    #[test]
    fn awkward_targets_survive() {
        let r = Reference::parse("acme.test/music-theory::Harmonic Series@2026-09-01");
        assert_eq!(r.target, "Harmonic Series");
        assert_eq!(r.stamp.as_deref(), Some("2026-09-01"));

        // A heading reference is not an anchor, and is left alone
        // rather than half-parsed.
        let h = Reference::parse("Modes#Why a mode holds");
        assert_eq!(h.target, "Modes#Why a mode holds");
        assert!(h.anchor.is_none());
    }

    /// An email-shaped display or a stamp-free reference must not
    /// invent a stamp out of an `@` on the wrong side of the grammar.
    #[test]
    fn an_at_sign_outside_the_target_is_not_a_stamp() {
        let r = Reference::parse("Contact|write to alice@acme.test");
        assert!(r.stamp.is_none());
        assert_eq!(r.display.as_deref(), Some("write to alice@acme.test"));
        assert_eq!(r.target, "Contact");
    }

    #[test]
    fn parse_and_render_round_trip() {
        for text in [
            "Modes",
            "bible::John.3.16",
            "acme.test/music-theory::Ionian",
            "acme.test/music-theory::Ionian@2026-09-01",
            "acme.test/music-theory::Harmonic Series@2026-09-01#^partials",
            "acme.test/music-theory::Ionian@2026-09-01#^partials|Ionian",
        ] {
            assert_eq!(Reference::parse(text).render(), text, "round trip: {text}");
        }
    }

    #[test]
    fn scanning_finds_references_and_where_they_sit() {
        let md = "See [[Modes]] and \
                  [[acme.test/music-theory::Ionian@2026-09-01|Ionian]] today.";
        let found = scan(md);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].1.target, "Modes");
        assert_eq!(found[1].1.source.as_deref(), Some("music-theory"));
        // The range is exact, so a rewrite can splice.
        assert_eq!(&md[found[0].0.clone()], "[[Modes]]");
    }

    #[test]
    fn an_unclosed_bracket_is_not_a_reference() {
        assert!(scan("[[unclosed").is_empty());
        assert!(scan("[[spanning\nlines]]").is_empty());
    }
}
