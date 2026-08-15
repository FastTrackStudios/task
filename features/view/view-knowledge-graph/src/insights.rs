//! Graph insights — port of `graph-insights.ts`.
//!
//! Two analyses over the built graph:
//!   - [`surprising_connections`] — edges that cross communities/types
//!     or couple peripheral nodes to hubs.
//!   - [`knowledge_gaps`] — isolated pages, sparse clusters, bridges.

use std::collections::{HashMap, HashSet};

use crate::model::{CommunityInfo, GraphEdge, GraphNode};

/// An edge flagged as unexpectedly interesting.
#[derive(Debug, Clone, PartialEq)]
pub struct SurprisingConnection {
    pub source: GraphNode,
    pub target: GraphNode,
    pub score: i32,
    pub reasons: Vec<String>,
    /// Stable key (sorted endpoint ids) for dismiss tracking.
    pub key: String,
}

/// The category of a detected gap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GapKind {
    IsolatedNode,
    SparseCommunity,
    BridgeNode,
}

/// A structural weakness worth the user's attention.
#[derive(Debug, Clone, PartialEq)]
pub struct KnowledgeGap {
    pub kind: GapKind,
    pub title: String,
    pub description: String,
    pub node_ids: Vec<String>,
    pub suggestion: String,
}

const STRUCTURAL_IDS: &[&str] = &["index", "log", "overview"];

/// Find up to `limit` surprising connections, highest score first.
pub fn surprising_connections(
    nodes: &[GraphNode],
    edges: &[GraphEdge],
    limit: usize,
) -> Vec<SurprisingConnection> {
    let node_map: HashMap<&str, &GraphNode> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let max_degree = nodes.iter().map(|n| n.link_count).max().unwrap_or(1).max(1) as f32;

    let distant_pairs: HashSet<&str> = [
        "source-concept",
        "concept-source",
        "source-synthesis",
        "synthesis-source",
        "query-entity",
        "entity-query",
    ]
    .into_iter()
    .collect();

    let mut scored: Vec<SurprisingConnection> = Vec::new();
    for e in edges {
        let (Some(&source), Some(&target)) = (
            node_map.get(e.source.as_str()),
            node_map.get(e.target.as_str()),
        ) else {
            continue;
        };
        if STRUCTURAL_IDS.contains(&source.id.as_str())
            || STRUCTURAL_IDS.contains(&target.id.as_str())
        {
            continue;
        }

        let mut score = 0;
        let mut reasons: Vec<String> = Vec::new();

        // Signal 1: cross-community.
        if source.community != target.community {
            score += 3;
            reasons.push("crosses community boundary".to_string());
        }

        // Signal 2: cross-type.
        if source.kind != target.kind {
            let pair = format!("{}-{}", source.kind, target.kind);
            if distant_pairs.contains(pair.as_str()) {
                score += 2;
                reasons.push(format!("connects {} to {}", source.kind, target.kind));
            } else {
                score += 1;
                reasons.push("different types".to_string());
            }
        }

        // Signal 3: peripheral ↔ hub.
        let min_deg = source.link_count.min(target.link_count);
        let max_deg = source.link_count.max(target.link_count) as f32;
        if min_deg <= 2 && max_deg >= max_degree * 0.5 {
            score += 2;
            reasons.push("peripheral node links to hub".to_string());
        }

        // Signal 4: weak-but-present edge.
        if e.weight < 2.0 && e.weight > 0.0 {
            score += 1;
            reasons.push("weak but present connection".to_string());
        }

        if score >= 3 && !reasons.is_empty() {
            let mut pair = [source.id.clone(), target.id.clone()];
            pair.sort();
            scored.push(SurprisingConnection {
                source: (*source).clone(),
                target: (*target).clone(),
                score,
                reasons,
                key: pair.join(":::"),
            });
        }
    }

    scored.sort_by(|a, b| b.score.cmp(&a.score));
    scored.truncate(limit);
    scored
}

