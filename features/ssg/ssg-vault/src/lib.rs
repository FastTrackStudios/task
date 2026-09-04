//! `ssg-vault` — a vault of markdown notes, read once at build time and
//! rendered to finished HTML.
//!
//! Four FTS sites (Keyflow, Signal, Ignition, Session) publish a guide
//! that is a *vault*: a directory of markdown notes carrying frontmatter
//! and `[[wikilink]]` cross-references, authored in the repo and edited
//! through Task. Each site had grown its own build script and its own
//! renderer to ship that vault, and the three that existed had drifted
//! into three different answers:
//!
//! - Ignition rendered markdown to HTML in `build.rs` — right.
//! - Signal shipped the markdown and ran a parser in the browser.
//! - Keyflow shipped the markdown and rendered it through the *editor*
//!   in read-only mode, which meant a reader waited on the editor, the
//!   editor's state machine and a WebGL2 chart surface before a
//!   paragraph appeared.
//!
//! This crate is the one answer. A vault is read, parsed and rendered on
//! the host, at build time, and what reaches the browser is HTML that is
//! already finished. Nothing here is interactive and nothing here runs in
//! a browser: editing a vault belongs in the Task app, and rendering one
//! belongs in a build script.
//!
//! ## What "finished" means
//!
//! [`Renderer::render`] resolves everything a reader's browser would
//! otherwise have to:
//!
//! - **`[[wikilinks]]`** become real `<a href>`s under a configured base
//!   path, with `|` aliases honoured and unresolved targets marked
//!   rather than silently linked into the void.
//! - **Fenced code blocks** pass through a [`FenceRenderer`] first, so a
//!   site can substitute its own markup for its own languages — Keyflow
//!   engraves a ```kf fence to inline SVG here, which is why its guide
//!   needs neither the chart renderer nor a GPU.
//! - **Frontmatter and nav footers** are stripped from the prose; they
//!   are metadata, and the page draws its own navigation from the
//!   ordering the frontmatter declares.
//!
//! ## Shape
//!
//! - [`scan`] walks a directory into [`Note`]s — the raw text plus its
//!   parsed frontmatter.
//! - [`Renderer`] turns a [`Note`] into a [`RenderedPage`].
//! - `ssg-build` codegens those pages into a `&'static [Page]` for a
//!   consuming crate; `ssg-ui` renders that static into Dioxus.
//!
//! Everything here is pure apart from [`scan`], and [`scan`] is the only
//! thing that touches the filesystem — so the renderer is testable
//! against string literals and builds anywhere.

mod feed;
mod frontmatter;
mod render;
mod scan;
mod static_model;
mod wikilink;

pub use feed::{rss, sitemap};
pub use frontmatter::Frontmatter;
pub use render::{FenceRenderer, Heading, RenderedPage, Renderer};
pub use scan::{Note, ScanError, scan, scan_with};
pub use static_model::{StaticHeading, StaticPage, StaticVault};
pub use wikilink::{Wikilink, wikilinks};

/// A page as it exists in a built site: the finished HTML plus the
/// metadata a table of contents needs.
///
/// This is the owned, build-time form. `ssg-build` lowers it into a
/// `&'static` equivalent that a consuming crate includes directly, so a
/// running site allocates none of this.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    /// URL segment, from the file name. Also the `[[wikilink]]` target.
    pub slug: String,
    /// Display title — frontmatter `title:`, else the first `# heading`,
    /// else the de-slugged file name.
    pub title: String,
    /// One line for a table of contents — frontmatter `summary:` or
    /// `blurb:`, whichever the vault uses.
    pub summary: String,
    /// Sort key from `order:`. Pages without one sort last.
    pub order: u32,
    /// The section this note sits under in the table of contents, from
    /// `stage:`. Empty when the vault has no stages.
    pub stage: String,
    /// Frontmatter `type:`, lowercased — what the graph colours by.
    /// `"other"` when absent.
    pub kind: String,
    /// The note verbatim, frontmatter and footer included. The graph
    /// reads this; a reader never sees it.
    pub source: String,
    /// The note's markdown without its frontmatter.
    ///
    /// Neither of the other two: [`Self::source`] carries the metadata
    /// block, and [`Self::html`] is no longer markdown. This is for a
    /// consumer that wants the prose *as text* — Keyflow's workbench
    /// opens a chapter in a live editor, and an editor showing a
    /// `---` block is showing the reader plumbing.
    pub body: String,
    /// The note rendered for a reader: no frontmatter, no nav footer,
    /// wikilinks resolved, fences expanded.
    pub html: String,
    /// The note's headings, in document order — its own shape, for an
    /// in-page contents list and for a search index to point into.
    pub headings: Vec<Heading>,
    /// Frontmatter `tags:`, lowercased.
    pub tags: Vec<String>,
    /// Words of prose, for a reading estimate.
    pub words: u32,
    /// When the note last changed, as an RFC 3339 date, or empty when
    /// nothing established it. Filled in by `ssg-build` from git
    /// history, which is opt-in — see its `dates` method.
    pub updated: String,
    /// Outbound wikilink targets that resolved to a page in this vault,
    /// in document order and deduplicated. Backlinks are the reverse of
    /// this, computed over the whole vault.
    pub links: Vec<String>,
    /// Wikilink targets that matched no page. Kept rather than dropped
    /// so a build can report a broken cross-reference instead of
    /// shipping a dead link.
    pub broken_links: Vec<String>,
}

/// Every page of a vault, in reading order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Vault {
    /// Pages sorted by `(order, slug)` — the frontmatter's order, with
    /// the slug breaking ties so a build is reproducible.
    pub pages: Vec<Page>,
}

impl Vault {
    /// Look up a page by slug.
    #[must_use]
    pub fn page(&self, slug: &str) -> Option<&Page> {
        self.pages.iter().find(|p| p.slug == slug)
    }

    /// The pages that link *to* `slug`, in reading order.
    #[must_use]
    pub fn backlinks(&self, slug: &str) -> Vec<&Page> {
        self.pages
            .iter()
            .filter(|p| p.links.iter().any(|l| l == slug))
            .collect()
    }

    /// Every wikilink in the vault that resolved to nothing, as
    /// `(source slug, missing target)` pairs.
    ///
    /// A build script is expected to fail on a non-empty result: a
    /// broken cross-reference in a vault is a typo, and it is cheaper to
    /// catch at build time than to ship.
    #[must_use]
    pub fn broken_links(&self) -> Vec<(&str, &str)> {
        self.pages
            .iter()
            .flat_map(|p| {
                p.broken_links
                    .iter()
                    .map(move |t| (p.slug.as_str(), t.as_str()))
            })
            .collect()
    }
}
