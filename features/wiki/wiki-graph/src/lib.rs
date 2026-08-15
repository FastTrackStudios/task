//! `wiki-graph` — pure-computation graph layer for the
//! wiki feature.
//!
//! Walks `<vault>/Wiki/`, parses frontmatter + wikilinks,
//! and builds nodes + 4-signal-weighted edges.
//!
//! ## Signals (matches `llm_wiki` defaults)
//!
//! | Signal          | Weight | Condition                            |
//! |-----------------|--------|--------------------------------------|
//! | Direct link     | ×3.0   | `[[wikilink]]` from A → B            |
//! | Source overlap  | ×4.0   | A & B share an entry in `sources:`   |
//! | Adamic-Adar     | ×1.5   | Σ 1/log(deg(n)) over shared neighbors |
//! | Type affinity   | ×1.0   | Same `type:` frontmatter             |
//!
//! ## Gap kinds today
//!
//! - **Orphan** — node with degree ≤ 1.
//! - **Missing page** — wikilink target with no matching
//!   page on disk (the link references a concept that
//!   doesn't exist as a `.md` file).
//!
//! Louvain clustering + sparse-cluster + bridge-node gaps
//! land in a follow-up.
//!
//! ## Output
//!
//! Built around the wire types in `wiki_proto::graph`
//! (`WikiGraph`, `GraphNode`, `GraphEdge`,
//! `RelevanceScore`, `KnowledgeGap`). Backends mounting
//! `wiki_proto::WikiService` plug this in for the
//! `build_graph` / `gaps` methods.

pub mod analyze;
mod build;
pub mod code_extract;
mod context;
mod gaps;
mod louvain;
mod parse;
mod scan;
mod scoring;
mod tier_lint;

pub use analyze::{
    AffectedResult, AnalyzedGraph, affected_by, analyze, fan_out_hubs, god_nodes, kind_counts,
    relation_counts, render_report,
};
pub use build::{build_clusters, build_graph};
pub use code_extract::{
    CodeEdge, CodeExtractError, CodeExtraction, CodeLang, CodeNode, Confidence, Relation,
    SymbolKind, extract_file, extract_source, scan_code_tree,
};
pub use context::{ContextOpts, ContextResult, NOTE_ID_PREFIX, build_context};
pub use gaps::find_gaps;
pub use parse::extract_wikilinks;
pub use scan::ScanError;
pub use tier_lint::{Tier, TierViolation, lint_org_tree};
