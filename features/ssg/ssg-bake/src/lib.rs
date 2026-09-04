//! `ssg-bake` — routes to `index.html` files.
//!
//! Point it at the directory `dx build` produced, hand it the site's
//! root component and a list of routes, and it writes
//! `<root>/<route>/index.html` for each — the app as it renders at that
//! URL, with no scripts.
//!
//! ```no_run
//! # use dioxus::prelude::*;
//! # fn App() -> Element { rsx! { p { "hi" } } }
//! # fn main() -> Result<(), ssg_bake::BakeError> {
//! ssg_bake::Site::at("target/dx/signal-web/release/web/public", App)?
//!     .page(ssg_bake::Page::new("/guide/running-it").title("Running it — Signal"))
//!     .bake()?;
//! # Ok(()) }
//! ```
//!
//! ## Why not `dx build --ssg`
//!
//! `dx`'s own static generation boots the site as a fullstack server,
//! asks it for a route list over an HTTP endpoint, and fetches every
//! route to warm an incremental cache. It is the right tool when the
//! pages being baked are the *same* pages the server would otherwise
//! render live — the cache is a shortcut, and hydration takes over in
//! the browser.
//!
//! These pages are not that. A guide page is finished at build time and
//! there is nothing for a client to hydrate: no state, no handlers, no
//! server functions. Going through `dx --ssg` would mean making each
//! site a fullstack server — axum, tokio, a server binary, every route
//! in the app compiling for the host — to produce files that a
//! 200-line renderer produces directly. And it would still ship the wasm
//! bootstrap in every baked page, because the index template is
//! per-server, not per-route, so the one thing we actually want would
//! need overriding anyway.
//!
//! So this bakes directly, with `dioxus-ssr`, and leaves `dx build` to
//! do what it is good at: compiling the interactive half of the site and
//! producing the asset-hashed `index.html` that [`Shell::from_index`]
//! then borrows for its `<head>`.
//!
//! ## What comes out
//!
//! An `index.html` per route, with the site's own stylesheet links and
//! meta tags, the page's markup already in the body, and **no scripts**.
//! A reader gets text on the first paint, the browser fetches nothing
//! but CSS, and the page works with JavaScript disabled. Navigation
//! between baked pages is ordinary link-following.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use dioxus::prelude::{Element, History, VirtualDom};

mod history;

use history::BakedHistory;

/// A page to bake.
pub struct Page {
    route: String,
    title: Option<String>,
    description: Option<String>,
}

impl Page {
    /// A page at `route` — a URL path such as `/guide/chords`.
    ///
    /// The route is what gets rendered: the baker seeds the app's
    /// history with it and renders the app, so what lands in the file is
    /// what a browser at that URL would have shown. That matters more
    /// than it sounds — rendering a bare component instead would drop
    /// everything the app puts *around* a page, and a router `Link` in
    /// that chrome would panic for want of a router above it.
    #[must_use]
    pub fn new(route: impl Into<String>) -> Self {
        Self {
            route: route.into(),
            title: None,
            description: None,
        }
    }

    /// Replace the shell's `<title>`.
    #[must_use]
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Set `<meta name="description">`.
    ///
    /// Worth doing for the same reason the title is: these pages are the
    /// ones a search engine and a link preview will actually read, since
    /// they are the ones that are complete without running anything.
    #[must_use]
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}

/// The HTML around a baked page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shell {
    before_body: String,
    after_body: String,
}

