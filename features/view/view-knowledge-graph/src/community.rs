//! Community detection + cohesion.
//!
//! `wiki-graph.ts` runs graphology's Louvain. We don't pull a graph
//! library into wasm, so this is a deterministic weighted
//! label-propagation pass instead — same *shape* of output
//! ([`CommunityInfo`] with cohesion + top nodes, renumbered by size),
//! good clustering for wiki-scale graphs, no external dep. Determinism
//! comes from sorted iteration (no RNG), so renders are stable.

use std::collections::{HashMap, HashSet};

use crate::model::{CommunityInfo, GraphEdge};

/// Minimal node view needed for detection.
pub struct CommNode<'a> {
    pub id: &'a str,
    pub label: &'a str,
    pub link_count: u32,
}

/// Run detection. Returns `(assignments, communities)` where
/// `assignments[node_id] = community_id` and communities are sorted by
/// descending node count and renumbered `0..n`.
pub fn detect_communities(
    nodes: &[CommNode<'_>],
    edges: &[GraphEdge],
) -> (HashMap<String, u32>, Vec<CommunityInfo>) {
    if nodes.is_empty() {
        return (HashMap::new(), Vec::new());
    }

    // Index nodes by id (sorted for deterministic iteration order).
    let mut ids: Vec<&str> = nodes.iter().map(|n| n.id).collect();
    ids.sort_unstable();
    let index: HashMap<&str, usize> = ids.iter().enumerate().map(|(i, id)| (*id, i)).collect();

    // Weighted adjacency (undirected, deduped).
    let mut adj: Vec<Vec<(usize, f32)>> = vec![Vec::new(); ids.len()];
    for e in edges {
        let (Some(&s), Some(&t)) = (index.get(e.source.as_str()), index.get(e.target.as_str()))
        else {
            continue;
        };
        if s == t {
            continue;
        }
        let w = e.weight.max(0.01);
        adj[s].push((t, w));
        adj[t].push((s, w));
    }

    // Label propagation: start with each node in its own community.
    let mut labels: Vec<usize> = (0..ids.len()).collect();
    for _ in 0..20 {
        let mut changed = false;
        // Deterministic node order (sorted ids → index order).
        for v in 0..ids.len() {
            if adj[v].is_empty() {
                continue;
            }
            // Sum neighbor weights per candidate label.
            let mut tally: HashMap<usize, f32> = HashMap::new();
            for &(u, w) in &adj[v] {
                *tally.entry(labels[u]).or_insert(0.0) += w;
            }
            // Pick the heaviest label; tie-break on smallest label id
            // for determinism.
            let best = tally
                .into_iter()
                .max_by(|a, b| {
                    a.1.partial_cmp(&b.1)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then(b.0.cmp(&a.0))
                })
                .map(|(label, _)| label);
            if let Some(best) = best {
                if best != labels[v] {
                    labels[v] = best;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }

    // Group members by raw label.
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, &label) in labels.iter().enumerate() {
        groups.entry(label).or_default().push(i);
    }

    // Edge set for cohesion lookups (undirected, by index pair).
    let mut edge_set: HashSet<(usize, usize)> = HashSet::new();
    for e in edges {
        if let (Some(&s), Some(&t)) = (index.get(e.source.as_str()), index.get(e.target.as_str())) {
            if s != t {
                edge_set.insert((s.min(t), s.max(t)));
            }
        }
    }

    let link_count: HashMap<&str, u32> = nodes.iter().map(|n| (n.id, n.link_count)).collect();
    let label_of: HashMap<usize, &str> = ids.iter().enumerate().map(|(i, id)| (i, *id)).collect();

    // Build community summaries.
    let mut communities: Vec<(Vec<usize>, f32, Vec<String>)> = Vec::new();
    for members in groups.values() {
        let n = members.len();
        let mut intra = 0usize;
        for i in 0..members.len() {
            for j in (i + 1)..members.len() {
                let a = members[i].min(members[j]);
                let b = members[i].max(members[j]);
                if edge_set.contains(&(a, b)) {
                    intra += 1;
                }
            }
        }
        let possible = if n > 1 { n * (n - 1) / 2 } else { 1 };
        let cohesion = intra as f32 / possible as f32;

        // Top 5 member labels by link_count.
        let mut sorted = members.clone();
        sorted.sort_by(|&a, &b| {
            let la = *link_count.get(label_of[&a]).unwrap_or(&0);
            let lb = *link_count.get(label_of[&b]).unwrap_or(&0);
            lb.cmp(&la)
        });
        let top_nodes: Vec<String> = sorted
            .iter()
            .take(5)
            .map(|&idx| {
                let id = label_of[&idx];
                nodes
                    .iter()
                    .find(|nd| nd.id == id)
                    .map_or_else(|| id.to_string(), |nd| nd.label.to_string())
            })
            .collect();

        communities.push((members.clone(), cohesion, top_nodes));
    }

    // Sort by descending node count, renumber 0..n.
    communities.sort_by(|a, b| b.0.len().cmp(&a.0.len()));

    let mut assignments: HashMap<String, u32> = HashMap::new();
    let mut infos: Vec<CommunityInfo> = Vec::new();
    for (new_id, (members, cohesion, top_nodes)) in communities.into_iter().enumerate() {
        for &idx in &members {
            assignments.insert(ids[idx].to_string(), new_id as u32);
        }
        infos.push(CommunityInfo {
            id: new_id as u32,
            node_count: members.len(),
            cohesion,
            top_nodes,
        });
    }

    (assignments, infos)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge(s: &str, t: &str) -> GraphEdge {
        GraphEdge::wikilink(s.to_string(), t.to_string(), 1.0)
    }

    #[test]
    fn two_cliques_become_two_communities() {
        let nodes = vec![
            CommNode {
                id: "a",
                label: "A",
                link_count: 2,
            },
            CommNode {
                id: "b",
                label: "B",
                link_count: 2,
            },
            CommNode {
                id: "c",
                label: "C",
                link_count: 2,
            },
            CommNode {
                id: "x",
                label: "X",
                link_count: 2,
            },
            CommNode {
                id: "y",
                label: "Y",
                link_count: 2,
            },
            CommNode {
                id: "z",
                label: "Z",
                link_count: 2,
            },
        ];
        let edges = vec![
            edge("a", "b"),
            edge("b", "c"),
            edge("a", "c"),
            edge("x", "y"),
            edge("y", "z"),
            edge("x", "z"),
        ];
        let (assign, comms) = detect_communities(&nodes, &edges);
        assert_eq!(comms.len(), 2);
        // a,b,c share one community; x,y,z another.
        assert_eq!(assign["a"], assign["b"]);
        assert_eq!(assign["a"], assign["c"]);
        assert_ne!(assign["a"], assign["x"]);
        assert!(comms[0].cohesion > 0.9);
    }

    #[test]
    fn empty_is_empty() {
        let (a, c) = detect_communities(&[], &[]);
        assert!(a.is_empty() && c.is_empty());
    }
}
