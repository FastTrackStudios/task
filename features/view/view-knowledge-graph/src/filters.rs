//! Visibility filtering — port of `graph-filters.ts` + `graph-visibility.ts`.

use std::collections::HashSet;

use crate::model::{GraphEdge, GraphNode};

/// Which nodes the user has chosen to hide.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphFilterState {
    /// Hide nodes whose `kind` is in this set.
    pub hidden_kinds: HashSet<String>,
    /// Hide nodes with these explicit ids.
    pub hidden_node_ids: HashSet<String>,
    /// Hide structural pages (index/overview/log/schema/purpose).
    pub hide_structural: bool,
    /// Hide nodes with no links (`link_count == 0`).
    pub hide_isolated: bool,
    /// Hide nodes with `link_count` above this cap.
    pub max_links: Option<u32>,
    /// Hide rated edges below this ordinal confidence (`0..=4`). Unrated
    /// (wikilink) edges are unaffected — gate those with `typed_only`.
    pub min_confidence: Option<u8>,
    /// Show only typed/rated edges — hide plain wikilinks. Combined with
    /// `min_confidence`, this is the "only strongly-established links" view.
    pub typed_only: bool,
    /// Hide edges with these relation types.
    pub hidden_relations: HashSet<String>,
}

impl Default for GraphFilterState {
    /// Matches `DEFAULT_GRAPH_FILTERS`: structural hidden, rest shown.
    fn default() -> Self {
        Self {
            hidden_kinds: HashSet::new(),
            hidden_node_ids: HashSet::new(),
            hide_structural: true,
            hide_isolated: false,
            max_links: None,
            min_confidence: None,
            typed_only: false,
            hidden_relations: HashSet::new(),
        }
    }
}

impl GraphFilterState {
    /// True when any filter would remove something.
    pub fn is_active(&self) -> bool {
        self.hide_structural
            || self.hide_isolated
            || !self.hidden_kinds.is_empty()
            || !self.hidden_node_ids.is_empty()
            || self.max_links.is_some()
            || self.min_confidence.is_some()
            || self.typed_only
            || !self.hidden_relations.is_empty()
    }

    /// Whether an edge survives the relation / confidence filters.
    #[must_use]
    pub fn edge_passes(&self, edge: &GraphEdge) -> bool {
        if self.hidden_relations.contains(&edge.relation) {
            return false;
        }
        if self.typed_only && edge.confidence.is_none() {
            return false;
        }
        if let (Some(min), Some(c)) = (self.min_confidence, edge.confidence) {
            if c < min {
                return false;
            }
        }
        true
    }
}

/// Result of applying filters: the surviving slice + which ids were cut.
#[derive(Debug, Clone, PartialEq)]
pub struct FilteredGraph {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub hidden_node_ids: HashSet<String>,
}

/// Ids treated as structural regardless of path.
const STRUCTURAL_IDS: &[&str] = &["index", "overview", "log", "schema", "purpose"];

/// True for index/overview/log/schema/purpose pages — the connective
/// scaffolding that links to everything and clutters the map.
pub fn is_structural(node: &GraphNode) -> bool {
    let id = node.id.to_ascii_lowercase();
    if STRUCTURAL_IDS.contains(&id.as_str()) {
        return true;
    }
    if node.kind == "overview" {
        return true;
    }
    let p = node.path.replace('\\', "/").to_ascii_lowercase();
    p.ends_with("/wiki/index.md")
        || p.ends_with("/wiki/overview.md")
        || p.ends_with("/wiki/log.md")
        || p.ends_with("/purpose.md")
        || p.ends_with("/schema.md")
}

/// Apply `filters` to a graph, dropping edges whose endpoints vanish.
pub fn apply_filters(
    nodes: &[GraphNode],
    edges: &[GraphEdge],
    filters: &GraphFilterState,
) -> FilteredGraph {
    let mut hidden: HashSet<String> = HashSet::new();
    for node in nodes {
        if filters.hidden_node_ids.contains(&node.id)
            || filters.hidden_kinds.contains(&node.kind)
            || (filters.hide_structural && is_structural(node))
            || (filters.hide_isolated && node.link_count == 0)
            || filters.max_links.is_some_and(|m| node.link_count > m)
        {
            hidden.insert(node.id.clone());
        }
    }

    let visible_nodes: Vec<GraphNode> = nodes
        .iter()
        .filter(|n| !hidden.contains(&n.id))
        .cloned()
        .collect();
    let visible_ids: HashSet<&str> = visible_nodes.iter().map(|n| n.id.as_str()).collect();
    let visible_edges: Vec<GraphEdge> = edges
        .iter()
        .filter(|e| {
            visible_ids.contains(e.source.as_str())
                && visible_ids.contains(e.target.as_str())
                && filters.edge_passes(e)
        })
        .cloned()
        .collect();

    FilteredGraph {
        nodes: visible_nodes,
        edges: visible_edges,
        hidden_node_ids: hidden,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, kind: &str, links: u32) -> GraphNode {
        GraphNode {
            id: id.to_string(),
            label: id.to_string(),
            kind: kind.to_string(),
            path: format!("/wiki/{id}.md"),
            link_count: links,
            community: 0,
        }
    }

    #[test]
    fn default_hides_structural_only() {
        let nodes = vec![node("index", "overview", 9), node("acme", "entity", 2)];
        let edges = vec![];
        let f = apply_filters(&nodes, &edges, &GraphFilterState::default());
        assert_eq!(f.nodes.len(), 1);
        assert_eq!(f.nodes[0].id, "acme");
    }

    fn edge(source: &str, target: &str, relation: &str, confidence: Option<u8>) -> GraphEdge {
        GraphEdge {
            source: source.into(),
            target: target.into(),
            weight: 1.0,
            relation: relation.into(),
            confidence,
        }
    }

    #[test]
    fn confidence_and_relation_edge_filters() {
        let nodes = vec![
            node("a", "verse", 3),
            node("b", "verse", 1),
            node("c", "verse", 1),
        ];
        let edges = vec![
            edge("a", "b", "cross-ref", Some(4)), // certain typed
            edge("a", "c", "related", Some(1)),   // speculative typed
            edge("b", "c", "", None),             // plain wikilink
        ];
        // Strongly-established view: typed only, confidence >= likely(3).
        let mut f = GraphFilterState {
            typed_only: true,
            min_confidence: Some(3),
            ..Default::default()
        };
        f.hide_structural = false;
        let g = apply_filters(&nodes, &edges, &f);
        assert_eq!(g.edges.len(), 1);
        assert_eq!(g.edges[0].relation, "cross-ref");

        // Hide a relation type.
        let mut f2 = GraphFilterState {
            hide_structural: false,
            ..Default::default()
        };
        f2.hidden_relations.insert("related".into());
        assert_eq!(apply_filters(&nodes, &edges, &f2).edges.len(), 2);
    }

    #[test]
    fn isolated_filter() {
        let nodes = vec![node("a", "entity", 0), node("b", "entity", 3)];
        let filt = GraphFilterState {
            hide_isolated: true,
            ..Default::default()
        };
        let f = apply_filters(&nodes, &[], &filt);
        assert_eq!(f.nodes.len(), 1);
        assert_eq!(f.nodes[0].id, "b");
    }
}
