//! `ssg-ui` — the components that render a built vault.
//!
//! Every component here is a pure function of `&'static` data. There are
//! no signals, no effects, no hooks, and no event handlers: a guide page
//! is rendered once, on the host, by `dx build --ssg`, and what ships is
//! the HTML that came out. Navigation is ordinary `<a href>`, which is
//! why these pages work with JavaScript off and why a site can serve
//! them without putting its wasm bundle in front of a reader.
//!
//! That constraint is the point, not an accident of the port. Keyflow's
//! guide previously rendered through the editor in read-only mode: to
//! read a paragraph you waited on the editor crate, its state machine,
//! its decoration pipeline and a WebGL2 chart surface. Nothing in that
//! chain was needed to show text that had not changed since the build.
//!
//! ## The pieces
//!
//! - [`VaultArticle`] — one note's prose.
//! - [`VaultToc`] — the table of contents, grouped by stage.
//! - [`ChapterNav`] — previous / next in reading order.
//! - [`Backlinks`] — what points here.
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

pub use article::VaultArticle;
#[cfg(feature = "graph")]
pub use graph::{VaultGraph, local_graph, vault_graph};
pub use nav::{Backlinks, ChapterNav, VaultToc};

pub use ssg_vault::{StaticPage, StaticVault};

use dioxus::prelude::Asset;
use dioxus::prelude::*;

/// A minimal stylesheet for the components: layout, spacing and the
/// broken-link mark, and nothing else.
///
/// Colours come from CSS custom properties with inherited fallbacks, so
/// the sheet slots into a site's theme rather than fighting it. A site
/// that would rather style `ssg-*` itself simply does not link this.
pub const VAULT_STYLE: Asset = asset!("/assets/vault.css");
