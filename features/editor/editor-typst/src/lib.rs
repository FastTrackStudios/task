//! Typst rendering for the Editor.
//!
//! Exposes a `Compiler` that takes a Typst source string and
//! returns either an SVG (for inline live-preview) or a PDF
//! (for "Export PDF"). The crate is intentionally narrow —
//! one struct, three methods — so the markdown decoration
//! source can call into it from a single place.
//!
//! Wasm-compatible. The hostile bits of the upstream `typst`
//! crate (rayon, stacker) are not pulled in by the default
//! feature set; `send_wrapper` + `parking_lot` cover what's
//! needed for the single-threaded browser runtime.

mod world;

use thiserror::Error;
pub use world::World;

/// Anything that can go wrong compiling a Typst source.
#[derive(Debug, Error)]
pub enum CompileError {
    /// Typst's diagnostics — usually syntax / type errors in
    /// the source. Pre-formatted by `format_diagnostics` for
    /// display.
    #[error("typst compile failed:\n{0}")]
    Diagnostics(String),
    /// PDF export failed at the `typst-pdf` layer. Rare; usually
    /// indicates a malformed `PagedDocument`.
    #[error("typst pdf export failed: {0}")]
    Pdf(String),
}

/// A reusable Typst compiler. Holds the bundled-font World;
/// successive `compile_*` calls reuse comemo's caches so
/// re-rendering the same source after a small edit is fast.
pub struct Compiler {
    world: World,
}

impl Compiler {
    /// Build a compiler with the default bundled font set
    /// (`typst-assets`'s `fonts` feature). The Editor only
    /// needs one — keep it as a long-lived singleton.
    #[must_use]
    pub fn new() -> Self {
        Self {
            world: World::with_bundled_fonts(),
        }
    }

    /// Override the main source. Subsequent renders use this as
    /// the document. Cheap — only re-parses if the source
    /// actually changed.
    pub fn set_source(&mut self, source: impl Into<String>) {
        self.world.set_main_source(source.into());
    }

    /// Compile the current source to a single concatenated SVG.
    /// Suitable for embedding as the `data-widget-html` payload
    /// of an editor `Decoration::widget`.
    pub fn compile_svg(&mut self) -> Result<String, CompileError> {
        let document = typst::compile::<typst::layout::PagedDocument>(&self.world)
            .output
            .map_err(|errs| CompileError::Diagnostics(format_diagnostics(&errs, &self.world)))?;
        // For inline math / small blocks the doc is one page;
        // for longer Typst snippets we stack pages with a 0-gap
        // separator using `typst-svg::svg_merged`.
        let svg = if document.pages.len() == 1 {
            typst_svg::svg(&document.pages[0])
        } else {
            typst_svg::svg_merged(&document, typst::layout::Abs::zero())
        };
        Ok(svg)
    }

    /// Compile the current source to PDF bytes. Used by the
    /// "Export PDF" command.
    pub fn compile_pdf(&mut self) -> Result<Vec<u8>, CompileError> {
        let document = typst::compile::<typst::layout::PagedDocument>(&self.world)
            .output
            .map_err(|errs| CompileError::Diagnostics(format_diagnostics(&errs, &self.world)))?;
        let options = typst_pdf::PdfOptions::default();
        typst_pdf::pdf(&document, &options).map_err(|e| CompileError::Pdf(format!("{e:?}")))
    }
}

impl Default for Compiler {
    fn default() -> Self {
        Self::new()
    }
}

/// Pretty-print typst's `SourceDiagnostic` list into something
/// the UI can show in a toast or hover popover. Carries the
/// span text where available.
fn format_diagnostics(errs: &ecow::EcoVec<typst::diag::SourceDiagnostic>, world: &World) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    for diag in errs {
        let _ = write!(out, "{:?}: {}", diag.severity, diag.message);
        if let Some(span) = world.lookup_span(diag.span) {
            let _ = write!(out, "  @ {span}");
        }
        out.push('\n');
        for hint in &diag.hints {
            let _ = writeln!(out, "  hint: {hint}");
        }
    }
    out.trim_end().to_string()
}
