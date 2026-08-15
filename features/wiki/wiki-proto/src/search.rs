//! Hybrid token + vector search over the wiki.
//!
//! Mirrors `llm_wiki`'s `POST /api/v1/projects/{id}/search`.
//! Token search is always available (cheap grep with TF-IDF
//! ranking); vector search is optional and degrades gracefully
//! to token-only when the backend has no vector index loaded
//! (see [`SearchHits::mode`]).

use facet::Facet;

#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(C)]
pub struct SearchOpts {
    /// User query. Tokenized by the backend (lowercase, split
    /// on whitespace + punctuation).
    pub query: String,
    /// Result cap. `0` ⇒ backend default (`llm_wiki` uses 20).
    pub top_k: u32,
    /// If `true`, include the matched page's full markdown
    /// (with frontmatter) in each hit. `false` ⇒ snippet only.
    pub include_content: bool,
    /// What kind of search to run.
    pub mode: SearchMode,
    /// Optional `type:` filter — only hits whose page has a
    /// matching `type:` frontmatter field. Empty = no filter.
    pub node_type: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Facet)]
#[repr(C)]
pub enum SearchMode {
    /// Grep + TF-IDF ranking over markdown bodies. No
    /// embedding required.
    Token,
    /// Token + vector hybrid. Requires the backend to have a
    /// vector index loaded; otherwise the response downgrades
    /// to `Token`.
    Hybrid,
}

#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(C)]
pub struct SearchHits {
    /// What the backend actually ran. May differ from the
    /// request when `Hybrid` was asked for but no vector
    /// index is loaded.
    pub mode: SearchMode,
    /// Hit count contributed by token search alone.
    pub token_count: u32,
    /// Hit count contributed by vector search alone. `0` for
    /// `mode == Token`.
    pub vector_count: u32,
    /// Final merged + ranked hits. Sorted by `score`
    /// descending.
    pub hits: Vec<SearchHit>,
}

#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(C)]
pub struct SearchHit {
    /// Vault-relative page path.
    pub path: String,
    /// Page title.
    pub title: String,
    /// Short surrounding-context excerpt. Single line, ≤ 200
    /// chars.
    pub snippet: String,
    /// Full markdown — only populated when
    /// `SearchOpts::include_content` was set.
    pub content: String,
    /// Combined score. Comparable within one response;
    /// not comparable across calls.
    pub score: f32,
    /// Matched query terms surfaced for highlight rendering.
    pub matched_terms: Vec<String>,
}
