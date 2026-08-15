//! 4-signal relevance graph + Louvain clusters.
//!
//! Mirrors `llm_wiki`'s graph model (see `api_server.rs::handle_graph`
//! in `nashsu/llm_wiki`). Nodes are wiki pages keyed by their
//! vault-relative path; edges are scored by combining four signals:
//!
//! | Signal          | Weight | Source                                      |
//! |-----------------|--------|---------------------------------------------|
//! | Direct link     | ×3.0   | `[[wikilink]]` from page A → page B         |
//! | Source overlap  | ×4.0   | Both cite the same entry in `sources:`      |
//! | Adamic-Adar     | ×1.5   | Σ 1/log(degree(n)) over shared neighbors    |
//! | Type affinity   | ×1.0   | Both have the same `type:` frontmatter      |
//!
//! Backends are free to expose tunable weights, but the
//! defaults match `llm_wiki` for portability.

use facet::Facet;

/// Options for [`crate::service::WikiService::build_graph`].
#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(C)]
pub struct GraphOpts {
    /// Optional substring filter applied to titles. Empty =
    /// no filter.
    pub query: String,
    /// Optional `type:` filter. Empty = all types.
    pub node_type: String,
    /// Cap on the number of nodes returned. Backends sort by
    /// degree descending, then truncate. `0` = no cap.
    pub limit: u32,
    /// Override the default weights. `None` ⇒ defaults.
    pub weights: Option<RelevanceWeights>,
}

/// Per-signal weights. Defaults match `llm_wiki` ×{3, 4, 1.5, 1}.
#[derive(Debug, Clone, Copy, PartialEq, Facet)]
#[repr(C)]
pub struct RelevanceWeights {
    pub direct_link: f32,
    pub source_overlap: f32,
    pub adamic_adar: f32,
    pub type_affinity: f32,
}

impl Default for RelevanceWeights {
    fn default() -> Self {
        Self {
            direct_link: 3.0,
            source_overlap: 4.0,
            adamic_adar: 1.5,
            type_affinity: 1.0,
        }
    }
}

/// Snapshot returned from [`crate::service::WikiService::build_graph`].
#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(C)]
pub struct WikiGraph {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
pub struct GraphNode {
    /// Stable id — vault-relative path (e.g.
    /// `Wiki/Concepts/Spaced repetition.md`).
    pub id: String,
    /// Page title (frontmatter `title:` or basename).
    pub label: String,
    /// Page `type:` (`entity`, `concept`, `source`, ...).
    pub node_type: String,
    /// Combined inbound + outbound degree.
    pub link_count: u32,
    /// Louvain community id, if [`WikiGraph`] came from a
    /// clustered call. Empty string otherwise.
    pub community: String,
}

#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(C)]
pub struct GraphEdge {
    pub source: String,
    pub target: String,
    /// Combined weight after applying [`RelevanceWeights`].
    pub weight: f32,
    /// Per-signal breakdown. Useful for surfacing *why* the
    /// edge exists in UI tooltips.
    pub signals: RelevanceScore,
}

/// Per-signal breakdown of a single edge's weight.
#[derive(Debug, Clone, Copy, PartialEq, Facet)]
#[repr(C)]
pub struct RelevanceScore {
    pub direct_link: f32,
    pub source_overlap: f32,
    pub adamic_adar: f32,
    pub type_affinity: f32,
    pub total: f32,
}

/// One Louvain community.
#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(C)]
pub struct Cluster {
    /// Stable id — backend-assigned, opaque.
    pub id: String,
    /// Human-facing label (LLM-generated or backend heuristic).
    pub name: String,
    /// Member page paths.
    pub members: Vec<String>,
    /// Cohesion score = actual edges / possible edges within
    /// the cluster. Range `[0, 1]`. Below ~0.15 ⇒ sparse,
    /// flagged as a knowledge gap.
    pub cohesion: f32,
}

/// A knowledge gap surfaced by the graph analysis. Becomes
/// fodder for [`crate::research::ResearchPlan`].
#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(C)]
pub struct KnowledgeGap {
    pub id: String,
    pub kind: GapKind,
    /// Affected page paths or cluster id, depending on
    /// `kind`.
    pub subjects: Vec<String>,
    /// LLM-readable explanation.
    pub explanation: String,
}

#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(C)]
pub enum GapKind {
    /// Page with degree ≤ 1 — nothing links to it.
    Orphan,
    /// Cluster with cohesion < threshold (default 0.15).
    SparseCluster,
    /// High-betweenness bridge node — losing it would
    /// disconnect a community. Worth fleshing out.
    Bridge,
    /// Concept mentioned in N pages but lacks its own page.
    MissingPage,
}