impl Shell {
    /// Build a shell from a full `index.html`, splitting it at the app's
    /// mount point and removing every script.
    ///
    /// This is how a baked page keeps the site's own `<head>` —
    /// stylesheet links with their content hashes, fonts, favicons, meta
    /// tags — without any of it being duplicated here and drifting.
    ///
    /// Scripts go because a baked page has nothing to hydrate. Leaving
    /// the wasm bootstrap in would have every reader download and
    /// instantiate a bundle whose only effect is to re-render markup
    /// that is already on screen.
    pub fn from_index(html: &str, mount_id: &str) -> Result<Self, BakeError> {
        let marker = format!(r#"id="{mount_id}""#);
        let at = html
            .find(&marker)
            .ok_or_else(|| BakeError::Shell(format!("no element with {marker} in index.html")))?;
        let open_end = html[at..]
            .find('>')
            .ok_or_else(|| BakeError::Shell("mount element is not closed".to_owned()))?;
        let split = at + open_end + 1;

        Ok(Self {
            before_body: ensure_charset(&strip_scripts(&html[..split])),
            after_body: strip_scripts(&html[split..]),
        })
    }

    /// Read `index.html` from a built site directory.
    pub fn from_index_file(path: impl AsRef<Path>, mount_id: &str) -> Result<Self, BakeError> {
        let path = path.as_ref();
        let html = std::fs::read_to_string(path).map_err(|source| BakeError::Io {
            path: path.to_owned(),
            source,
        })?;
        Self::from_index(&html, mount_id)
    }

    /// A minimal shell, for a site with no built `index.html` to borrow.
    #[must_use]
    pub fn minimal(title: &str) -> Self {
        Self {
            before_body: format!(
                "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n\
                 <meta charset=\"utf-8\">\n\
                 <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
                 <title>{title}</title>\n\
                 </head>\n<body>\n<div id=\"main\">"
            ),
            after_body: "</div>\n</body>\n</html>\n".to_owned(),
        }
    }

    /// Append markup to the `<head>` of every page this shell wraps.
    ///
    /// Needed more often than it looks. A Dioxus app can put its
    /// stylesheet in the document with `document::Style` or
    /// `document::Link` from inside a component, and those are
    /// side-effecting document nodes: `dioxus-ssr` renders the component
    /// tree, not the document, so they leave no trace in a baked page.
    /// A site that styles itself that way passes the same `<style>` or
    /// `<link>` here, and its baked pages look like its live ones.
    #[must_use]
    pub fn head(mut self, html: &str) -> Self {
        self.before_body = match self.before_body.find("</head>") {
            Some(at) => format!(
                "{}{html}{}",
                &self.before_body[..at],
                &self.before_body[at..]
            ),
            // No `</head>` to insert before means this is not a document
            // shell at all. Appending would put the markup in the body,
            // where a `<style>` still applies and a `<link>` still loads
            // — wrong, but visibly wrong rather than silently dropped.
            None => format!("{}{html}", self.before_body),
        };
        self
    }

    /// Wrap rendered markup, applying a page's title and description.
    fn wrap(&self, body: &str, page: &Page) -> String {
        let mut head = self.before_body.clone();
        if let Some(title) = &page.title {
            head = replace_title(&head, title);
        }
        if let Some(description) = &page.description {
            head = set_description(&head, description);
        }
        format!("{head}{body}{}", self.after_body)
    }
}

/// A set of pages to write into one directory.
pub struct Site {
    root: PathBuf,
    shell: Shell,
    app: fn() -> Element,
    pages: Vec<Page>,
}

impl Site {
    /// Bake `app` into `root`, taking the shell from `root/index.html`.
    ///
    /// `root` is what `dx build` produced — its `public` directory. `app`
    /// is the site's root component, the same one it launches with: each
    /// page is that component rendered at one route.
    pub fn at(root: impl Into<PathBuf>, app: fn() -> Element) -> Result<Self, BakeError> {
        let root = root.into();
        let shell = Shell::from_index_file(root.join("index.html"), "main")?;
        Ok(Self {
            root,
            shell,
            app,
            pages: Vec::new(),
        })
    }

    /// Bake into `root` with a shell of your own.
    #[must_use]
    pub fn with_shell(root: impl Into<PathBuf>, app: fn() -> Element, shell: Shell) -> Self {
        Self {
            root: root.into(),
            shell,
            app,
            pages: Vec::new(),
        }
    }

    /// Add a page.
    #[must_use]
    pub fn page(mut self, page: Page) -> Self {
        self.pages.push(page);
        self
    }

    /// Add several.
    #[must_use]
    pub fn pages(mut self, pages: impl IntoIterator<Item = Page>) -> Self {
        self.pages.extend(pages);
        self
    }

    /// Render every page and write it out.
    ///
    /// Returns the paths written, in order, so a caller can report what
    /// it did — a bake that silently produces nothing looks exactly like
    /// a bake that worked.
    pub fn bake(self) -> Result<Vec<PathBuf>, BakeError> {
        let mut written = Vec::with_capacity(self.pages.len());

        for page in &self.pages {
            let dir = self.root.join(page.route.trim_start_matches('/'));
            std::fs::create_dir_all(&dir).map_err(|source| BakeError::Io {
                path: dir.clone(),
                source,
            })?;

            // t[impl ssg.output.route-shape] — a directory plus
            // `index.html` rather than `<route>.html`:
            // it serves at the route's own URL on any static host,
            // without rewrite rules and without a trailing-slash
            // redirect.
            let path = dir.join("index.html");
            let body = self.render(&page.route);
            std::fs::write(&path, self.shell.wrap(&body, page)).map_err(|source| {
                BakeError::Io {
                    path: path.clone(),
                    source,
                }
            })?;
            written.push(path);
        }

        Ok(written)
    }

