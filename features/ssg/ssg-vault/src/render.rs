//! A note in, finished HTML out.
//!
//! The pipeline is three passes and one parse:
//!
//! 1. Strip the nav footer, if the vault writes one.
//! 2. Rewrite `[[wikilinks]]` into ordinary markdown links (or into a
//!    marked-up span, when the target does not exist).
//! 3. Parse once, substituting any fence a [`FenceRenderer`] claims.
//!
//! Order matters. Wikilinks are rewritten *before* the parse because
//! `[[x]]` is not markdown and the parser mangles it; fences are
//! substituted *during* the parse because that is the only place their
//! language tag and their body arrive together.

use std::collections::BTreeSet;

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd, html};

use crate::frontmatter::Frontmatter;
use crate::scan::Note;
use crate::wikilink;

/// Substitutes markup for a fenced code block.
///
/// Returns `Some(html)` to replace the fence, `None` to leave it as an
/// ordinary highlighted code block. The two arguments are the info
/// string (```` ```kf+ ```` gives `"kf+"`) and the fence body.
///
/// This is the seam that lets a site put its own rendering in a static
/// page without this crate knowing anything about it. Keyflow's guide
/// passes one that engraves a chart to inline SVG — the same engraver
/// the app uses, running on the host with no GPU — which is what lets
/// its guide ship with no chart renderer in the browser at all.
pub type FenceRenderer<'a> = dyn Fn(&str, &str) -> Option<String> + 'a;

/// How a vault's notes become HTML.
///
/// Built once per vault and reused across its notes, so the fence
/// renderers and the slug table are set up a single time.
pub struct Renderer<'a> {
    link_base: String,
    known_slugs: BTreeSet<String>,
    fences: Vec<Box<FenceRenderer<'a>>>,
    strip_nav_footer: bool,
    broken_link_class: String,
}

/// What [`Renderer::render`] produces: the HTML plus what the render
/// learned about the note's links.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RenderedPage {
    /// The finished HTML.
    pub html: String,
    /// Wikilink targets that resolved, in document order, deduplicated.
    pub links: Vec<String>,
    /// Wikilink targets that matched no known slug.
    pub broken_links: Vec<String>,
}

impl<'a> Renderer<'a> {
    /// A renderer for a vault mounted at `link_base` (e.g. `/guide`)
    /// containing `slugs`.
    ///
    /// The slug set is required rather than optional because resolution
    /// is the point: without it every wikilink would be emitted as a
    /// link and a typo would ship as a 404 instead of failing the build.
    #[must_use]
    pub fn new(link_base: impl Into<String>, slugs: impl IntoIterator<Item = String>) -> Self {
        Self {
            link_base: link_base.into().trim_end_matches('/').to_owned(),
            known_slugs: slugs.into_iter().collect(),
            fences: Vec::new(),
            strip_nav_footer: true,
            broken_link_class: "ssg-broken-link".to_owned(),
        }
    }

    /// Add a fence renderer. Renderers are tried in the order added, and
    /// the first to return `Some` wins.
    #[must_use]
    pub fn fence(mut self, renderer: impl Fn(&str, &str) -> Option<String> + 'a) -> Self {
        self.fences.push(Box::new(renderer));
        self
    }

    /// Keep the trailing `Previous: … · Next: … · Up: …` line.
    ///
    /// It is dropped by default: the page draws real navigation from the
    /// vault's own ordering, and printing the same chain twice helps
    /// nobody. A vault whose footer carries something other than
    /// navigation turns this back on.
    #[must_use]
    pub fn keep_nav_footer(mut self) -> Self {
        self.strip_nav_footer = false;
        self
    }

    /// The class put on a wikilink that resolved to nothing.
    #[must_use]
    pub fn broken_link_class(mut self, class: impl Into<String>) -> Self {
        self.broken_link_class = class.into();
        self
    }

    /// Render one note.
    //
    // t[impl ssg.render.finished] — the whole page is produced here, at
    // build time. Nothing downstream parses markdown.
    #[must_use]
    pub fn render(&self, note: &Note) -> RenderedPage {
        let (_, body) = Frontmatter::split(&note.source);
        let body = if self.strip_nav_footer {
            strip_nav_footer(body)
        } else {
            body
        };

        let (markdown, links, broken_links) = self.resolve_wikilinks(body);
        RenderedPage {
            html: self.to_html(&markdown),
            links,
            broken_links,
        }
    }

