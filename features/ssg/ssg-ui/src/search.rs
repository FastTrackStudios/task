//! Search across the vault.
//!
//! There is no index to build and nothing to fetch. Every page is
//! already `&'static` in the binary — that is what `ssg-build` produced
//! — so search is a scan over data the page is holding anyway. A vault
//! of a few dozen notes is a few hundred kilobytes of text, and matching
//! against it takes less time than a network round trip to ask someone
//! else.
//!
//! That is worth stating because the usual shape of this feature is a
//! JSON index emitted at build time and fetched on first keystroke, and
//! it would be strictly worse here: another artefact to keep in step,
//! another request, and a first search that is slower than every
//! subsequent one.
//!
//! ## Hydration
//!
//! The component has state, so it does not render *identically* forever
//! — but its first render does, because an empty query has exactly one
//! rendering. That is all hydration requires: the server draws the
//! empty box, the client adopts it, and the first keystroke is the first
//! thing that differs.

use dioxus::prelude::*;
use ssg_vault::{StaticPage, StaticVault};

/// One hit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    /// The page matched.
    pub page: &'static StaticPage,
    /// The heading the match sits under, if it was found in the body
    /// rather than in the title. Gives the result a `#fragment` to land
    /// on, so a hit in a long note opens at the right section.
    pub heading: Option<&'static ssg_vault::StaticHeading>,
    /// A window of the text around the match.
    pub excerpt: String,
    /// Higher sorts first.
    score: u32,
}

/// Search `vault` for `query`.
///
/// Pure, and public because a site may want its own results UI — the
/// scoring is the part worth sharing.
///
/// Scoring is deliberately crude, in the order a reader expects: a title
/// match beats a heading match beats a body match. Within a kind, the
/// vault's reading order breaks ties, so "the earliest chapter that
/// mentions this" wins — which for a guide is almost always the one that
/// introduces it.
#[must_use]
pub fn search(vault: StaticVault, query: &str, limit: usize) -> Vec<Hit> {
    let needle = query.trim().to_lowercase();
    if needle.len() < 2 {
        // One letter matches most of a vault, which is not a search
        // result, it is the table of contents with extra steps.
        return Vec::new();
    }

    let mut hits: Vec<Hit> = Vec::new();

    for page in vault.pages {
        if page.title.to_lowercase().contains(&needle) {
            hits.push(Hit {
                page,
                heading: None,
                excerpt: page.summary.to_owned(),
                score: 300,
            });
            continue;
        }

        if let Some(heading) = page
            .headings
            .iter()
            .find(|h| h.text.to_lowercase().contains(&needle))
        {
            hits.push(Hit {
                page,
                heading: Some(heading),
                excerpt: heading.text.to_owned(),
                score: 200,
            });
            continue;
        }

        // The note's prose, not its rendered HTML: a match inside a tag
        // name or an attribute is not a match a reader would recognise.
        let body = page.body.to_lowercase();
        if let Some(at) = body.find(&needle) {
            hits.push(Hit {
                page,
                heading: heading_before(page, at),
                excerpt: excerpt(page.body, at, needle.len()),
                score: 100,
            });
        }
    }

    hits.sort_by(|a, b| b.score.cmp(&a.score));
    hits.truncate(limit);
    hits
}

/// The last heading at or before a byte offset in the body.
///
/// Found by searching the body for each heading's text rather than by
/// tracking offsets through the render: the headings were collected from
/// the *rendered* stream, where offsets no longer line up with the
/// markdown. Approximate, and good enough to say "this hit is under
/// that heading".
fn heading_before(
    page: &'static StaticPage,
    at: usize,
) -> Option<&'static ssg_vault::StaticHeading> {
    let body = page.body;
    page.headings
        .iter()
        .filter_map(|h| body.find(h.text).map(|pos| (pos, h)))
        .filter(|(pos, _)| *pos <= at)
        .max_by_key(|(pos, _)| *pos)
        .map(|(_, h)| h)
}

/// A window of `text` around a match, on character boundaries.
fn excerpt(text: &str, at: usize, len: usize) -> String {
    const BEFORE: usize = 60;
    const AFTER: usize = 90;

    let start = floor_boundary(text, at.saturating_sub(BEFORE));
    let end = ceil_boundary(text, (at + len + AFTER).min(text.len()));

    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    // Collapsed, because markdown's newlines and list markers make an
    // excerpt read as broken text when they survive into one line.
    out.push_str(&plain(&text[start..end]));
    if end < text.len() {
        out.push('…');
    }
    out
}