    /// Render the app as it would appear at `route`.
    ///
    /// The route reaches the app as a [`History`] in root context, which
    /// is where the router looks for it — so the render goes through the
    /// real router and produces the real page, chrome included.
    ///
    /// `rebuild_in_place` is a synchronous rebuild: it resolves the
    /// component tree and does not wait on futures. That is exactly
    /// right for a page whose content is `&'static` and exactly wrong
    /// for one that fetches — which is the line between a page that
    /// should be baked and one that should not.
    fn render(&self, route: &str) -> String {
        let mut dom = VirtualDom::new(self.app)
            .with_root_context::<Rc<dyn History>>(Rc::new(BakedHistory::at(route)));
        dom.rebuild_in_place();
        dioxus::ssr::render(&dom)
    }
}

/// What can go wrong baking.
#[derive(Debug)]
pub enum BakeError {
    /// The shell could not be built from the given `index.html`.
    Shell(String),
    /// A file could not be read or written.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl std::fmt::Display for BakeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Shell(why) => write!(f, "cannot build the page shell: {why}"),
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
        }
    }
}

impl std::error::Error for BakeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Shell(_) => None,
            Self::Io { source, .. } => Some(source),
        }
    }
}

/// Remove every `<script>…</script>`, and the `<link>` tags that exist
/// only to preload one.
//
// t[impl ssg.output.no-script]
fn strip_scripts(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;

    while let Some(start) = rest.find("<script") {
        out.push_str(&rest[..start]);
        // A self-closing or unclosed script tag would otherwise swallow
        // the rest of the document; stop at the tag instead.
        let after = &rest[start..];
        rest = match after.find("</script>") {
            Some(end) => &after[end + "</script>".len()..],
            None => match after.find('>') {
                Some(end) => &after[end + 1..],
                None => "",
            },
        };
    }
    out.push_str(rest);

    strip_script_preloads(&out)
}