    /// Rewrite every wikilink, and report which resolved.
    //
    // t[impl ssg.render.links] — a target in the slug set becomes a real
    // link; one that is not is reported for the build to fail on.
    fn resolve_wikilinks(&self, body: &str) -> (String, Vec<String>, Vec<String>) {
        let mut out = String::with_capacity(body.len());
        let mut links = Vec::new();
        let mut broken = Vec::new();
        let mut cursor = 0;

        for link in wikilink::wikilinks(body) {
            out.push_str(&body[cursor..link.span.0]);
            cursor = link.span.1;

            if self.known_slugs.contains(&link.target) {
                // Escape the label as markdown link text: a `]` in an
                // alias would otherwise close the link early and spill
                // the URL into the prose.
                out.push('[');
                out.push_str(&escape_link_text(&link.alias));
                out.push_str("](");
                out.push_str(&self.link_base);
                out.push('/');
                out.push_str(&link.target);
                out.push(')');
                if !links.contains(&link.target) {
                    links.push(link.target);
                }
            } else {
                // Not a link. A dead `<a href>` invites a click that
                // 404s; a marked span says "this was meant to point
                // somewhere" and leaves the text readable. The build is
                // expected to fail on these anyway — see
                // `Vault::broken_links` — so this is what a `--no-fail`
                // preview looks like, not what ships.
                out.push_str("<span class=\"");
                out.push_str(&self.broken_link_class);
                out.push_str("\">");
                out.push_str(&escape_html(&link.alias));
                out.push_str("</span>");
                if !broken.contains(&link.target) {
                    broken.push(link.target);
                }
            }
        }
        out.push_str(&body[cursor..]);

        (out, links, broken)
    }

    /// Parse markdown to HTML, giving the fence renderers first refusal
    /// on every fenced block.
    fn to_html(&self, markdown: &str) -> String {
        let mut options = Options::empty();
        options.insert(Options::ENABLE_TABLES);
        options.insert(Options::ENABLE_FOOTNOTES);
        options.insert(Options::ENABLE_STRIKETHROUGH);
        options.insert(Options::ENABLE_SMART_PUNCTUATION);

        let mut events = Vec::new();
        // The fence currently open, as (info string, accumulated body).
        // A fence's language and its text arrive in different events, so
        // the body has to be collected before a renderer can be asked
        // about it.
        let mut open: Option<(String, String)> = None;

        for event in Parser::new_ext(markdown, options) {
            match event {
                Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info))) => {
                    open = Some((info.into_string(), String::new()));
                }
                Event::Text(text) if open.is_some() => {
                    if let Some((_, body)) = open.as_mut() {
                        body.push_str(&text);
                    }
                }
                Event::End(TagEnd::CodeBlock) if open.is_some() => {
                    let Some((info, body)) = open.take() else {
                        continue;
                    };
                    match self.render_fence(&info, &body) {
                        Some(html) => events.push(Event::Html(html.into())),
                        // Nobody claimed it: put back exactly the events
                        // pulldown-cmark would have emitted, so an
                        // ordinary fence renders as an ordinary fence.
                        None => {
                            events.push(Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(
                                info.into(),
                            ))));
                            events.push(Event::Text(body.into()));
                            events.push(Event::End(TagEnd::CodeBlock));
                        }
                    }
                }
                other => events.push(other),
            }
        }

        let mut html = String::with_capacity(markdown.len().saturating_mul(2));
        html::push_html(&mut html, events.into_iter());
        html
    }

    /// First renderer to claim `info` wins.
    //
    // t[impl ssg.render.fences]
    fn render_fence(&self, info: &str, body: &str) -> Option<String> {
        self.fences.iter().find_map(|f| f(info, body))
    }
}

// t[impl ssg.render.metadata] — the footer half; `Frontmatter::split`
// is the other.
/// The note without its trailing `Previous: … · Next: … · Up: …` line.
///
/// The footer is the last line and always carries an `Up:` wikilink, so
/// that is what identifies it. The `---` rule above it goes too — it was
/// separating the footer from the prose, and left behind it would close
/// the page on a horizontal line.
fn strip_nav_footer(body: &str) -> &str {
    let trimmed = body.trim_end();
    let Some((above, last)) = trimmed.rsplit_once('\n') else {
        return body;
    };
    if !last.contains("Up: [[") {
        return body;
    }
    let cut = above.trim_end();
    cut.strip_suffix("---").map_or(cut, str::trim_end)
}

