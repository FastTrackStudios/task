//! `ssg-build` — a vault, compiled into the site that publishes it.
//!
//! This is the whole of a site's `build.rs`:
//!
//! ```no_run
//! // build.rs
//! ssg_build::Vault::at("docs/guides/keyflow")
//!     .link_base("/guide")
//!     .emit();
//! ```
//!
//! and the whole of the module that consumes it:
//!
//! ```ignore
//! ssg::include_vault!();          // pub static VAULT: ssg::StaticVault
//! ```
//!
//! It replaces four hand-written build scripts — one per site, between
//! 130 and 210 lines each — that had drifted into three different
//! answers about how a vault becomes a page. What comes out is
//! `&'static` data: finished HTML, resolved links, reading order. A
//! running site parses nothing.
//!
//! ## Why a build script at all
//!
//! The notes live outside the crate that publishes them
//! (`docs/guides/…`, not `apps/web/src/…`), and `include_str!` across
//! that boundary is invisible to cargo — it fails at compile time rather
//! than resolution time, and editing a note does not trigger a rebuild.
//! A build script is the sanctioned way out: the dependency is explicit,
//! and the `cargo:rerun-if-changed` lines [`Vault::emit`] prints make
//! cargo aware of every note it read.
//!
//! ## Failure
//!
//! Loudly. A build script's only error channel is a panic, and every
//! failure mode here — a vault directory that moved, a note that cannot
//! be read, a `[[wikilink]]` pointing at nothing — is one where the
//! alternative is shipping a site that is quietly wrong: an empty guide,
//! a missing chapter, a dead cross-reference. Better to fail the build.

// A build script reports failure by panicking; there is no other
// channel, and a vault that silently compiles to nothing would ship as
// an empty guide.
#![expect(
    clippy::panic,
    clippy::expect_used,
    reason = "build-script helper: panicking is the only way to fail a build"
)]

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use ssg_vault::{FenceRenderer, Renderer};

/// A vault to compile into the current crate.
pub struct Vault<'a> {
    dir: PathBuf,
    link_base: String,
    static_name: String,
    out_file: String,
    out_dir: Option<PathBuf>,
    crate_path: String,
    fences: Vec<Box<FenceRenderer<'a>>>,
    keep_nav_footer: bool,
    allow_broken_links: bool,
}

impl<'a> Vault<'a> {
    /// A vault at `dir`, relative to the crate being built.
    ///
    /// Relative to `CARGO_MANIFEST_DIR` rather than to the working
    /// directory, so the same call works under `cargo build`, `dx serve`
    /// and a nix sandbox, which disagree about where a build script runs.
    /// An absolute path is used as-is.
    #[must_use]
    pub fn at(dir: impl AsRef<Path>) -> Self {
        let dir = dir.as_ref();
        let dir = if dir.is_absolute() {
            dir.to_owned()
        } else {
            manifest_dir().join(dir)
        };

        Self {
            dir,
            link_base: "/guide".to_owned(),
            static_name: "VAULT".to_owned(),
            out_file: "ssg_vault.rs".to_owned(),
            out_dir: None,
            crate_path: "::ssg".to_owned(),
            fences: Vec::new(),
            keep_nav_footer: false,
            allow_broken_links: false,
        }
    }

    /// The URL prefix `[[wikilinks]]` resolve under. Default `/guide`.
    #[must_use]
    pub fn link_base(mut self, base: impl Into<String>) -> Self {
        self.link_base = base.into();
        self
    }

    /// Name of the generated static. Default `VAULT`.
    #[must_use]
    pub fn static_name(mut self, name: impl Into<String>) -> Self {
        self.static_name = name.into();
        self
    }

    /// File name written into `OUT_DIR`. Default `ssg_vault.rs`.
    ///
    /// Worth changing only when a crate compiles more than one vault.
    #[must_use]
    pub fn out_file(mut self, name: impl Into<String>) -> Self {
        self.out_file = name.into();
        self
    }

    /// Path the generated code reaches the page types through. Default
    /// `::ssg` — override for a crate that depends on `ssg-vault`
    /// directly instead of the facade.
    #[must_use]
    pub fn crate_path(mut self, path: impl Into<String>) -> Self {
        self.crate_path = path.into();
        self
    }

