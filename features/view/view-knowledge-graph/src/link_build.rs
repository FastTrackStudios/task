//! Build a graph from typed links — the verse↔verse↔entity↔note graph.
//!
//! Where [`crate::build`] builds the wiki's wikilink web, this builds from
//! the generalized [`links_proto::TypedLink`] store: each link becomes a
//! [`GraphEdge`] carrying its `relation` + `confidence`, and each
//! endpoint a [`GraphNode`]. Combined with the confidence / relation
//! filters, this is the quality-filtered knowledge graph ("show only the
//! strongly-established links").

use std::collections::BTreeMap;

use links_proto::{NodeRef, TypedLink, Visibility};

use crate::model::{CommunityInfo, GraphEdge, GraphNode, WikiGraph};

/// Build a [`WikiGraph`] from typed links. When `include_private` is
/// false, private links (and endpoints only reachable through them) are
/// dropped — the publishable view.
#[must_use]
pub fn build_link_graph(links: &[TypedLink], include_private: bool) -> WikiGraph {
    let mut nodes: BTreeMap<String, GraphNode> = BTreeMap::new();
    let mut degree: BTreeMap<String, u32> = BTreeMap::new();
    let mut edges = Vec::new();

    for link in links {
        if !include_private && link.visibility == Visibility::Private {
            continue;
        }
        let s = node_id(&link.source);
        let t = node_id(&link.target);
        nodes.entry(s.clone()).or_insert_with(|| node(&link.source));
        nodes.entry(t.clone()).or_insert_with(|| node(&link.target));
        *degree.entry(s.clone()).or_default() += 1;
        *degree.entry(t.clone()).or_default() += 1;

        let confidence = link.confidence as u8;
        edges.push(GraphEdge {
            source: s,
            target: t,
            // Weight scales with confidence (0.2 .. 1.0) for layout.
            weight: (f32::from(confidence) + 1.0) / 5.0,
            relation: link.relation.as_str().to_string(),
            confidence: Some(confidence),
        });
    }

    let nodes: Vec<GraphNode> = nodes
        .into_values()
        .map(|mut n| {
            n.link_count = degree.get(&n.id).copied().unwrap_or(0);
            n
        })
        .collect();

    WikiGraph {
        nodes,
        edges,
        communities: Vec::<CommunityInfo>::new(),
    }
}

/// Stable node id — the `kind:id` token (`verse:John.3.16`).
fn node_id(r: &NodeRef) -> String {
    r.to_token()
}

/// A graph node for an endpoint. `kind` is the node-kind string (so the
/// palette colors verses/entities/notes distinctly); `label` is the id.
fn node(r: &NodeRef) -> GraphNode {
    GraphNode {
        id: r.to_token(),
        label: r.id.clone(),
        kind: r.kind.as_str().to_string(),
        path: String::new(),
        link_count: 0,
        community: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use links_proto::{Confidence, NodeKind, Relation};

    fn link(from: &str, to: &str, rel: Relation, conf: Confidence, vis: Visibility) -> TypedLink {
        let mut l = TypedLink::new(NodeRef::verse(from), NodeRef::verse(to), rel, conf);
        l.visibility = vis;
        l
    }

    #[test]
    fn builds_nodes_edges_with_relation_and_confidence() {
        let links = vec![
            link(
                "John.3.16",
                "Romans.5.8",
                Relation::CrossRef,
                Confidence::Likely,
                Visibility::Public,
            ),
            link(
                "John.3.16",
                "Genesis.1.1",
                Relation::Related,
                Confidence::Speculative,
                Visibility::Private,
            ),
        ];
        // Public-only: drops the private edge and its dangling endpoint.
        let g = build_link_graph(&links, false);
        assert_eq!(g.edges.len(), 1);
        assert_eq!(g.edges[0].relation, "cross-ref");
        assert_eq!(g.edges[0].confidence, Some(Confidence::Likely as u8));
        assert_eq!(g.nodes.len(), 2);
        assert!(
            g.nodes
                .iter()
                .any(|n| n.id == "verse:John.3.16" && n.kind == "verse")
        );

        // Including private: both edges + all three nodes.
        let full = build_link_graph(&links, true);
        assert_eq!(full.edges.len(), 2);
        assert_eq!(full.nodes.len(), 3);
        // Degree of the hub node is 2.
        let hub = full
            .nodes
            .iter()
            .find(|n| n.id == "verse:John.3.16")
            .unwrap();
        assert_eq!(hub.link_count, 2);
    }

    #[test]
    fn node_kinds_distinguish_endpoints() {
        let l = TypedLink::new(
            NodeRef::verse("John.3.16"),
            NodeRef::new(NodeKind::Wiki, "Entities/Paul.md"),
            Relation::Mentions,
            Confidence::Certain,
        );
        let g = build_link_graph(&[l], true);
        assert!(g.nodes.iter().any(|n| n.kind == "verse"));
        assert!(g.nodes.iter().any(|n| n.kind == "wiki"));
    }
}
