//! Graph search — port of `graph-search.ts`. Whitespace-split AND match
//! over `label`, `id`, `kind`, and `path` (all lowercased).

use std::collections::HashSet;

use crate::model::{GraphEdge, GraphNode};

/// Search result: matched nodes, the edges between them, and the matched
/// id set (used by the renderer to highlight).
#[derive(Debug, Clone, PartialEq)]
pub struct SearchResult {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub matched_ids: HashSet<String>,
}

/// Apply a search query. An empty/whitespace query returns everything
/// with an empty `matched_ids` (i.e. "no active search").
pub fn apply_search(nodes: &[GraphNode], edges: &[GraphEdge], query: &str) -> SearchResult {
    let tokens: Vec<String> = query
        .to_ascii_lowercase()
        .split_whitespace()
        .map(|t| t.to_string())
        .collect();

    if tokens.is_empty() {
        return SearchResult {
            nodes: nodes.to_vec(),
            edges: edges.to_vec(),
            matched_ids: HashSet::new(),
        };
    }

    let mut matched_ids = HashSet::new();
    let matched: Vec<GraphNode> = nodes
        .iter()
        .filter(|n| {
            let haystack =
                format!("{} {} {} {}", n.label, n.id, n.kind, n.path).to_ascii_lowercase();
            let hit = tokens.iter().all(|tok| haystack.contains(tok.as_str()));
            if hit {
                matched_ids.insert(n.id.clone());
            }
            hit
        })
        .cloned()
        .collect();

    let visible: HashSet<&str> = matched.iter().map(|n| n.id.as_str()).collect();
    let visible_edges: Vec<GraphEdge> = edges
        .iter()
        .filter(|e| visible.contains(e.source.as_str()) && visible.contains(e.target.as_str()))
        .cloned()
        .collect();

    SearchResult {
        nodes: matched,
        edges: visible_edges,
        matched_ids,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, label: &str, kind: &str) -> GraphNode {
        GraphNode {
            id: id.to_string(),
            label: label.to_string(),
            kind: kind.to_string(),
            path: format!("/wiki/{id}.md"),
            link_count: 0,
            community: 0,
        }
    }

    #[test]
    fn empty_query_returns_all_no_matches() {
        let nodes = vec![node("a", "Alpha", "entity")];
        let r = apply_search(&nodes, &[], "  ");
        assert_eq!(r.nodes.len(), 1);
        assert!(r.matched_ids.is_empty());
    }

    #[test]
    fn and_match_across_fields() {
        let nodes = vec![
            node("acme", "Acme Corp", "entity"),
            node("beta", "Beta", "concept"),
        ];
        let r = apply_search(&nodes, &[], "acme entity");
        assert_eq!(r.nodes.len(), 1);
        assert!(r.matched_ids.contains("acme"));
    }
}
