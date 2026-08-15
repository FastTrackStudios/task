//! Wiki → graph builder — port of `buildWikiGraph` in `wiki-graph.ts`.
//!
//! Pure and FS-free: the caller walks `wiki/`, reads each `.md`, and
//! passes [`WikiFile`]s in. We extract nodes, resolve `[[wikilinks]]`
//! into deduped undirected edges, weight them by relevance, detect
//! communities, and return a fully populated [`WikiGraph`].

use std::collections::{HashMap, HashSet};

use crate::community::{CommNode, detect_communities};
use crate::model::{GraphEdge, GraphNode, WikiGraph};
use crate::parse::{WikiFile, extract_kind, extract_title, extract_wikilinks, resolve_target};
use crate::relevance::{build_retrieval_graph, calculate_relevance};

/// Page kinds excluded from the graph — query/research artifacts are
/// intermediate, not knowledge structure (matches TS `HIDDEN_TYPES`).
const HIDDEN_KINDS: &[&str] = &["query"];

/// Build the knowledge graph from a flat list of wiki markdown files.
pub fn build_wiki_graph(files: &[WikiFile]) -> WikiGraph {
    // Parse every file into preliminary node data.
    struct Raw {
        id: String,
        label: String,
        kind: String,
        path: String,
        links: Vec<String>,
    }
    let mut raw: Vec<Raw> = Vec::new();
    for f in files {
        let kind = extract_kind(&f.content);
        if HIDDEN_KINDS.contains(&kind.as_str()) {
            continue;
        }
        raw.push(Raw {
            id: f.id(),
            label: extract_title(&f.content, &f.name),
            kind,
            path: f.path.clone(),
            links: extract_wikilinks(&f.content),
        });
    }

    if raw.is_empty() {
        return WikiGraph::default();
    }

    let id_set: HashSet<String> = raw.iter().map(|r| r.id.clone()).collect();
    let id_vec: Vec<String> = id_set.iter().cloned().collect();

    // Resolve links → directed edges + degree counts.
    let mut link_counts: HashMap<String, u32> = raw.iter().map(|r| (r.id.clone(), 0u32)).collect();
    let mut directed: Vec<(String, String)> = Vec::new();
    for r in &raw {
        for link in &r.links {
            let Some(target) = resolve_target(link, &id_vec) else {
                continue;
            };
            if target == r.id || !id_set.contains(&target) {
                continue;
            }
            directed.push((r.id.clone(), target.clone()));
            *link_counts.get_mut(&r.id).unwrap() += 1;
            *link_counts.get_mut(&target).unwrap() += 1;
        }
    }

    // Dedup into undirected edges.
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut undirected: Vec<(String, String)> = Vec::new();
    for (s, t) in directed {
        let key = (s.clone(), t.clone());
        let rev = (t.clone(), s.clone());
        if !seen.contains(&key) && !seen.contains(&rev) {
            seen.insert(key);
            undirected.push((s, t));
        }
    }

    // Weight edges by relevance via the retrieval graph.
    let retrieval = build_retrieval_graph(files);
    let edges: Vec<GraphEdge> = undirected
        .into_iter()
        .map(|(s, t)| {
            let weight = match (retrieval.nodes.get(&s), retrieval.nodes.get(&t)) {
                (Some(a), Some(b)) => calculate_relevance(a, b, &retrieval),
                _ => 1.0,
            };
            GraphEdge::wikilink(s, t, weight)
        })
        .collect();

    // Community detection over the weighted graph.
    let comm_nodes: Vec<CommNode<'_>> = raw
        .iter()
        .map(|r| CommNode {
            id: &r.id,
            label: &r.label,
            link_count: *link_counts.get(&r.id).unwrap_or(&0),
        })
        .collect();
    let (assignments, communities) = detect_communities(&comm_nodes, &edges);

    let nodes: Vec<GraphNode> = raw
        .into_iter()
        .map(|r| GraphNode {
            link_count: *link_counts.get(&r.id).unwrap_or(&0),
            community: *assignments.get(&r.id).unwrap_or(&0),
            id: r.id,
            label: r.label,
            kind: r.kind,
            path: r.path,
        })
        .collect();

    WikiGraph {
        nodes,
        edges,
        communities,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, kind: &str, body: &str) -> WikiFile {
        WikiFile {
            name: name.to_string(),
            path: format!("/wiki/{name}"),
            content: format!("---\ntype: {kind}\n---\n{body}"),
        }
    }

    #[test]
    fn builds_nodes_edges_degrees() {
        let files = vec![
            file("a.md", "concept", "I link [[b]] and [[c]]."),
            file("b.md", "entity", "I link [[c]]."),
            file("c.md", "entity", "I link back to [[a]]."),
        ];
        let g = build_wiki_graph(&files);
        assert_eq!(g.nodes.len(), 3);
        // a->b, a->c, b->c, c->a  ... a<->c dedups to one undirected edge.
        assert_eq!(g.edges.len(), 3);
        let a = g.nodes.iter().find(|n| n.id == "a").unwrap();
        // a: out 2 (b,c) + in 1 (c) = 3
        assert_eq!(a.link_count, 3);
    }

    #[test]
    fn query_pages_are_hidden() {
        let files = vec![
            file("a.md", "concept", "[[research-result]]"),
            file("research-result.md", "query", "noise"),
        ];
        let g = build_wiki_graph(&files);
        assert_eq!(g.nodes.len(), 1);
        assert!(g.edges.is_empty());
    }

    #[test]
    fn empty_input_empty_graph() {
        assert_eq!(build_wiki_graph(&[]), WikiGraph::default());
    }
}
