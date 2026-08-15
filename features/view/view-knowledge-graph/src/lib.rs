//! Knowledge-graph view — a force-directed map of a wiki's `[[wikilink]]`
//! web, ported from nashsu/llm_wiki's `features/view` (graph-view.tsx +
//! the `graph-*` libs).
//!
//! The TS original is built on graphology (graph model + Louvain),
//! forceAtlas2 (layout, in a web worker), and sigma (WebGL rendering).
//! This crate replaces that whole stack with self-contained Rust:
//!
//! - [`build::build_wiki_graph`] turns a list of [`parse::WikiFile`]s
//!   into a [`model::WikiGraph`] (nodes, relevance-weighted edges,
//!   detected communities). It is **pure and FS-free** — the caller
//!   reads the `wiki/` tree and hands contents in, so the same code
//!   builds on `wasm32` and is trivially unit-tested.
//! - [`relevance`], [`community`], [`filters`], [`search`], and
//!   [`insights`] port the corresponding `graph-*.ts` analyses.
//! - [`layout`] is a deterministic Fruchterman–Reingold force layout
//!   (no RNG, no worker) — fast enough to run synchronously.
//! - [`components::KnowledgeGraphView`] is the dumb Dioxus SVG renderer:
//!   pass a [`model::WikiGraph`] in, get node-click events out.
//!
//! ## Typical wiring
//!
//! ```ignore
//! use view_knowledge_graph::{build_wiki_graph, parse::WikiFile, KnowledgeGraphView};
//!
//! let files: Vec<WikiFile> = read_wiki_dir(&org);     // caller's FS walk
//! let graph = build_wiki_graph(&files);
//!
//! rsx! {
//!     KnowledgeGraphView {
//!         graph,
//!         on_node_click: move |id: String| open_page(id),
//!     }
//! }
//! ```

pub mod build;
pub mod community;
pub mod components;
pub mod filters;
pub mod insights;
pub mod layout;
pub mod link_build;
pub mod model;
pub mod parse;
pub mod relevance;
pub mod search;

pub use build::build_wiki_graph;
pub use components::{
    GraphFilters, GraphFiltersProps, GraphLegend, GraphLegendProps, KnowledgeGraphView,
    KnowledgeGraphViewProps,
};
pub use filters::{FilteredGraph, GraphFilterState, apply_filters};
pub use insights::{
    GapKind, KnowledgeGap, SurprisingConnection, knowledge_gaps, surprising_connections,
};
pub use link_build::build_link_graph;
pub use model::{ColorMode, CommunityInfo, GraphEdge, GraphNode, NodeKind, WikiGraph};
pub use parse::WikiFile;
pub use search::{SearchResult, apply_search};