/// Drop `<link>` elements that preload scripts or wasm.
///
/// `dx` emits `modulepreload` and `preload as="fetch"` hints for the
/// bundle. With the bootstrap gone they would have the browser fetch a
/// megabyte of wasm that nothing then runs.
fn strip_script_preloads(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;

    while let Some(start) = rest.find("<link") {
        let Some(end) = rest[start..].find('>') else {
            break;
        };
        let tag = &rest[start..start + end + 1];
        let drop = tag.contains("modulepreload")
            || (tag.contains("preload") && (tag.contains(".wasm") || tag.contains(".js")));

        out.push_str(&rest[..start]);
        if !drop {
            out.push_str(tag);
        }
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    out
}

/// Guarantee a `<meta charset>` in the head, adding UTF-8 if there is
/// none.
///
/// Not paranoia. A browser served HTML without a charset — and a plain
/// file server sends no `Content-Type` charset either — falls back to a
/// locale default, and prose full of em dashes and curly quotes comes
/// out as mojibake. Markdown's smart punctuation makes every baked page
/// full of exactly those characters, so this is the difference between
/// readable and not.
fn ensure_charset(head: &str) -> String {
    if head.contains("charset") {
        return head.to_owned();
    }
    const META: &str = r#"<meta charset="utf-8">"#;
    // Directly after `<head>`, because the declaration has to appear in
    // the first 1024 bytes of the document to be honoured.
    match head.find("<head>") {
        Some(at) => {
            let after = at + "<head>".len();
            format!("{}{META}{}", &head[..after], &head[after..])
        }
        None => format!("{META}{head}"),
    }
}

/// Replace the shell's title, or add one if it has none.
fn replace_title(head: &str, title: &str) -> String {
    let escaped = escape(title);
    match (head.find("<title>"), head.find("</title>")) {
        (Some(open), Some(close)) if open < close => {
            format!(
                "{}<title>{escaped}</title>{}",
                &head[..open],
                &head[close + "</title>".len()..]
            )
        }
        _ => match head.find("</head>") {
            Some(at) => format!("{}<title>{escaped}</title>{}", &head[..at], &head[at..]),
            None => head.to_owned(),
        },
    }
}

/// Set `<meta name="description">`, replacing an existing one.
fn set_description(head: &str, description: &str) -> String {
    let meta = format!(
        r#"<meta name="description" content="{}">"#,
        escape(description)
    );

    if let Some(start) = head.find(r#"<meta name="description""#) {
        if let Some(end) = head[start..].find('>') {
            return format!("{}{meta}{}", &head[..start], &head[start + end + 1..]);
        }
    }
    match head.find("</head>") {
        Some(at) => format!("{}{meta}{}", &head[..at], &head[at..]),
        None => head.to_owned(),
    }
}

/// Escape text going into an attribute or element content.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    const INDEX: &str = r#"<!DOCTYPE html><html><head>
<title>Signal</title>
<link rel="stylesheet" href="/assets/main-abc123.css">
<link rel="modulepreload" href="/wasm/signal.js">
<link rel="preload" href="/wasm/signal_bg.wasm" as="fetch" type="application/wasm" crossorigin>
<script type="module">import init from "/wasm/signal.js"; init();</script>
</head><body><div id="main"></div></body></html>"#;

    fn shell() -> Shell {
        Shell::from_index(INDEX, "main").expect("shell")
    }

    // t[verify ssg.output.no-script]
    #[test]
    fn the_stylesheet_survives_and_the_bootstrap_does_not() {
        let shell = shell();
        assert!(shell.before_body.contains("main-abc123.css"));
        assert!(!shell.before_body.contains("<script"));
        assert!(!shell.before_body.contains("modulepreload"));
        assert!(!shell.before_body.contains(".wasm"));
    }

    #[test]
    fn the_body_lands_inside_the_mount_element() {
        let page = Page::new("/guide");
        let html = shell().wrap("<p>prose</p>", &page);
        assert!(html.contains(r#"<div id="main"><p>prose</p></div>"#));
    }

    #[test]
    fn a_page_title_replaces_the_shells() {
        let page = Page::new("/guide").title("Running it — Signal");
        let html = shell().wrap("", &page);
        assert!(html.contains("<title>Running it — Signal</title>"));
        assert!(!html.contains("<title>Signal</title>"));
    }

    #[test]
    fn a_description_is_added_and_escaped() {
        let page = Page::new("/guide").description(r#"Rigs & "racks""#);
        let html = shell().wrap("", &page);
        assert!(html.contains(r#"content="Rigs &amp; &quot;racks&quot;""#));
    }

    #[test]
    fn a_shell_without_a_charset_gets_one() {
        let shell = Shell::from_index(
            "<html><head></head><body><div id=\"main\"></div></body></html>",
            "main",
        )
        .expect("shell");
        assert!(shell.before_body.contains(r#"<meta charset="utf-8">"#));
        // Inside the first 1024 bytes, which is the rule that makes it
        // count.
        let at = shell.before_body.find("charset").expect("charset");
        assert!(at < 1024);
    }

    #[test]
    fn an_existing_charset_is_left_alone() {
        let html = r#"<html><head><meta charset="iso-8859-1"></head><body><div id="main"></div></body></html>"#;
        let shell = Shell::from_index(html, "main").expect("shell");
        assert!(shell.before_body.contains("iso-8859-1"));
        assert_eq!(shell.before_body.matches("charset").count(), 1);
    }

    #[test]
    fn extra_head_markup_lands_inside_the_head() {
        let shell = shell().head("<style>body{color:red}</style>");
        let page = Page::new("/guide");
        let html = shell.wrap("", &page);
        let head_end = html.find("</head>").expect("has a head");
        let style = html.find("<style>").expect("style was added");
        assert!(style < head_end, "style should sit inside the head");
    }

    #[test]
    fn a_missing_mount_point_is_an_error_not_a_silent_empty_page() {
        let err = Shell::from_index("<html><body></body></html>", "main").expect_err("no mount");
        assert!(matches!(err, BakeError::Shell(_)));
    }

    #[test]
    fn an_unclosed_script_tag_does_not_swallow_the_document() {
        let stripped = strip_scripts("<head><script src=x.js></head><body>keep</body>");
        assert!(stripped.contains("keep"));
        assert!(!stripped.contains("script"));
    }
}