/// Markdown reduced to the words in it.
///
/// An excerpt is a hint about which result this is, and a reader
/// scanning four of them should not have to see `**bold**`,
/// `[[a-slug|the label]]` and a row of backticks to do it. This is not a
/// markdown parser and does not want to be: it strips the punctuation
/// that carries no meaning once the text is out of its document, and
/// keeps everything else.
fn plain(markdown: &str) -> String {
    let mut out = String::with_capacity(markdown.len());
    let mut chars = markdown.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            // `[[target|label]]` reads as the label; `[[target]]` as the
            // target with its dashes opened out.
            '[' if chars.peek() == Some(&'[') => {
                chars.next();
                let mut inner = String::new();
                while let Some(c) = chars.next() {
                    if c == ']' && chars.peek() == Some(&']') {
                        chars.next();
                        break;
                    }
                    inner.push(c);
                }
                let shown = inner.split_once('|').map_or(inner.as_str(), |(_, l)| l);
                out.push_str(&shown.replace('-', " "));
            }
            // Emphasis, code and heading markers carry nothing here.
            '*' | '_' | '`' | '#' | '>' => {}
            _ => out.push(ch),
        }
    }

    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn floor_boundary(text: &str, mut at: usize) -> usize {
    while at > 0 && !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

fn ceil_boundary(text: &str, mut at: usize) -> usize {
    while at < text.len() && !text.is_char_boundary(at) {
        at += 1;
    }
    at
}

#[cfg(test)]
mod tests {
    use super::*;
    use ssg_vault::StaticHeading;

    static HEADINGS: &[StaticHeading] = &[StaticHeading {
        level: 2,
        text: "The song map",
        id: "the-song-map",
    }];

    const fn page(
        slug: &'static str,
        title: &'static str,
        body: &'static str,
        headings: &'static [StaticHeading],
    ) -> StaticPage {
        StaticPage {
            slug,
            title,
            summary: "",
            order: 0,
            stage: "",
            kind: "other",
            source: "",
            body,
            html: "",
            links: &[],
            headings,
            tags: &[],
            words: 0,
            updated: "",
        }
    }

    static PAGES: &[StaticPage] = &[
        page("tempo", "Tempo", "how fast", &[]),
        page(
            "the-song",
            "The song",
            "## The song map\n\nA song arrives as a tempo map.",
            HEADINGS,
        ),
        page("rhythm", "Rhythm", "nothing relevant here", &[]),
    ];

    fn vault() -> StaticVault {
        StaticVault::new(PAGES)
    }

    #[test]
    fn a_title_match_outranks_a_body_match() {
        let hits = search(vault(), "tempo", 8);
        assert_eq!(
            hits[0].page.slug, "tempo",
            "the page NAMED tempo comes first"
        );
        assert_eq!(hits[1].page.slug, "the-song");
    }

    #[test]
    fn a_body_hit_points_at_the_heading_it_sits_under() {
        let hits = search(vault(), "arrives", 8);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].heading.expect("a heading").id, "the-song-map");
    }

    #[test]
    fn matching_ignores_case() {
        assert_eq!(search(vault(), "TEMPO", 8).len(), 2);
    }

    #[test]
    fn one_letter_is_not_a_search() {
        // It would match most of the vault, which is the table of
        // contents with extra steps.
        assert!(search(vault(), "t", 8).is_empty());
        assert!(search(vault(), "  ", 8).is_empty());
    }

    #[test]
    fn the_limit_is_honoured() {
        assert_eq!(search(vault(), "tempo", 1).len(), 1);
    }

    #[test]
    fn an_excerpt_is_one_line_and_marked_where_it_was_cut() {
        let hits = search(vault(), "arrives", 8);
        let excerpt = &hits[0].excerpt;
        assert!(!excerpt.contains('\n'), "an excerpt must read as one line");
        assert!(excerpt.contains("arrives"));
    }

    #[test]
    fn an_excerpt_reads_as_prose_not_as_markdown() {
        static MD: &[StaticPage] = &[page(
            "m",
            "M",
            "The **song map** is a `chart` — see [[the-song|the song]] and [[key-changes]].",
            &[],
        )];
        let hits = search(StaticVault::new(MD), "chart", 8);
        let excerpt = &hits[0].excerpt;
        assert!(excerpt.contains("song map"), "{excerpt}");
        assert!(excerpt.contains("the song"), "an alias shows its label");
        assert!(excerpt.contains("key changes"), "a bare target opens out");
        for noise in ['*', '`', '[', ']', '|'] {
            assert!(!excerpt.contains(noise), "`{noise}` survived: {excerpt}");
        }
    }

    #[test]
    fn a_multibyte_body_does_not_split_a_character() {
        static WIDE: &[StaticPage] = &[page("w", "W", "— a dash — and a match here —", &[])];
        // Would panic on a non-boundary slice.
        let hits = search(StaticVault::new(WIDE), "match", 8);
        assert_eq!(hits.len(), 1);
    }
}

/// A search box and its results.
#[component]
pub fn VaultSearch(
    /// The vault to search.
    vault: StaticVault,
    /// URL prefix the vault is published under.
    base: &'static str,
    /// Most results to show.
    #[props(default = 8)]
    limit: usize,
    /// Placeholder in the empty box.
    #[props(default = "Search the guide")]
    placeholder: &'static str,
    /// Class for the wrapping element.
    #[props(default = "ssg-search")]
    class: &'static str,
) -> Element {
    let mut query = use_signal(String::new);
    let hits = use_memo(move || search(vault, &query.read(), limit));

    rsx! {
        div { class,
            input {
                r#type: "search",
                class: "ssg-search-input",
                placeholder,
                value: "{query}",
                autocomplete: "off",
                // `spellcheck` off: a search box is full of proper nouns
                // and identifiers, and a red squiggle under every one is
                // noise.
                spellcheck: "false",
                oninput: move |e| query.set(e.value()),
                onkeydown: move |e| {
                    if e.key() == Key::Escape {
                        query.set(String::new());
                    }
                },
            }

            // Nothing at all until there is a query: an empty results
            // panel below the box is a permanent piece of furniture that
            // says nothing.
            if !query.read().trim().is_empty() {
                if hits.read().is_empty() {
                    p { class: "ssg-search-empty", "No matches." }
                } else {
                    ul { class: "ssg-search-results",
                        for hit in hits.read().iter() {
                            li { key: "{hit.page.slug}-{hit.heading.map_or(\"\", |h| h.id)}",
                                a {
                                    href: match hit.heading {
                                        Some(h) => format!("{base}/{}#{}", hit.page.slug, h.id),
                                        None => format!("{base}/{}", hit.page.slug),
                                    },
                                    span { class: "ssg-search-title", "{hit.page.title}" }
                                    if let Some(heading) = hit.heading {
                                        span { class: "ssg-search-section", " › {heading.text}" }
                                    }
                                    if !hit.excerpt.is_empty() {
                                        span { class: "ssg-search-excerpt", "{hit.excerpt}" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
