//! Louvain modularity-based community detection over the
//! 4-signal-weighted graph. Plus knowledge-gap detection
//! that depends on clusters (sparse-cluster, bridge-node).
//!
//! ## Algorithm
//!
//! Single-level Louvain (no recursion / no graph
//! aggregation). For the wiki sizes we expect (hundreds of
//! pages), one level is plenty and avoids the
//! coarsen-and-recurse complexity. If a wiki ever needs
//! multi-level, the impl extends cleanly — the inner loop
//! already operates on a generic edge-weight matrix.
//!
//! 1. Each node starts in its own community.
//! 2. Repeat: for each node, evaluate the modularity
//!    delta of moving it into each neighbor's community;
//!    pick the best move. Stop when no node moves.
//! 3. Output: community id per node + cohesion score per
//!    community = sum of internal edge weights / (sum of
//!    internal + half of cross-community edge weights).
//!
//! ## Cohesion
//!
//! Reuses `llm_wiki`'s interpretation: 0..1, where ≥ 0.5 is
//! "well-connected" and < 0.15 surfaces as a sparse
//! cluster gap. Computed as `internal_weight /
//! total_weight` per community (with the cross-community
//! half-weight convention, so isolated nodes get
//! cohesion = 0).

use std::collections::HashMap;
use std::collections::HashSet;

use wiki_proto::graph::{Cluster, GapKind, KnowledgeGap};

use crate::parse::Page;
use crate::scoring::Indices;

/// Run Louvain on the 4-signal weighted graph. Returns one
/// `Cluster` per discovered community (singleton
/// communities are kept — they show up as orphan-like
/// clusters with cohesion 0).
pub(crate) fn louvain_clusters(
    pages: &[Page],
    idx: &Indices,
    edge_weights: &HashMap<(usize, usize), f32>,
) -> Vec<Cluster> {
    let n = pages.len();
    if n == 0 {
        return Vec::new();
    }
    // Adjacency: undirected weights per (i, j) with i < j.
    // Total edge weight m used by the modularity formula.
    let m: f32 = edge_weights.values().sum();
    if m <= 0.0 {
        return pages
            .iter()
            .enumerate()
            .map(|(i, p)| Cluster {
                id: format!("c{i}"),
                name: p.title.clone(),
                members: vec![p.rel_path.clone()],
                cohesion: 0.0,
            })
            .collect();
    }

    // Precompute per-node degree (sum of incident weights).
    let mut node_strength = vec![0.0_f32; n];
    for (&(i, j), &w) in edge_weights {
        node_strength[i] += w;
        node_strength[j] += w;
    }

    let mut comm: Vec<usize> = (0..n).collect();

    // Modularity gain from moving node `i` from its current
    // community into `target_comm`. Standard Newman formula.
    // We score *relative* gain, so the absolute Q is
    // irrelevant — only the deltas matter for greedy moves.
    //
    // ΔQ ≈ (k_i_in / m) - (Σ_tot * k_i) / (2 m²)
    //
    // where k_i_in = 2 × sum of weights from node i to nodes
    // already in target_comm, Σ_tot = sum of strengths of
    // nodes in target_comm.

    let two_m = 2.0 * m;
    let mut changed = true;
    let max_passes = 30;
    let mut pass = 0;
    while changed && pass < max_passes {
        changed = false;
        pass += 1;
        for i in 0..n {
            let best_move = pick_best_move(
                i,
                &comm,
                &idx.neighbors[i],
                edge_weights,
                &node_strength,
                two_m,
            );
            if let Some(new_c) = best_move {
                if new_c != comm[i] {
                    comm[i] = new_c;
                    changed = true;
                }
            }
        }
    }

    // Normalize community ids to compact range + label by
    // most-linked page in each community.
    let mut renumber: HashMap<usize, usize> = HashMap::new();
    let mut compact = Vec::with_capacity(n);
    for &c in &comm {
        let next = renumber.len();
        let id = *renumber.entry(c).or_insert(next);
        compact.push(id);
    }

    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, &c) in compact.iter().enumerate() {
        groups.entry(c).or_default().push(i);
    }

    let mut out = Vec::with_capacity(groups.len());
    for (cid, members) in groups {
        let cohesion = cohesion_for(&members, edge_weights);
        // Name = label of the highest-degree member.
        let label = members
            .iter()
            .max_by_key(|&&i| idx.neighbors[i].len())
            .map(|&i| pages[i].title.clone())
            .unwrap_or_default();
        out.push(Cluster {
            id: format!("c{cid}"),
            name: label,
            members: members.iter().map(|&i| pages[i].rel_path.clone()).collect(),
            cohesion,
        });
    }
    out.sort_by(|a, b| b.members.len().cmp(&a.members.len()));
    out
}

