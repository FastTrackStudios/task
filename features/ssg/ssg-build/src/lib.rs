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
    body: Option<Box<dyn Fn(&str) -> String + 'a>>,
    keep_nav_footer: bool,
    allow_broken_links: bool,
    feeds: Option<(String, PathBuf)>,
    dates: bool,
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
            body: None,
            keep_nav_footer: false,
            allow_broken_links: false,
            feeds: None,
            dates: false,
        }
    }

    /// Date each page from git — when the note last changed.
    ///
    /// One `git log` per note, at build time, which is cheap for a vault
    /// of a few dozen and is why this is opt-in rather than automatic.
    ///
    /// Degrades to nothing. A build with no git on `PATH`, or from a
    /// source tree with no history — which is exactly what a nix
    /// derivation hands you, since it copies the files and not the
    /// repository — leaves every date empty, and every consumer of a
    /// date already has to handle that. A build that half-works is the
    /// right outcome here: a missing "last updated" line is a smaller
    /// problem than a build that will not run outside a git checkout.
    #[must_use]
    pub fn dates(mut self) -> Self {
        self.dates = true;
        self
    }

    /// Also write `sitemap.xml` and `rss.xml` for the vault, given the
    /// site's origin (`https://ignition.fasttrackstudio.app`).
    ///
    /// `dir` is relative to the crate being built, and is a *source*
    /// directory rather than `OUT_DIR` on purpose: these two files have
    /// to be served from fixed URLs — `/sitemap.xml` is the one every
    /// crawler looks for — so they cannot go through the asset pipeline,
    /// which content-hashes what it touches. The site's build recipe
    /// copies them into the output alongside the pre-rendered pages.
    ///
    /// Both are generated, so the directory belongs in `.gitignore`.
    #[must_use]
    pub fn feeds(mut self, site_url: impl Into<String>, dir: impl AsRef<Path>) -> Self {
        let dir = dir.as_ref();
        let dir = if dir.is_absolute() {
            dir.to_owned()
        } else {
            manifest_dir().join(dir)
        };
        self.feeds = Some((site_url.into(), dir));
        self
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

    /// Render each note's body with `f` instead of the built-in markdown
    /// pass — see [`ssg_vault::Renderer::body_renderer`].
    #[must_use]
    pub fn body_renderer(mut self, f: impl Fn(&str) -> String + 'a) -> Self {
        self.body = Some(Box::new(f));
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

        let mut vault = ssg_vault::scan_with(&self.dir, |slugs| {
            let mut renderer = Renderer::new(&self.link_base, slugs);
            if self.keep_nav_footer {
                renderer = renderer.keep_nav_footer();
            }
            for fence in &self.fences {
                renderer = renderer.fence(move |info, body| fence(info, body));
            }
            if let Some(body) = &self.body {
                renderer = renderer.body_renderer(move |md| body(md));
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

        if self.dates {
            for page in &mut vault.pages {
                page.updated = git_date(&self.dir.join(format!("{}.md", page.slug)));
            }
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

        if let Some((site, dir)) = &self.feeds {
            std::fs::create_dir_all(dir)
                .unwrap_or_else(|e| panic!("cannot create {}: {e}", dir.display()));
            write(
                &dir.join("sitemap.xml"),
                &ssg_vault::sitemap(&vault, site, &self.link_base),
            );
            // The channel is named for the vault's front door, which is
            // the closest thing a guide has to a title of its own.
            let title = vault
                .pages
                .first()
                .map_or_else(String::new, |p| p.title.clone());
            write(
                &dir.join("rss.xml"),
                &ssg_vault::rss(&vault, site, &self.link_base, &title, ""),
            );
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
            let tags = page
                .tags
                .iter()
                .map(|t| format!("{t:?}"))
                .collect::<Vec<_>>()
                .join(", ");
            let headings = page
                .headings
                .iter()
                .map(|h| {
                    format!(
                        "{crate_path}::StaticHeading {{ level: {}, text: {:?}, id: {:?} }}",
                        h.level, h.text, h.id
                    )
                })
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
                 source: {:?},\n        body: {:?},\n        html: {:?},\n        links: &[{links}],\n        \
                 headings: &[{headings}],\n        tags: &[{tags}],\n        words: {},\n        \
                 updated: {:?},\n    }},\n",
                page.slug,
                page.title,
                page.summary,
                page.order,
                page.stage,
                page.kind,
                page.source,
                page.body,
                page.html,
                page.words,
                page.updated,
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

/// When a file last changed, as an RFC 3339 date, or empty.
///
/// The committer date rather than the author date: it is when the change
/// landed on this branch, which is what "last updated" means to a
/// reader. Any failure — no git, no history, an untracked file — is
/// empty rather than an error, because none of them is a reason to fail
/// a build.
fn git_date(path: &Path) -> String {
    let Ok(output) = std::process::Command::new("git")
        .args(["log", "-1", "--format=%cI", "--"])
        .arg(path)
        .current_dir(path.parent().unwrap_or(Path::new(".")))
        .output()
    else {
        return String::new();
    };
    if !output.status.success() {
        return String::new();
    }
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// Write a generated file, only when its contents changed.
///
/// Rewriting identical bytes still bumps the mtime, and a build script
/// whose output looks new every time makes everything downstream of it
/// look new every time.
fn write(path: &Path, contents: &str) {
    if std::fs::read_to_string(path).is_ok_and(|old| old == contents) {
        return;
    }
    std::fs::write(path, contents)
        .unwrap_or_else(|e| panic!("cannot write {}: {e}", path.display()));
}

/// The crate being built.
fn manifest_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"))
}

/// Where cargo wants generated code written.
fn out_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"))
}