/// Detect up to `limit` knowledge gaps.
pub fn knowledge_gaps(
    nodes: &[GraphNode],
    edges: &[GraphEdge],
    communities: &[CommunityInfo],
    limit: usize,
) -> Vec<KnowledgeGap> {
    let node_map: HashMap<&str, &GraphNode> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut gaps: Vec<KnowledgeGap> = Vec::new();

    // 1. Isolated nodes (degree <= 1).
    let isolated: Vec<&GraphNode> = nodes
        .iter()
        .filter(|n| n.link_count <= 1 && n.kind != "overview" && n.id != "index" && n.id != "log")
        .collect();
    if !isolated.is_empty() {
        let top: Vec<&str> = isolated.iter().take(5).map(|n| n.label.as_str()).collect();
        let mut desc = top.join(", ");
        if isolated.len() > 5 {
            desc.push_str(&format!(" and {} more", isolated.len() - 5));
        }
        gaps.push(KnowledgeGap {
            kind: GapKind::IsolatedNode,
            title: format!(
                "{} isolated page{}",
                isolated.len(),
                if isolated.len() > 1 { "s" } else { "" }
            ),
            description: desc,
            node_ids: isolated.iter().map(|n| n.id.clone()).collect(),
            suggestion: "These pages have few or no connections. Consider adding [[wikilinks]] to related pages, or research to expand their content.".to_string(),
        });
    }

    // 2. Sparse communities.
    for c in communities {
        if c.cohesion < 0.15 && c.node_count >= 3 {
            let title = c
                .top_nodes
                .first()
                .cloned()
                .unwrap_or_else(|| format!("Community {}", c.id));
            gaps.push(KnowledgeGap {
                kind: GapKind::SparseCommunity,
                title: format!("Sparse cluster: {title}"),
                description: format!(
                    "{} pages with cohesion {:.2} — internal connections are weak.",
                    c.node_count, c.cohesion
                ),
                node_ids: nodes
                    .iter()
                    .filter(|n| n.community == c.id)
                    .map(|n| n.id.clone())
                    .collect(),
                suggestion: "This knowledge area lacks internal cross-references. Consider adding links between these pages or researching to fill gaps.".to_string(),
            });
        }
    }

    // 3. Bridge nodes (touch >= 3 communities).
    let mut comm_neighbors: HashMap<&str, HashSet<u32>> = nodes
        .iter()
        .map(|n| (n.id.as_str(), HashSet::new()))
        .collect();
    for e in edges {
        if let (Some(&s), Some(&t)) = (
            node_map.get(e.source.as_str()),
            node_map.get(e.target.as_str()),
        ) {
            comm_neighbors
                .get_mut(e.source.as_str())
                .map(|set| set.insert(t.community));
            comm_neighbors
                .get_mut(e.target.as_str())
                .map(|set| set.insert(s.community));
        }
    }
    let mut bridges: Vec<&GraphNode> = nodes
        .iter()
        .filter(|n| {
            !STRUCTURAL_IDS.contains(&n.id.as_str())
                && comm_neighbors.get(n.id.as_str()).map_or(0, |s| s.len()) >= 3
        })
        .collect();
    bridges.sort_by(|a, b| {
        let bc = comm_neighbors.get(b.id.as_str()).map_or(0, |s| s.len());
        let ac = comm_neighbors.get(a.id.as_str()).map_or(0, |s| s.len());
        bc.cmp(&ac)
    });
    for bridge in bridges.into_iter().take(3) {
        let count = comm_neighbors
            .get(bridge.id.as_str())
            .map_or(0, |s| s.len());
        gaps.push(KnowledgeGap {
            kind: GapKind::BridgeNode,
            title: format!("Key bridge: {}", bridge.label),
            description: format!(
                "Connects {count} different knowledge clusters. This is a critical junction in your wiki."
            ),
            node_ids: vec![bridge.id.clone()],
            suggestion: "This page bridges multiple knowledge areas. Ensure it's well-maintained — if it's thin, expanding it will strengthen your entire wiki.".to_string(),
        });
    }

    gaps.truncate(limit);
    gaps
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, kind: &str, community: u32, links: u32) -> GraphNode {
        GraphNode {
            id: id.to_string(),
            label: id.to_string(),
            kind: kind.to_string(),
            path: format!("/wiki/{id}.md"),
            link_count: links,
            community,
        }
    }

    #[test]
    fn cross_community_edge_is_surprising() {
        let nodes = vec![node("a", "concept", 0, 5), node("b", "entity", 1, 5)];
        let edges = vec![GraphEdge::wikilink("a".into(), "b".into(), 5.0)];
        let s = surprising_connections(&nodes, &edges, 5);
        assert_eq!(s.len(), 1);
        assert!(s[0].score >= 3);
    }

    #[test]
    fn isolated_node_is_a_gap() {
        let nodes = vec![node("lonely", "entity", 0, 0), node("hub", "entity", 0, 4)];
        let gaps = knowledge_gaps(&nodes, &[], &[], 8);
        assert!(gaps.iter().any(|g| g.kind == GapKind::IsolatedNode));
    }
}
