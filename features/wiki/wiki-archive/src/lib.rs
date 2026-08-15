//! `task wiki archive <url|file>` — the URL-archiving front
//! door for the wiki feature (phase 1, tracked issue
//! `32e7b28f`).
//!
//! Pipeline shape (the raw→ingest pipeline itself is
//! UNCHANGED — this crate only produces its inputs):
//!
//! ```text
//! URL ──► router::classify ──► extractor ──► (Provenance, markdown body)
//!                                                  │
//!                  compose_source_markdown ◄───────┘
//!                          │
//!        Wiki/raw/sources/<slug>-<canon8>.md   (sha-deduped by wiki-live)
//!                          │
//!                  ingest queue (two-step LLM, review loop)
//! ```
//!
//! - [`router`] — content-type classification + canonical-URL
//!   normalization. The canonical URL is the cross-importer
//!   dedup key: the same article saved in Pocket and Readwise
//!   canonicalizes identically, hashes identically, and lands
//!   on the same `raw/sources/` filename.
//! - [`provenance`] — the frontmatter contract on archived
//!   source pages (`source_url`, `canonical_url`,
//!   `content_type`, `archived_at`, `extractor`, `media`,
//!   `duration`) + slug/filename derivation.
//!
//! Extractors (article readability, Google Docs export,
//! yt-dlp transcripts) and the bookmark importers (Readwise /
//! Karakeep / Pocket / Netscape) layer on in sibling modules;
//! each emits the same `(Provenance, String)` pair so the CLI
//! glue stays a single code path.

pub mod article;
pub mod feed;
pub mod health;
pub mod import;
pub mod pdf;
pub mod podcast;
pub mod provenance;
pub mod reddit;
pub mod router;
pub mod transcript;
#[cfg(feature = "whisper")]
pub mod whisper;
pub mod x;
pub mod youtube;

pub use provenance::{Provenance, canon_hash8, compose_source_markdown, find_canonical_match};
pub use router::{Route, canonicalize, classify, content_type_for};

use thiserror::Error;

/// Errors across the archive crate. Extractor-specific
/// failure detail rides in the message — callers branch on
/// the variant, humans read the string.
#[derive(Debug, Error)]
pub enum ArchiveError {
    /// The input wasn't a parseable absolute URL.
    #[error("invalid url `{0}`: {1}")]
    InvalidUrl(String, String),
    /// Network-level fetch failure (DNS, TLS, timeout, …).
    #[error("fetch {url}: {message}")]
    Fetch { url: String, message: String },
    /// The server answered, but not usefully (4xx/5xx,
    /// empty body, paywall interstitial, …).
    #[error("bad response from {url}: {message}")]
    BadResponse { url: String, message: String },
    /// Readability/extraction produced nothing usable.
    #[error("extract {url}: {message}")]
    Extract { url: String, message: String },
    /// Subprocess extractor (yt-dlp) failed or is missing.
    #[error("subprocess `{program}`: {message}")]
    Subprocess { program: String, message: String },
    /// Import-file parse failure (Pocket zip, Netscape HTML,
    /// recorded API JSON, …).
    #[error("import parse: {0}")]
    ImportParse(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}
