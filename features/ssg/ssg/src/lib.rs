//! `ssg` — publish a Task vault as a static site.
//!
//! A *vault* is a directory of markdown notes with frontmatter and
//! `[[wikilink]]` cross-references: Task's own wiki, and the guide every
//! FTS site ships. This feature turns one into HTML at build time, so a
//! reader gets a finished page instead of a program that renders one.
//!
//! Editing stays in the Task app. Nothing here writes, syncs, or knows
//! how to; a vault is read once, by a build script, out of the
//! repository that owns it.
//!
//! ## The three moving parts
//!
//! **1. A build script compiles the vault in.** `ssg-build` reads the
//! directory, renders each note, and writes a `&'static` page table into
//! `OUT_DIR`:
//!
//! ```ignore
//! // build.rs — `ssg-build` is a *build*-dependency of the site, so it
//! // is not in scope here to compile against.
//! fn main() {
//!     ssg_build::Vault::at("../../docs/guides/keyflow")
//!         .link_base("/guide")
//!         .emit();
//! }
//! ```
//!
//! **2. The site includes it.** One macro, and the vault is a static:
//!
//! ```ignore
//! // src/guide.rs
//! ssg::include_vault!();   // pub static VAULT: ssg::StaticVault
//! ```
//!
//! **3. The vault enumerates its own routes for the pre-render.** This
//! is Dioxus's own static generation, wired the way its documentation
//! prescribes. `dx build --ssg` builds the app's server, starts it, POSTs
//! `/api/static_routes` for the list of paths, and GETs each one so the
//! incremental renderer writes it to disk:
//!
//! ```ignore
//! // The server's incremental cache writes into `public` next to the
//! // executable — which is where the CLI also puts the web bundle, so
//! // the two land in one deployable directory. `clear_cache(false)`
//! // keeps it from deleting the rest of that directory on each build.
//! fn main() {
//!     dioxus::LaunchBuilder::new()
//!         .with_cfg(server_only! {
//!             ServeConfig::builder().incremental(
//!                 dioxus::server::IncrementalRendererConfig::new()
//!                     .static_dir(
//!                         std::env::current_exe().unwrap().parent().unwrap().join("public"),
//!                     )
//!                     .clear_cache(false),
//!             )
//!         })
//!         .launch(App);
//! }
//!
//! // The endpoint name is fixed — the CLI looks for exactly this one.
//! #[server(endpoint = "static_routes")]
//! async fn static_routes() -> ServerFnResult<Vec<String>> {
//!     Ok(guide::VAULT.routes("/guide"))
//! }
//! ```
//!
//! Build it with **both** flags:
//!
//! ```text
//! dx build --platform web --ssg --force-sequential
//! ```
//!
//! `--force-sequential` is not about build speed. The pre-render uses
//! `public/index.html` as its page shell, and that file is written by
//! the *client* build; run in parallel — the default — the server can
//! reach the render before the client has produced it, and every page
//! comes out wrapped in Dioxus's bare fallback shell instead: no
//! `<title>`, no `<meta charset>` (so smart punctuation arrives as
//! mojibake), and no bundle script, so nothing hydrates. The build still
//! reports success. Sequential runs the client first. (dioxus#3518.)
//!
//! [`StaticVault::routes`] is the interesting half. `Routable::static_routes()`
//! gives the router's *fully static* paths, and a vault's pages are not
//! among them: `/guide/:slug` is one route with a parameter, and only the
//! vault knows what the slugs are. Handing that list over is the whole of
//! **partial** static generation — those paths are pre-rendered, and
//! every other route in the app is untouched and still dynamic.
//!
//! ## What the reader gets
//!
//! An `index.html` per note, already carrying the finished page: prose,
//! resolved cross-references, expanded fences, and the link graph as
//! SVG. It paints before the bundle has loaded; the bundle then hydrates
//! it and the site is an ordinary Dioxus app again, with client-side
//! routing between chapters.
//!
//! Because the pages are complete HTML, the output directory is also
//! servable by anything — no server process, no rewrite rules.

pub use ssg_vault::{
    Frontmatter, Heading, Note, Page, RenderedPage, Renderer, ScanError, StaticHeading, StaticPage,
    StaticVault, Vault, rss, scan, scan_with, sitemap,
};

#[cfg(feature = "ui")]
pub use ssg_ui::{
    Backlinks, ChapterNav, Hit, LinkPreviews, PageTags, PageToc, TagIndex, TaggedPages, VAULT_CSS,
    VAULT_STYLE, VaultArticle, VaultSearch, VaultToc, search,
};

#[cfg(feature = "graph")]
pub use ssg_ui::{VaultGraph, local_graph, vault_graph};

/// Include the vault table `ssg-build` generated.
///
/// Expands to the `pub static VAULT: StaticVault` the build script
/// emitted. Call it in the module that owns the guide.
///
/// The default file name matches `ssg-build`'s; a crate compiling more
/// than one vault passes the name it gave to `Vault::out_file`:
///
/// ```ignore
/// ssg::include_vault!();                  // ssg_vault.rs
/// ssg::include_vault!("ssg_recipes.rs");  // a second vault
/// ```
#[macro_export]
macro_rules! include_vault {
    () => {
        $crate::include_vault!("ssg_vault.rs");
    };
    ($file:literal) => {
        include!(concat!(env!("OUT_DIR"), "/", $file));
    };
}
