//! Search over `<vault>/Wiki/`.
//!
//! Two modes, mirroring `wiki_proto::search::SearchMode`:
//!
//! - **Token** (always on) — grep + TF-IDF over page bodies.
//!   Pure Rust, no embeddings, no model.
//! - **Hybrid** (feature `vector`) — token search merged
//!   with vector retrieval over a `LanceDB` table. Off by
//!   default; enable when the vault has been indexed via
//!   the embedding pipeline (lands separately).
//!
//! Both modes share the `search()` entry point — callers
//! pick the mode via `SearchOpts`. When the binary wasn't
//! built with `vector`, `Hybrid` downgrades to `Token`
//! transparently and reports the actual mode used in
//! `SearchHits::mode`.

mod scan;
mod token;

#[cfg(feature = "vector")]
mod vector;

use std::path::Path;

use thiserror::Error;
use wiki_proto::search::{SearchHits, SearchMode, SearchOpts};

#[derive(Debug, Error)]
pub enum SearchError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("wiki root not found at {0}")]
    NoWikiRoot(String),
    #[error("vector backend: {0}")]
    Vector(String),
}

/// Token-only search over `<vault_root>/Wiki/`. Always
/// available.
pub fn search(vault_root: &Path, opts: SearchOpts) -> Result<SearchHits, SearchError> {
    match opts.mode {
        SearchMode::Token => token::search_token(vault_root, &opts),
        SearchMode::Hybrid => search_hybrid(vault_root, opts),
    }
}

#[cfg(feature = "vector")]
fn search_hybrid(vault_root: &Path, opts: SearchOpts) -> Result<SearchHits, SearchError> {
    vector::search_hybrid(vault_root, &opts)
}

#[cfg(not(feature = "vector"))]
fn search_hybrid(vault_root: &Path, opts: SearchOpts) -> Result<SearchHits, SearchError> {
    // Graceful downgrade — the response's `mode` field
    // will report `Token` so callers see what actually
    // ran.
    let mut opts = opts;
    opts.mode = SearchMode::Token;
    token::search_token(vault_root, &opts)
}
