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

use std::collections::{BTreeMap, BTreeSet};

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
    /// Normalised link target → canonical slug. See [`Renderer::alias`].
    aliases: BTreeMap<String, String>,
    fences: Vec<Box<FenceRenderer<'a>>>,
    /// Renders a note's body to HTML, replacing the built-in markdown
    /// pass. See [`Renderer::body_renderer`].
    body: Option<Box<dyn Fn(&str) -> String + 'a>>,
    strip_nav_footer: bool,
    broken_link_class: String,
}

/// One heading in a note.
///
/// The note's own shape, which a page uses for its in-page contents and
/// a search index uses to say *where* in a page a hit is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heading {
    /// `1` for `#`, `2` for `##`, and so on.
    pub level: u8,
    /// The heading's text, with any markup flattened away.
    pub text: String,
    /// Its `id` in the rendered HTML — what `#fragment` addresses it.
    pub id: String,
}

/// What [`Renderer::render`] produces: the HTML plus what the render
/// learned about the note.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RenderedPage {
    /// The finished HTML.
    pub html: String,
    /// The note's headings, in document order.
    pub headings: Vec<Heading>,
    /// How many words of prose the note carries.
    pub words: u32,
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
        let known_slugs: BTreeSet<String> = slugs.into_iter().collect();
        // Every slug resolves case-insensitively, and with spaces where
        // it has dashes. `[[Key Meter Changes]]` is how a person writing
        // prose refers to `key-meter-changes.md`, and Obsidian resolves
        // it, so a vault authored in Obsidian is full of them.
        let aliases = known_slugs
            .iter()
            .map(|slug| (normalise(slug), slug.clone()))
            .collect();

        Self {
            body: None,
            link_base: link_base.into().trim_end_matches('/').to_owned(),
            known_slugs,
            aliases,
            fences: Vec::new(),
            strip_nav_footer: true,
            broken_link_class: "ssg-broken-link".to_owned(),
        }
    }

    /// Let `alias` resolve to `slug`.
    ///
    /// A vault's links are written by a person, and a person writes the
    /// page's *name*: `[[Recording]]`, not `[[recording]]`. Titles are
    /// registered here so those resolve — which is what Obsidian and
    /// Quartz do, and what stops a real vault arriving with dozens of
    /// "broken" references that are nothing of the kind.
    ///
    /// Matching ignores case, and treats spaces and dashes alike. An
    /// alias that collides with an existing one loses: a slug is always
    /// the more specific claim on its own name.
    #[must_use]
    pub fn alias(mut self, alias: &str, slug: impl Into<String>) -> Self {
        self.aliases
            .entry(normalise(alias))
            .or_insert_with(|| slug.into());
        self
    }

    /// Register several aliases — typically every page's title.
    #[must_use]
    pub fn aliases<K: AsRef<str>, V: Into<String>>(
        mut self,
        pairs: impl IntoIterator<Item = (K, V)>,
    ) -> Self {
        for (alias, slug) in pairs {
            self = self.alias(alias.as_ref(), slug);
        }
        self
    }

    /// The slug a wikilink target names, if the vault has one.
    fn resolve(&self, target: &str) -> Option<&str> {
        if self.known_slugs.contains(target) {
            // Borrowed from the set so the exact spelling wins without a
            // lookup in the alias table.
            return self.known_slugs.get(target).map(String::as_str);
        }
        self.aliases.get(&normalise(target)).map(String::as_str)
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
        let markdown = rewrite_callouts(&markdown);
        let (built_in, headings) = self.to_html(&markdown);
        // Headings come from the markdown either way — see
        // `body_renderer`. Only the HTML is the host's to replace.
        let html = self
            .body
            .as_ref()
            .map_or(built_in, |render| render(&markdown));
        RenderedPage {
            html,
            headings,
            words: word_count(body),
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

            if let Some(slug) = self.resolve(&link.target).map(ToOwned::to_owned) {
                // Escape the label as markdown link text: a `]` in an
                // alias would otherwise close the link early and spill
                // the URL into the prose.
                out.push('[');
                out.push_str(&escape_link_text(&link.alias));
                out.push_str("](");
                out.push_str(&self.link_base);
                out.push('/');
                // The resolved slug, not what was written: `[[Recording]]`
                // has to become `/guide/recording` or the link 404s on a
                // case-sensitive host.
                out.push_str(&slug);
                out.push(')');
                if !links.contains(&slug) {
                    links.push(slug);
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

    /// Render each note's body with `f` instead of the built-in markdown
    /// pass.
    ///
    /// For a host that already has a markdown renderer it would rather
    /// use — the editor's, so that a note looks the same published as it
    /// does being written, callouts and task lists and all.
    ///
    /// Headings are still parsed out of the markdown here, because the
    /// table of contents, the deep links and the search index are built
    /// from them and a replacement renderer has no reason to know about
    /// any of that. Wikilinks are still resolved before `f` sees the
    /// text, so what arrives is ordinary markdown with real links in it.
    #[must_use]
    pub fn body_renderer(mut self, f: impl Fn(&str) -> String + 'a) -> Self {
        self.body = Some(Box::new(f));
        self
    }

    /// Parse markdown to HTML, giving the fence renderers first refusal
    /// on every fenced block, and give every heading an `id`.
    ///
    /// The ids are what make a heading addressable — `#the-song-map` in
    /// a URL, a table of contents that can link into the page, a search
    /// result that lands on a section rather than at the top. They are
    /// derived from the heading text the way GitHub derives them, so a
    /// link written by hand against the rendered page keeps working.
    fn to_html(&self, markdown: &str) -> (String, Vec<Heading>) {
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
        // The heading currently open, as (level, its events, its text).
        // Same reason: the text arrives in pieces, and the `id` cannot
        // be computed until all of it has.
        let mut heading: Option<(u8, Vec<Event<'_>>, String)> = None;
        let mut headings: Vec<Heading> = Vec::new();
        let mut used_ids: BTreeMap<String, u32> = BTreeMap::new();
        // Markdown has no nested links, but a counter rather than a bool
        // keeps the close from firing on an *external* link's `End`.
        let mut internal_link_depth = 0u32;

        for event in Parser::new_ext(markdown, options) {
            match event {
                // t[impl ssg.render.internal-links]
                // A link into this vault is marked, so the page can tell
                // "somewhere else in the guide" from "off the site" —
                // which is what a preview on hover needs to know, and
                // what lets a stylesheet distinguish the two.
                //
                // Rewritten as raw HTML rather than left as a link event
                // so the attribute can be added; the label's own markup
                // passes through untouched between the two halves,
                // because only the tags are replaced.
                Event::Start(Tag::Link { ref dest_url, .. })
                    if dest_url.starts_with(&format!("{}/", self.link_base)) =>
                {
                    let slug = dest_url
                        .rsplit('/')
                        .next()
                        .unwrap_or_default()
                        .split('#')
                        .next()
                        .unwrap_or_default();
                    events.push(Event::Html(
                        format!(
                            "<a href=\"{}\" data-ssg-link=\"{}\">",
                            escape_html(dest_url),
                            escape_html(slug)
                        )
                        .into(),
                    ));
                    internal_link_depth += 1;
                }
                Event::End(TagEnd::Link) if internal_link_depth > 0 => {
                    internal_link_depth -= 1;
                    events.push(Event::Html("</a>".into()));
                }
                Event::Start(Tag::Heading { level, .. }) => {
                    heading = Some((level as u8, Vec::new(), String::new()));
                }
                // t[impl ssg.order.headings]
                Event::End(TagEnd::Heading(_)) if heading.is_some() => {
                    let Some((level, inner, text)) = heading.take() else {
                        continue;
                    };
                    let id = unique_id(&slugify(&text), &mut used_ids);
                    events.push(Event::Html(
                        format!("<h{level} id=\"{}\">", escape_html(&id)).into(),
                    ));
                    events.extend(inner);
                    events.push(Event::Html(format!("</h{level}>").into()));
                    headings.push(Heading { level, text, id });
                }
                // Inside a heading everything is buffered, including the
                // text, so the id can be built from it.
                event if heading.is_some() => {
                    if let Some((_, inner, text)) = heading.as_mut() {
                        if let Event::Text(t) | Event::Code(t) = &event {
                            text.push_str(t);
                        }
                        inner.push(event);
                    }
                }
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
        (html, headings)
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

/// A heading's text as an HTML `id`.
///
/// GitHub's rules, because they are the ones a person writing
/// `[see below](#the-song-map)` by hand will have in mind: lowercase,
/// spaces to dashes, and everything that is not a letter, number, dash
/// or underscore dropped.
fn slugify(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.trim().chars() {
        if ch.is_alphanumeric() || ch == '-' || ch == '_' {
            out.extend(ch.to_lowercase());
        } else if ch.is_whitespace() {
            if !out.ends_with('-') && !out.is_empty() {
                out.push('-');
            }
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    // A heading of nothing but punctuation would otherwise get an empty
    // id, and `#` addresses the top of the page rather than that
    // heading.
    if out.is_empty() {
        out.push_str("section");
    }
    out
}

/// `id`, made unique within the page.
///
/// Two `## Notes` headings in one note are ordinary, and duplicate ids
/// are not: the second one would be unaddressable, because a fragment
/// always finds the first. Suffixed the way GitHub does it.
fn unique_id(id: &str, used: &mut BTreeMap<String, u32>) -> String {
    let count = used.entry(id.to_owned()).or_insert(0);
    *count += 1;
    if *count == 1 {
        id.to_owned()
    } else {
        format!("{id}-{}", *count - 1)
    }
}

/// Words of prose in a note.
///
/// Deliberately rough — it counts whitespace-separated runs over the
/// markdown, so a fenced code block counts and so does a link's URL.
/// The number exists to say "this is a five-minute read, not a
/// forty-minute one", and no reader has ever needed that to be exact.
fn word_count(body: &str) -> u32 {
    u32::try_from(body.split_whitespace().count()).unwrap_or(u32::MAX)
}

/// A link target reduced to what it names, ignoring how it was spelled.
///
/// Lowercased, with runs of spaces, dashes and underscores collapsed to
/// a single dash. So `Key Meter Changes`, `key-meter-changes` and
/// `Key_Meter__Changes` are one target.
fn normalise(target: &str) -> String {
    let mut out = String::with_capacity(target.len());
    let mut last_was_sep = false;
    for ch in target.trim().chars() {
        if ch.is_whitespace() || ch == '-' || ch == '_' {
            if !last_was_sep && !out.is_empty() {
                out.push('-');
            }
            last_was_sep = true;
        } else {
            out.extend(ch.to_lowercase());
            last_was_sep = false;
        }
    }
    // A trailing separator would make `foo-` a different target from
    // `foo`, which no author means.
    while out.ends_with('-') {
        out.pop();
    }
    out
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

/// The callout types, in the order the editor's slash menu offers them.
///
/// Twelve, not GitHub's five. The syntax is Obsidian's and so is the set,
/// because these notes are written in the editor — a chapter drafted with
/// `> [!question]` has to render as a question here, not fall back to a
/// plain quote because the published renderer only knew about alerts.
const CALLOUTS: [&str; 12] = [
    "note", "abstract", "info", "tip", "success", "question", "warning", "failure", "danger",
    "bug", "example", "quote",
];

/// Rewrite `> [!type] Title` blockquotes into callout markup.
///
/// A pre-pass over the markdown rather than a transform of the event
/// stream: the wikilink rewrite above is already a pre-pass, the callout
/// body has to stay markdown (it holds links, code, charts), and an HTML
/// block followed by a blank line lets CommonMark parse the inside of it
/// normally. Doing it in the event stream would mean buffering a whole
/// blockquote to find out what it was.
///
/// `pulldown-cmark` 0.10 has no `BlockQuoteKind`, and the version that
/// does only knows GitHub's five alerts, so this is hand-rolled either
/// way.
fn rewrite_callouts(markdown: &str) -> String {
    let mut out = String::with_capacity(markdown.len());
    let mut lines = markdown.lines().peekable();

    while let Some(line) = lines.next() {
        let Some((kind, title)) = callout_header(line) else {
            out.push_str(line);
            out.push('\n');
            continue;
        };

        out.push_str(&format!(
            "<div class=\"ssg-callout ssg-callout-{kind}\">\n<div class=\"ssg-callout-title\">{}</div>\n<div class=\"ssg-callout-body\">\n\n",
            escape_html(&title),
        ));
        // The rest of the quote, with one level of `> ` removed, is
        // ordinary markdown and is emitted as such.
        while let Some(next) = lines.peek() {
            let Some(rest) = next.strip_prefix('>') else {
                break;
            };
            out.push_str(rest.strip_prefix(' ').unwrap_or(rest));
            out.push('\n');
            lines.next();
        }
        out.push_str("\n</div>\n</div>\n\n");
    }
    out
}

/// `> [!type]` or `> [!type] Title`, if the line is one.
///
/// The title defaults to the type, capitalised, which is what Obsidian
/// shows and what makes a bare `> [!warning]` useful on its own.
fn callout_header(line: &str) -> Option<(&'static str, String)> {
    let rest = line.trim_start().strip_prefix('>')?.trim_start();
    let rest = rest.strip_prefix("[!")?;
    let (kind, rest) = rest.split_once(']')?;
    let kind = CALLOUTS
        .iter()
        .find(|k| k.eq_ignore_ascii_case(kind.trim()))?;
    let title = rest.trim();
    let title = if title.is_empty() {
        let mut c = kind.chars();
        c.next().map_or_else(String::new, |f| {
            f.to_uppercase().collect::<String>() + c.as_str()
        })
    } else {
        title.to_owned()
    };
    Some((kind, title))
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
        assert!(
            out.html
                .contains(r#"<a href="/guide/chords" data-ssg-link="chords">chords</a>"#)
        );
        assert_eq!(out.links, vec!["chords"]);
        assert!(out.broken_links.is_empty());
    }

    #[test]
    fn an_alias_becomes_the_link_text() {
        let out = renderer().render(&note("see [[chords|the chord page]]"));
        assert!(
            out.html
                .contains(r#"data-ssg-link="chords">the chord page</a>"#)
        );
    }

    // t[verify ssg.render.internal-links]
    #[test]
    fn an_external_link_is_not_marked_as_a_vault_link() {
        let out = renderer().render(&note("[docs](https://example.com) and [[chords]]"));
        // The marker says "somewhere else in this guide", which is what
        // a hover preview and a stylesheet both need to know.
        assert!(
            out.html
                .contains(r#"<a href="https://example.com">docs</a>"#)
        );
        assert_eq!(out.html.matches("data-ssg-link").count(), 1);
    }

    #[test]
    fn a_marked_link_keeps_the_markup_in_its_label() {
        let out = renderer().render(&note("see [[chords|the **chord** page]]"));
        assert!(out.html.contains("data-ssg-link=\"chords\""));
        assert!(out.html.contains("<strong>chord</strong>"), "{}", out.html);
    }

    #[test]
    fn a_target_resolves_whatever_its_case() {
        let out = renderer().render(&note("see [[Chords]] and [[CHORDS]]"));
        assert_eq!(out.links, vec!["chords"]);
        // The canonical slug, not what was written — a case-sensitive
        // host would 404 on `/guide/Chords`.
        assert!(out.html.contains(r#"href="/guide/chords""#));
        assert!(!out.html.contains("/guide/Chords"));
    }

    #[test]
    fn spaces_dashes_and_underscores_name_the_same_page() {
        let r = Renderer::new("/guide", ["key-meter-changes".to_owned()]);
        for written in [
            "key meter changes",
            "Key Meter Changes",
            "key_meter__changes",
        ] {
            let out = r.render(&note(&format!("see [[{written}]]")));
            assert_eq!(out.links, vec!["key-meter-changes"], "for `{written}`");
        }
    }

    #[test]
    fn a_title_resolves_to_its_page() {
        let out = renderer()
            .alias("The Rhythm Page", "rhythm")
            .render(&note("see [[The Rhythm Page]]"));
        assert_eq!(out.links, vec!["rhythm"]);
    }

    #[test]
    fn a_slug_beats_an_alias_that_collides_with_it() {
        // `chords` is a real page; an alias claiming that name must not
        // take it, or a link to the page would land somewhere else.
        let out = renderer()
            .alias("chords", "rhythm")
            .render(&note("see [[chords]]"));
        assert_eq!(out.links, vec!["chords"]);
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
        assert!(out.html.contains(r#"<h1 id="chords">Chords</h1>"#));
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

    // t[verify ssg.order.headings]
    #[test]
    fn headings_get_addressable_ids() {
        let out = renderer().render(&note("# The Song Map\n\n## Why it works\n"));
        assert!(
            out.html
                .contains(r#"<h1 id="the-song-map">The Song Map</h1>"#)
        );
        assert!(
            out.html
                .contains(r#"<h2 id="why-it-works">Why it works</h2>"#)
        );

        let ids: Vec<&str> = out.headings.iter().map(|h| h.id.as_str()).collect();
        assert_eq!(ids, ["the-song-map", "why-it-works"]);
        assert_eq!(out.headings[0].level, 1);
        assert_eq!(out.headings[1].level, 2);
        assert_eq!(out.headings[1].text, "Why it works");
    }

    #[test]
    fn a_repeated_heading_still_gets_its_own_id() {
        // A fragment always finds the first match, so a duplicate id
        // would make the second heading unaddressable.
        let out = renderer().render(&note("## Notes\n\ntext\n\n## Notes\n"));
        let ids: Vec<&str> = out.headings.iter().map(|h| h.id.as_str()).collect();
        assert_eq!(ids, ["notes", "notes-1"]);
    }

    #[test]
    fn heading_markup_is_flattened_for_the_id_and_kept_in_the_html() {
        let out = renderer().render(&note("## The `kf` fence, explained\n"));
        assert_eq!(out.headings[0].text, "The kf fence, explained");
        assert_eq!(out.headings[0].id, "the-kf-fence-explained");
        // The rendered heading keeps its `<code>`.
        assert!(out.html.contains("<code>kf</code>"));
    }

    #[test]
    fn a_heading_of_only_punctuation_still_gets_an_id() {
        let out = renderer().render(&note("## ???\n"));
        assert_eq!(out.headings[0].id, "section");
    }

    #[test]
    fn words_are_counted_for_a_reading_estimate() {
        let out = renderer().render(&note("---\ntitle: x\n---\n\none two three four\n"));
        assert_eq!(out.words, 4);
    }

    #[test]
    fn tables_and_footnotes_render() {
        let out = renderer().render(&note("| a | b |\n|---|---|\n| 1 | 2 |\n"));
        assert!(out.html.contains("<table>"));
    }
}

#[cfg(test)]
mod callout_tests {
    use super::*;

    #[test]
    fn a_callout_becomes_its_own_block() {
        let out = rewrite_callouts("> [!warning] Careful\n> body text\n");
        assert!(out.contains("ssg-callout-warning"), "{out}");
        assert!(out.contains("Careful"), "{out}");
        assert!(out.contains("body text"), "{out}");
    }

    #[test]
    fn a_bare_callout_titles_itself() {
        let out = rewrite_callouts("> [!tip]\n> do this\n");
        assert!(out.contains(">Tip</div>"), "{out}");
    }

    #[test]
    fn every_editor_callout_type_is_known() {
        // The set has to match the editor's slash menu, or a chapter
        // written there renders as a plain quote once published.
        for kind in CALLOUTS {
            let src = format!("> [!{kind}] t\n> b\n");
            let out = rewrite_callouts(&src);
            assert!(
                out.contains(&format!("ssg-callout-{kind}")),
                "{kind}: {out}"
            );
        }
    }

    #[test]
    fn an_ordinary_quote_is_left_alone() {
        let out = rewrite_callouts("> just a quote\n");
        assert!(!out.contains("ssg-callout"), "{out}");
        assert!(out.contains("> just a quote"), "{out}");
    }

    #[test]
    fn an_unknown_type_is_left_as_a_quote() {
        let out = rewrite_callouts("> [!nosuchtype] x\n> y\n");
        assert!(!out.contains("ssg-callout"), "{out}");
    }

    #[test]
    fn the_body_stays_markdown() {
        // The inside of a callout holds links, code and charts, so it
        // must reach the parser as markdown rather than as escaped text.
        let out = rewrite_callouts("> [!note] N\n> see **this**\n");
        assert!(out.contains("**this**"), "{out}");
    }
}

#[cfg(test)]
mod callout_render_tests {
    use super::*;

    #[test]
    fn a_callout_survives_the_whole_render() {
        // Through `Renderer::render`, not just the pre-pass: the HTML
        // block has to reach the parser intact and its body has to come
        // out parsed rather than escaped.
        let r = Renderer::new("/guide", vec!["other".to_owned()]);
        let note = Note {
            slug: "n".to_owned(),
            path: std::path::PathBuf::from("n.md"),
            source: "> [!tip] Try it\n> see **bold** and [[other]]\n".to_owned(),
        };
        let page = r.render(&note);
        assert!(page.html.contains("ssg-callout-tip"), "{}", page.html);
        assert!(page.html.contains("<strong>bold</strong>"), "{}", page.html);
        assert!(page.html.contains("/guide/other"), "{}", page.html);
    }
}

#[cfg(test)]
mod body_renderer_tests {
    use super::*;

    fn note(source: &str) -> Note {
        Note {
            slug: "n".to_owned(),
            path: std::path::PathBuf::from("n.md"),
            source: source.to_owned(),
        }
    }

    #[test]
    fn a_body_renderer_replaces_the_html() {
        let r = Renderer::new("/guide", Vec::new()).body_renderer(|md| format!("<!--{md}-->"));
        let page = r.render(&note("# Title\n\nbody\n"));
        assert!(page.html.starts_with("<!--"), "{}", page.html);
        assert!(!page.html.contains("<h1"), "{}", page.html);
    }

    #[test]
    fn headings_still_come_from_the_markdown() {
        // The table of contents, deep links and search are built from
        // these, and a replacement renderer has no reason to know that.
        let r = Renderer::new("/guide", Vec::new()).body_renderer(|_| "<p>x</p>".to_owned());
        let page = r.render(&note("# Title\n\n## Second\n"));
        assert_eq!(page.headings.len(), 2, "{:?}", page.headings);
    }

    #[test]
    fn wikilinks_are_resolved_before_the_renderer_sees_them() {
        // What arrives is ordinary markdown with real links in it, so a
        // host renderer needs to know nothing about `[[…]]`.
        let seen = std::cell::RefCell::new(String::new());
        {
            let r = Renderer::new("/guide", vec!["other".to_owned()]).body_renderer(|md| {
                seen.borrow_mut().push_str(md);
                String::new()
            });
            let _ = r.render(&note("see [[other]]\n"));
        }
        assert!(seen.borrow().contains("/guide/other"), "{}", seen.borrow());
        assert!(!seen.borrow().contains("[["), "{}", seen.borrow());
    }

    #[test]
    fn without_one_the_built_in_pass_is_used() {
        let r = Renderer::new("/guide", Vec::new());
        let page = r.render(&note("# Title\n"));
        assert!(page.html.contains("<h1"), "{}", page.html);
    }
}
