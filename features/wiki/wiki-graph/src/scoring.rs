//! 4-signal edge scoring, applied to the page set produced
//! by [`super::scan`].
//!
//! Operates on a `&[Page]` slice + precomputed lookup
//! tables (title → page, source → pages). Each `(a, b)`
//! pair gets one [`wiki_proto::graph::RelevanceScore`]
//! summing the four contributions, then collapsed into a
//! [`wiki_proto::graph::GraphEdge`].
//!
//! Edges are undirected for scoring purposes — the
//! `direction` of a wikilink contributes the same weight
//! either way. Resolution always uses lowercase-title
//! matching to mimic Obsidian's case-insensitive linking.

use std::collections::{HashMap, HashSet};

use wiki_proto::graph::{GraphEdge, GraphNode, RelevanceScore, RelevanceWeights, WikiGraph};

use crate::parse::Page;

/// Precomputed lookup tables built once per scan.
pub(crate) struct Indices {
    /// title (lowercased) → index in `pages`.
    pub(crate) by_title: HashMap<String, usize>,
    /// page-stem (lowercased) → index — Obsidian lets you
    /// link by either title or filename stem.
    pub(crate) by_stem: HashMap<String, usize>,
    /// source-id → indices that cite it.
    #[allow(dead_code)]
    pub(crate) by_source: HashMap<String, Vec<usize>>,
    /// Outbound edges (sender → receivers) resolved into
    /// indices. Unresolved targets are dropped.
    pub(crate) outedges: Vec<HashSet<usize>>,
    /// Undirected neighbor sets (union of outedges
    /// in either direction).
    pub(crate) neighbors: Vec<HashSet<usize>>,
}

impl Indices {
    pub(crate) fn build(pages: &[Page]) -> Self {
        let mut by_title = HashMap::new();
        let mut by_stem = HashMap::new();
        let mut by_source: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, p) in pages.iter().enumerate() {
            by_title.insert(p.title.to_lowercase(), i);
            if let Some(stem) = std::path::Path::new(&p.rel_path)
                .file_stem()
                .and_then(|s| s.to_str())
            {
                by_stem.insert(stem.to_lowercase(), i);
            }
            for src in &p.sources {
                by_source.entry(src.clone()).or_default().push(i);
            }
        }

        let mut outedges: Vec<HashSet<usize>> = vec![HashSet::new(); pages.len()];
        for (i, p) in pages.iter().enumerate() {
            for target in &p.outlinks {
                let key = target.to_lowercase();
                if let Some(&j) = by_title.get(&key).or_else(|| by_stem.get(&key)) {
                    if i != j {
                        outedges[i].insert(j);
                    }
                }
            }
        }

        let mut neighbors: Vec<HashSet<usize>> = vec![HashSet::new(); pages.len()];
        for (i, outs) in outedges.iter().enumerate() {
            for &j in outs {
                neighbors[i].insert(j);
                neighbors[j].insert(i);
            }
        }

        Self {
            by_title,
            by_stem,
            by_source,
            outedges,
            neighbors,
        }
    }
}

/// Compute the full graph snapshot.
pub(crate) fn score_graph(
    pages: &[Page],
    idx: &Indices,
    weights: &RelevanceWeights,
    query: &str,
    node_type: &str,
) -> WikiGraph {
    let mut nodes: Vec<GraphNode> = pages
        .iter()
        .enumerate()
        .map(|(i, p)| GraphNode {
            id: p.rel_path.clone(),
            label: p.title.clone(),
            node_type: p.page_type.clone(),
            link_count: idx.neighbors[i].len() as u32,
            community: String::new(),
        })
        .collect();

    let mut edges = Vec::new();
    for i in 0..pages.len() {
        for &j in &idx.neighbors[i] {
            if i >= j {
                continue;
            }
            let score = signal_score(pages, idx, i, j, weights);
            if score.total <= 0.0 {
                continue;
            }
            edges.push(GraphEdge {
                source: pages[i].rel_path.clone(),
                target: pages[j].rel_path.clone(),
                weight: score.total,
                signals: score,
            });
        }
    }

    // Filter by node_type and query.
    if !node_type.is_empty() {
        nodes.retain(|n| n.node_type == node_type);
    }
    if !query.is_empty() {
        let needle = query.to_lowercase();
        nodes.retain(|n| {
            n.label.to_lowercase().contains(&needle) || n.id.to_lowercase().contains(&needle)
        });
    }
    let kept: HashSet<String> = nodes.iter().map(|n| n.id.clone()).collect();
    edges.retain(|e| kept.contains(&e.source) && kept.contains(&e.target));

    WikiGraph { nodes, edges }
}

/// Build the `(i, j) → total_weight` matrix used by
/// `louvain` (and any future graph algorithm). Same
/// scoring as `score_graph` but keyed for direct lookup
/// instead of producing `GraphEdge`s.
pub(crate) fn edge_weight_matrix(
    pages: &[Page],
    idx: &Indices,
    weights: &RelevanceWeights,
) -> HashMap<(usize, usize), f32> {
    let mut out = HashMap::new();
    for i in 0..pages.len() {
        for &j in &idx.neighbors[i] {
            if i >= j {
                continue;
            }
            let score = signal_score(pages, idx, i, j, weights);
            if score.total > 0.0 {
                out.insert((i, j), score.total);
            }
        }
    }
    out
}

fn signal_score(
    pages: &[Page],
    idx: &Indices,
    i: usize,
    j: usize,
    w: &RelevanceWeights,
) -> RelevanceScore {
    // Direct link — fires once if either direction has a
    // `[[wikilink]]`.
    let direct_link = if idx.outedges[i].contains(&j) || idx.outedges[j].contains(&i) {
        w.direct_link
    } else {
        0.0
    };

    // Source overlap — count of shared `sources:` entries.
    let pa: HashSet<&String> = pages[i].sources.iter().collect();
    let pb: HashSet<&String> = pages[j].sources.iter().collect();
    let shared_sources = pa.intersection(&pb).count();
    let source_overlap = (shared_sources as f32) * w.source_overlap;

    // Adamic-Adar over shared *page* neighbors.
    let shared_neighbors: Vec<&usize> = idx.neighbors[i].intersection(&idx.neighbors[j]).collect();
    let mut aa = 0.0_f32;
    for &n in shared_neighbors {
        let deg = idx.neighbors[n].len();
        if deg > 1 {
            aa += 1.0 / (deg as f32).ln();
        }
    }
    let adamic_adar = aa * w.adamic_adar;

    let type_affinity = if pages[i].page_type == pages[j].page_type {
        w.type_affinity
    } else {
        0.0
    };

    let total = direct_link + source_overlap + adamic_adar + type_affinity;
    RelevanceScore {
        direct_link,
        source_overlap,
        adamic_adar,
        type_affinity,
        total,
    }
}