/// Escape the characters that would end a markdown link's text early.
fn escape_link_text(text: &str) -> String {
    text.replace('\\', r"\\")
        .replace('[', r"\[")
        .replace(']', r"\]")
}

/// Escape text being written straight into HTML.
fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(source: &str) -> Note {
        Note {
            slug: "test".to_owned(),
            path: "test.md".into(),
            source: source.to_owned(),
        }
    }

    fn renderer<'a>() -> Renderer<'a> {
        Renderer::new("/guide", ["chords".to_owned(), "rhythm".to_owned()])
    }

    #[test]
    fn resolves_a_wikilink_to_a_route() {
        let out = renderer().render(&note("see [[chords]]"));
        assert!(out.html.contains(r#"<a href="/guide/chords">chords</a>"#));
        assert_eq!(out.links, vec!["chords"]);
        assert!(out.broken_links.is_empty());
    }

    #[test]
    fn an_alias_becomes_the_link_text() {
        let out = renderer().render(&note("see [[chords|the chord page]]"));
        assert!(
            out.html
                .contains(r#"<a href="/guide/chords">the chord page</a>"#)
        );
    }

    // t[verify ssg.render.links]
    #[test]
    fn an_unresolved_target_is_marked_not_linked() {
        let out = renderer().render(&note("see [[missing]]"));
        assert!(!out.html.contains("<a href"));
        assert!(
            out.html
                .contains(r#"<span class="ssg-broken-link">missing</span>"#)
        );
        assert_eq!(out.broken_links, vec!["missing"]);
    }

    #[test]
    fn links_are_deduplicated_but_ordered() {
        let out = renderer().render(&note("[[rhythm]] [[chords]] [[rhythm]]"));
        assert_eq!(out.links, vec!["rhythm", "chords"]);
    }

    // t[verify ssg.render.metadata]
    #[test]
    fn frontmatter_never_reaches_the_html() {
        let out = renderer().render(&note("---\ntitle: Chords\n---\n\n# Chords\n"));
        assert!(!out.html.contains("title:"));
        assert!(out.html.contains("<h1>Chords</h1>"));
    }

    #[test]
    fn the_nav_footer_and_its_rule_are_dropped() {
        let out = renderer().render(&note(
            "# Chords\n\nprose\n\n---\n\nPrevious: [[rhythm]] · Up: [[chords]]\n",
        ));
        assert!(!out.html.contains("Previous:"));
        assert!(!out.html.contains("<hr"));
        assert!(out.html.contains("prose"));
    }

    #[test]
    fn keeping_the_footer_is_opt_in() {
        let out = renderer()
            .keep_nav_footer()
            .render(&note("prose\n\nUp: [[chords]]\n"));
        assert!(out.html.contains("/guide/chords"));
    }

    // t[verify ssg.render.fences]
    #[test]
    fn a_fence_renderer_replaces_its_language() {
        let out = renderer()
            .fence(|info, body| (info == "kf").then(|| format!("<svg>{body}</svg>")))
            .render(&note("```kf\nchart\n```\n"));
        assert!(out.html.contains("<svg>chart\n</svg>"));
        assert!(!out.html.contains("<code"));
    }

    #[test]
    fn an_unclaimed_fence_stays_a_code_block() {
        let out = renderer()
            .fence(|info, _| (info == "kf").then(|| "<svg/>".to_owned()))
            .render(&note("```rust\nfn main() {}\n```\n"));
        assert!(out.html.contains(r#"<code class="language-rust">"#));
        assert!(out.html.contains("fn main()"));
    }

    // t[verify ssg.render.code-verbatim]
    #[test]
    fn wikilinks_inside_a_fence_are_left_alone() {
        let out = renderer().render(&note("```\n[[chords]]\n```\n"));
        assert!(!out.html.contains("<a href"));
        assert!(out.links.is_empty());
    }

    #[test]
    fn tables_and_footnotes_render() {
        let out = renderer().render(&note("| a | b |\n|---|---|\n| 1 | 2 |\n"));
        assert!(out.html.contains("<table>"));
    }
}
