//! Edge relevance scoring — port of `graph-relevance.ts`.
//!
//! Builds a [`RetrievalGraph`] (nodes + in/out link sets + sources) and
//! scores any node pair with four signals: direct links, shared
//! sources, Adamic–Adar common neighbors, and type affinity. The wiki
//! builder uses these scores as edge weights so stronger relationships
//! render thicker/darker.

use std::collections::{HashMap, HashSet};

use crate::parse::{
    WikiFile, extract_kind, extract_sources, extract_title, extract_wikilinks, resolve_target,
};

#[derive(Debug, Clone)]
pub struct RetrievalNode {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub path: String,
    pub sources: Vec<String>,
    pub out_links: HashSet<String>,
    pub in_links: HashSet<String>,
}

impl RetrievalNode {
    fn neighbors(&self) -> HashSet<String> {
        self.out_links.union(&self.in_links).cloned().collect()
    }
    fn degree(&self) -> usize {
        self.out_links.len() + self.in_links.len()
    }
}

#[derive(Debug, Clone, Default)]
pub struct RetrievalGraph {
    pub nodes: HashMap<String, RetrievalNode>,
}

// Signal weights — identical to the TS `WEIGHTS` constant.
const W_DIRECT_LINK: f32 = 3.0;
const W_SOURCE_OVERLAP: f32 = 4.0;
const W_COMMON_NEIGHBOR: f32 = 1.5;
const W_TYPE_AFFINITY: f32 = 1.0;

/// Type-affinity lookup, mirroring `TYPE_AFFINITY`. Missing pairs score
/// `0.5`.
fn type_affinity(a: &str, b: &str) -> f32 {
    let row: &[(&str, f32)] = match a {
        "entity" => &[
            ("concept", 1.2),
            ("entity", 0.8),
            ("source", 1.0),
            ("synthesis", 1.0),
            ("query", 0.8),
        ],
        "concept" => &[
            ("entity", 1.2),
            ("concept", 0.8),
            ("source", 1.0),
            ("synthesis", 1.2),
            ("query", 1.0),
        ],
        "source" => &[
            ("entity", 1.0),
            ("concept", 1.0),
            ("source", 0.5),
            ("query", 0.8),
            ("synthesis", 1.0),
        ],
        "query" => &[
            ("concept", 1.0),
            ("entity", 0.8),
            ("synthesis", 1.0),
            ("source", 0.8),
            ("query", 0.5),
        ],
        "synthesis" => &[
            ("concept", 1.2),
            ("entity", 1.0),
            ("source", 1.0),
            ("query", 1.0),
            ("synthesis", 0.8),
        ],
        _ => return 0.5,
    };
    row.iter().find(|(k, _)| *k == b).map_or(0.5, |(_, v)| *v)
}

/// One parsed wiki file before link resolution:
/// `(id, title, kind, path, sources, wikilinks)`.
type RawNode = (String, String, String, String, Vec<String>, Vec<String>);

/// Build the retrieval graph from wiki files.
pub fn build_retrieval_graph(files: &[WikiFile]) -> RetrievalGraph {
    let mut raw: Vec<RawNode> = Vec::new();
    for f in files {
        let id = f.id();
        raw.push((
            id,
            {
                let t = extract_title(&f.content, &f.name);
                if t.is_empty() {
                    f.name
                        .strip_suffix(".md")
                        .unwrap_or(&f.name)
                        .replace('-', " ")
                } else {
                    t
                }
            },
            extract_kind(&f.content),
            f.path.clone(),
            extract_sources(&f.content),
            extract_wikilinks(&f.content),
        ));
    }

    let ids: HashSet<String> = raw.iter().map(|r| r.0.clone()).collect();
    let id_vec: Vec<String> = ids.iter().cloned().collect();

    let mut out_links: HashMap<String, HashSet<String>> = HashMap::new();
    let mut in_links: HashMap<String, HashSet<String>> = HashMap::new();
    for id in &ids {
        out_links.insert(id.clone(), HashSet::new());
        in_links.insert(id.clone(), HashSet::new());
    }

    for (id, _, _, _, _, links) in &raw {
        for link in links {
            if let Some(target) = resolve_target(link, &id_vec) {
                if &target == id {
                    continue;
                }
                out_links.get_mut(id).unwrap().insert(target.clone());
                in_links.get_mut(&target).unwrap().insert(id.clone());
            }
        }
    }

    let mut nodes = HashMap::new();
    for (id, title, kind, path, sources, _) in raw {
        let out = out_links.remove(&id).unwrap_or_default();
        let inn = in_links.remove(&id).unwrap_or_default();
        nodes.insert(
            id.clone(),
            RetrievalNode {
                id,
                title,
                kind,
                path,
                sources,
                out_links: out,
                in_links: inn,
            },
        );
    }

    RetrievalGraph { nodes }
}

/// Score the relevance between two retrieval nodes. Symmetric, `>= 0`.
pub fn calculate_relevance(a: &RetrievalNode, b: &RetrievalNode, graph: &RetrievalGraph) -> f32 {
    if a.id == b.id {
        return 0.0;
    }

    // Signal 1: direct links (either direction).
    let forward = if a.out_links.contains(&b.id) {
        1.0
    } else {
        0.0
    };
    let backward = if b.out_links.contains(&a.id) {
        1.0
    } else {
        0.0
    };
    let direct = (forward + backward) * W_DIRECT_LINK;

    // Signal 2: source overlap.
    let sources_a: HashSet<&String> = a.sources.iter().collect();
    let shared = b.sources.iter().filter(|s| sources_a.contains(s)).count();
    let source_overlap = shared as f32 * W_SOURCE_OVERLAP;

    // Signal 3: Adamic–Adar over common neighbors.
    let neighbors_a = a.neighbors();
    let neighbors_b = b.neighbors();
    let mut adamic_adar = 0.0_f32;
    for n in &neighbors_a {
        if neighbors_b.contains(n) {
            if let Some(node) = graph.nodes.get(n) {
                let degree = node.degree().max(2) as f32;
                adamic_adar += 1.0 / degree.ln();
            }
        }
    }
    let common_neighbor = adamic_adar * W_COMMON_NEIGHBOR;

    // Signal 4: type affinity.
    let affinity = type_affinity(&a.kind, &b.kind) * W_TYPE_AFFINITY;

    direct + source_overlap + common_neighbor + affinity
}

/// Top-`limit` nodes most relevant to `node_id`, highest score first.
pub fn related_nodes<'a>(
    node_id: &str,
    graph: &'a RetrievalGraph,
    limit: usize,
) -> Vec<(&'a RetrievalNode, f32)> {
    let source = match graph.nodes.get(node_id) {
        Some(n) => n,
        None => return Vec::new(),
    };
    let mut scored: Vec<(&RetrievalNode, f32)> = graph
        .nodes
        .values()
        .filter(|n| n.id != node_id)
        .map(|n| (n, calculate_relevance(source, n, graph)))
        .filter(|(_, r)| *r > 0.0)
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(limit);
    scored
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
    fn direct_links_boost_relevance() {
        let files = vec![
            file("a.md", "concept", "links [[b]]"),
            file("b.md", "entity", "no links"),
            file("c.md", "source", "unrelated"),
        ];
        let g = build_retrieval_graph(&files);
        let a = &g.nodes["a"];
        let b = &g.nodes["b"];
        let c = &g.nodes["c"];
        assert!(calculate_relevance(a, b, &g) > calculate_relevance(a, c, &g));
    }
}