    /// Where to write the generated file. Defaults to cargo's `OUT_DIR`,
    /// which is what a build script wants.
    ///
    /// Exists because `OUT_DIR` is process-global: anything that drives
    /// `emit` other than cargo — a test, a tool that renders a vault
    /// without building anything — has no way to say where the output
    /// goes without racing every other caller in the process.
    #[must_use]
    pub fn out_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.out_dir = Some(dir.into());
        self
    }

    /// Substitute markup for a fenced code block — see
    /// [`ssg_vault::FenceRenderer`].
    ///
    /// This is where a site puts its own rendering into an otherwise
    /// static page. Keyflow engraves its ```kf fences to inline SVG here,
    /// on the host, with the same engraver the app uses and no GPU — so
    /// its guide ships charts without shipping a chart renderer.
    #[must_use]
    pub fn fence(mut self, renderer: impl Fn(&str, &str) -> Option<String> + 'a) -> Self {
        self.fences.push(Box::new(renderer));
        self
    }

    /// Keep each note's trailing `Previous: … · Up: …` line, which is
    /// dropped by default.
    #[must_use]
    pub fn keep_nav_footer(mut self) -> Self {
        self.keep_nav_footer = true;
        self
    }

    /// Warn about `[[wikilinks]]` that resolve to nothing instead of
    /// failing the build.
    ///
    /// For a vault mid-rewrite, where half the pages a note points at do
    /// not exist yet. Not for a vault that ships.
    #[must_use]
    pub fn allow_broken_links(mut self) -> Self {
        self.allow_broken_links = true;
        self
    }

    /// Read the vault, render it, and write the generated static into
    /// `OUT_DIR`.
    ///
    /// Panics with the reason if the vault cannot be read, is empty, or
    /// (unless [`Self::allow_broken_links`]) contains a wikilink that
    /// resolves to nothing.
    pub fn emit(self) {
        println!("cargo:rerun-if-changed=build.rs");
        println!("cargo:rerun-if-changed={}", self.dir.display());

        let vault = ssg_vault::scan_with(&self.dir, |slugs| {
            let mut renderer = Renderer::new(&self.link_base, slugs);
            if self.keep_nav_footer {
                renderer = renderer.keep_nav_footer();
            }
            for fence in &self.fences {
                renderer = renderer.fence(move |info, body| fence(info, body));
            }
            renderer
        })
        .unwrap_or_else(|e| panic!("{e}"));

        // t[impl ssg.build.rerun] — per-note, not just the directory: on most filesystems a
        // directory's mtime does not change when a file inside it is
        // edited, so watching the directory alone catches an added or
        // deleted note and misses every edit to an existing one.
        for page in &vault.pages {
            println!(
                "cargo:rerun-if-changed={}",
                self.dir.join(format!("{}.md", page.slug)).display()
            );
        }

        let broken = vault.broken_links();
        if !broken.is_empty() {
            let detail = broken
                .iter()
                .map(|(from, to)| format!("  {from}.md → [[{to}]]"))
                .collect::<Vec<_>>()
                .join("\n");
            assert!(
                self.allow_broken_links,
                "{} broken cross-reference(s) in {}:\n{detail}\n\
                 Fix the target, or call .allow_broken_links() to downgrade this to a warning.",
                broken.len(),
                self.dir.display(),
            );
            println!("cargo:warning=broken cross-references in the vault:\n{detail}");
        }

        let dest = self
            .out_dir
            .clone()
            .unwrap_or_else(out_dir)
            .join(&self.out_file);
        std::fs::write(&dest, self.codegen(&vault))
            .unwrap_or_else(|e| panic!("cannot write {}: {e}", dest.display()));
    }

    /// The generated source: one `StaticPage` per note, plus the
    /// `StaticVault` that wraps them.
    fn codegen(&self, vault: &ssg_vault::Vault) -> String {
        let Self {
            static_name,
            crate_path,
            ..
        } = self;

        let mut out = format!(
            "// @generated by ssg-build from {} — do not edit.\n\n",
            self.dir.display()
        );

        let _ = writeln!(
            out,
            "static {static_name}_PAGES: &[{crate_path}::StaticPage] = &["
        );
        for page in &vault.pages {
            let links = page
                .links
                .iter()
                .map(|l| format!("{l:?}"))
                .collect::<Vec<_>>()
                .join(", ");
            // `{:?}` on a &str is a valid Rust string literal — escapes,
            // quotes and all — which is what makes emitting source out of
            // arbitrary note text safe rather than a quoting minefield.
            let _ = write!(
                out,
                "    {crate_path}::StaticPage {{\n        \
                 slug: {:?},\n        title: {:?},\n        summary: {:?},\n        \
                 order: {},\n        stage: {:?},\n        kind: {:?},\n        \
                 source: {:?},\n        html: {:?},\n        links: &[{links}],\n    }},\n",
                page.slug,
                page.title,
                page.summary,
                page.order,
                page.stage,
                page.kind,
                page.source,
                page.html,
            );
        }
        out.push_str("];\n\n");

        let _ = writeln!(
            out,
            "/// The vault compiled from `{}`, in reading order.\n\
             pub static {static_name}: {crate_path}::StaticVault =\n    \
             {crate_path}::StaticVault::new({static_name}_PAGES);",
            self.dir.display()
        );
        out
    }
}

/// The crate being built.
fn manifest_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"))
}

/// Where cargo wants generated code written.
fn out_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"))
}
