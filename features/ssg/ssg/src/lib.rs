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
//! **3. The route enumerates itself for the bake.** `dx build --ssg`
//! asks the running server which paths to render, over a server function
//! at `/api/static_routes`. A dynamic route like `/guide/:slug` cannot be
//! enumerated from the route table — only the vault knows its slugs — so
//! it hands them over:
//!
//! ```ignore
//! #[server(endpoint = "static_routes")]
//! async fn static_routes() -> ServerFnResult<Vec<String>> {
//!     Ok(guide::VAULT.routes("/guide"))
//! }
//! ```
//!
//! That is the whole of *partial* static generation: those paths are
//! baked to HTML at build time, and every other route stays live.
//!
//! ## What the reader gets
//!
//! An `index.html` per note, complete before any script runs: prose,
//! resolved cross-references, engraved fences, and the link graph as
//! plain SVG. Guide pages carry no wasm at all — see the `ssg-ui` module
//! docs for why that is a design constraint rather than an optimisation.

pub use ssg_vault::{
    Frontmatter, Note, Page, RenderedPage, Renderer, ScanError, StaticPage, StaticVault, Vault,
    scan, scan_with,
};

#[cfg(feature = "ui")]
pub use ssg_ui::{Backlinks, ChapterNav, VAULT_CSS, VAULT_STYLE, VaultArticle, VaultToc};

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
