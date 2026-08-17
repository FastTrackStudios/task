//! `pdf` — HTML/CSS to PDF.
//!
//! Wrapper over [`fulgur`](https://github.com/fulgur-rs/fulgur):
//! a Servo-derived stylo/blitz layout pipeline fronted by a
//! MiniJinja template engine, writing to PDF via `krilla`.
//! Equivalent to "WeasyPrint, but pure Rust" — exactly the
//! shape we need for invoice templates.
//!
//! ## Why this lives at `libs/` not `features/`
//!
//! `libs/` is for cross-cutting deps (one crate, no proto/db/ui
//! split). `features/` is for product features with their own
//! data model. PDF rendering is a tool — finance uses it for
//! invoices, reports use it for weekly summaries, future
//! exports use it for whatever else. Single crate, no schema.
//!
//! ## Three entry points
//!
//! - [`render_html`] — minimal: HTML + optional CSS → PDF bytes.
//! - [`render_template`] — MiniJinja template + JSON data → PDF
//!   bytes. The standard path for invoices + reports; templates
//!   live in the calling crate's `templates/` directory.
//! - [`render_invoice`] — convenience wrapper around
//!   `render_template` with the built-in invoice template
//!   ([`INVOICE_TEMPLATE`]). Pass an [`InvoiceData`] struct;
//!   get a PDF back.
//!
//! ## Pinning
//!
//! fulgur is at v0.16 (pre-1.0). Cargo.toml pins to `=0.16`;
//! bumping is a deliberate review, not a `cargo update`. The
//! transitive deps (stylo, blitz, krilla, taffy, parley) are
//! heavy — first build will be slow.

mod error;
mod invoice;
mod render;

pub use error::PdfError;
pub use invoice::{render_invoice, InvoiceData, InvoiceLine, InvoiceParty, INVOICE_TEMPLATE};
pub use render::{render_html, render_template};
