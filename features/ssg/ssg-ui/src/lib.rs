//! `ssg-ui` — the components that render a built vault.
//!
//! Every component here is a pure function of `&'static` data: no
//! signals, no effects, no hooks, no event handlers, no futures.
//!
//! That is what makes them safe to pre-render. `dx build --ssg` renders
//! each route on the server and writes the HTML to disk; the browser
//! paints that, then the wasm bundle hydrates it. Hydration only works
//! if the client's first render matches the server's exactly, and a
//! component with no state and no I/O cannot disagree with itself.
//!
//! It is also the point of the exercise. Keyflow's guide used to render
//! through the editor in read-only mode: to read a paragraph you waited
//! on the editor crate, its state machine, its decoration pipeline and a
//! WebGL2 chart surface — none of which was needed to show text that had
//! not changed since the build. Now that text is in the HTML, and the
//! bundle arrives afterwards to make the site interactive again.
//!
//! Navigation is ordinary `<a href>` rather than the router's `Link`,
//! for a duller reason: `Link` is generic over the site's own `Routable`
//! enum, which this crate cannot know. The cost is a page load between
//! chapters instead of a client-side transition — and every one of those
//! pages is pre-rendered, so it is a cheap one. A site that wants the
//! transition can lay out its own contents rail with `Link` and use the
//! rest of these components as they are.
//!
//! ## The pieces
//!
//! - [`VaultArticle`] — one note's prose.
//! - [`VaultToc`] — the table of contents, grouped by stage.
//! - [`ChapterNav`] — previous / next in reading order.
//! - [`Backlinks`] — what points here.
//! - [`PageToc`] — the headings *inside* one page, as `#fragment`
//!   links. The other table of contents.
//! - [`PageTags`], [`TagIndex`], [`TaggedPages`] — the vault's
//!   cross-cutting axis, for a vault whose notes carry `tags:`.
//! - [`VaultSearch`] — search over the whole vault, with no index to
//!   build and nothing to fetch: the pages are already in the binary.
//! - `VaultGraph` — the local or whole-vault link graph, as static SVG
//!   with clickable nodes. Behind the `graph` feature: it reaches the
//!   knowledge-graph crate, and a site publishing prose should not pay
//!   for a dependency chain it never draws.
//!
//! They compose into a page but do not assume one: a site lays them out
//! in its own shell, with its own chrome. Class names are all `ssg-`
//! prefixed and every component takes a `class` override, so a site can
//! either use [`VAULT_STYLE`] or style them entirely itself.

mod article;
#[cfg(feature = "graph")]
mod graph;
mod nav;
mod page_toc;
mod search;
mod tags;

pub use article::VaultArticle;
#[cfg(feature = "graph")]
pub use graph::{VaultGraph, local_graph, vault_graph};
pub use nav::{Backlinks, ChapterNav, VaultToc};
pub use page_toc::PageToc;
pub use search::{Hit, VaultSearch, search};
pub use tags::{PageTags, TagIndex, TaggedPages};

pub use ssg_vault::{StaticHeading, StaticPage, StaticVault};

use dioxus::prelude::Asset;
use dioxus::prelude::*;

/// A minimal stylesheet for the components: layout, spacing and the
/// broken-link mark, and nothing else.
///
/// Colours come from CSS custom properties with inherited fallbacks, so
/// the sheet slots into a site's theme rather than fighting it. A site
/// that would rather style `ssg-*` itself simply does not link this.
pub const VAULT_STYLE: Asset = asset!("/assets/vault.css");

/// The same stylesheet, as bytes.
///
/// [`VAULT_STYLE`] is the right thing for a site that links its CSS.
/// This is for the two cases where that does not work:
///
/// - **Crossing a repo boundary.** Every consumer of this crate is a
///   different repository, and `include_str!` into a git dependency has
///   no stable path on disk. Exporting the bytes is the established
///   answer here — `architect_ui::THEME_CSS` exists for the same reason.
/// - **A pre-rendered page.** Inlining the sheet means the HTML that
///   `dx build --ssg` writes is styled on its own, with no second round
///   trip before the text is readable — which is most of the point of
///   pre-rendering it.
pub const VAULT_CSS: &str = include_str!("../assets/vault.css");