fn pick_best_move(
    i: usize,
    comm: &[usize],
    neighbors: &HashSet<usize>,
    edge_weights: &HashMap<(usize, usize), f32>,
    node_strength: &[f32],
    two_m: f32,
) -> Option<usize> {
    let current = comm[i];
    // Build candidate set: i's current community + each
    // neighbor's community.
    let mut candidates: HashSet<usize> = HashSet::new();
    candidates.insert(current);
    for &n in neighbors {
        candidates.insert(comm[n]);
    }
    let mut best = current;
    let mut best_gain = 0.0_f32;
    for &cand in &candidates {
        if cand == current {
            continue;
        }
        let mut k_i_in = 0.0_f32;
        let mut sigma_tot = 0.0_f32;
        for (j, &c) in comm.iter().enumerate() {
            if c != cand {
                continue;
            }
            sigma_tot += node_strength[j];
            if j == i {
                continue;
            }
            let key = if i < j { (i, j) } else { (j, i) };
            if let Some(&w) = edge_weights.get(&key) {
                k_i_in += w;
            }
        }
        let k_i = node_strength[i];
        let gain = (k_i_in / two_m) - (sigma_tot * k_i) / (two_m * two_m);
        if gain > best_gain {
            best_gain = gain;
            best = cand;
        }
    }
    if best == current { None } else { Some(best) }
}

fn cohesion_for(members: &[usize], edge_weights: &HashMap<(usize, usize), f32>) -> f32 {
    if members.len() < 2 {
        return 0.0;
    }
    let set: HashSet<usize> = members.iter().copied().collect();
    let mut internal = 0.0_f32;
    let mut external = 0.0_f32;
    for (&(i, j), &w) in edge_weights {
        let i_in = set.contains(&i);
        let j_in = set.contains(&j);
        if i_in && j_in {
            internal += w;
        } else if i_in || j_in {
            external += w;
        }
    }
    let total = internal + 0.5 * external;
    if total <= 0.0 {
        0.0
    } else {
        (internal / total).clamp(0.0, 1.0)
    }
}

/// Surface cluster-shaped gaps: sparse-cluster (cohesion <
/// 0.15) + bridge-node (highest-betweenness node whose
/// removal would split a cluster). Bridge detection here
/// is approximate — we flag the single highest-degree node
/// per cluster whose neighbors span ≥ 2 other clusters.
pub(crate) fn cluster_gaps(
    clusters: &[Cluster],
    pages: &[Page],
    idx: &Indices,
) -> Vec<KnowledgeGap> {
    let mut out = Vec::new();

    // Sparse clusters.
    for c in clusters {
        if c.members.len() >= 3 && c.cohesion < 0.15 {
            out.push(KnowledgeGap {
                id: format!("sparse-{}", c.id),
                kind: GapKind::SparseCluster,
                subjects: c.members.clone(),
                explanation: format!(
                    "Cluster \"{}\" has {} pages but cohesion {:.2} — under the 0.15 sparse threshold. \
                    Members may need more cross-links or a unifying source.",
                    c.name,
                    c.members.len(),
                    c.cohesion
                ),
            });
        }
    }

    // Bridge candidates.
    let community_of: HashMap<String, &str> = clusters
        .iter()
        .flat_map(|c| c.members.iter().map(move |m| (m.clone(), c.id.as_str())))
        .collect();
    for (i, p) in pages.iter().enumerate() {
        let me = match community_of.get(&p.rel_path) {
            Some(c) => *c,
            None => continue,
        };
        let mut other_comms: HashSet<&str> = HashSet::new();
        for &n in &idx.neighbors[i] {
            if let Some(nc) = community_of.get(&pages[n].rel_path) {
                if *nc != me {
                    other_comms.insert(*nc);
                }
            }
        }
        if other_comms.len() >= 2 && idx.neighbors[i].len() >= 4 {
            out.push(KnowledgeGap {
                id: format!("bridge-{i}"),
                kind: GapKind::Bridge,
                subjects: vec![p.rel_path.clone()],
                explanation: format!(
                    "Page \"{}\" bridges {} communities and has degree {} — losing it would fragment them.",
                    p.title,
                    other_comms.len(),
                    idx.neighbors[i].len()
                ),
            });
        }
    }
    out
}
