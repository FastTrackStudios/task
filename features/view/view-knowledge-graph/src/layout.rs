//! Force-directed layout — replaces llm_wiki's forceAtlas2 + web worker.
//!
//! A Fruchterman–Reingold spring model with center gravity (the TS code
//! runs forceAtlas2 with `strongGravityMode`). It is deterministic:
//! initial positions come from a golden-angle spiral seeded by node
//! index plus an id-hash jitter — no RNG — so the same graph always
//! lays out identically and SVG re-renders don't jump.
//!
//! For wiki-scale graphs (hundreds of nodes) this runs in a few ms and
//! can execute synchronously inside the component; no worker needed.

use std::collections::HashMap;

use crate::model::{GraphEdge, GraphNode};

/// A computed node position in layout space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Position {
    pub x: f32,
    pub y: f32,
}

/// Tunables for the layout.
#[derive(Debug, Clone, Copy)]
pub struct LayoutConfig {
    /// User spacing multiplier (llm_wiki "graph spacing" slider). `1.0`
    /// is the neutral default; higher spreads nodes apart.
    pub spacing: f32,
    /// Iteration override. `None` scales with node count.
    pub iterations: Option<usize>,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            spacing: 1.0,
            iterations: None,
        }
    }
}

/// Iteration count by node count (port of `layoutIterations`).
fn default_iterations(n: usize) -> usize {
    match n {
        _ if n > 2500 => 28,
        _ if n > 1200 => 40,
        _ if n > 600 => 65,
        _ if n > 250 => 90,
        _ => 140,
    }
}

fn id_hash(id: &str) -> u32 {
    // FNV-1a 32-bit offset basis + prime.
    let mut h: u32 = 2_166_136_261;
    for b in id.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(16_777_619);
    }
    h
}

/// Compute positions for every node. Returns a map keyed by node id.
/// Edge `weight` strengthens attraction so related pages cluster.
// Short math names (`n`, `k`, `d`, `temp`) read truer to the
// force-directed layout literature than verbose ones would.
#[allow(clippy::many_single_char_names)]
pub fn layout(
    nodes: &[GraphNode],
    edges: &[GraphEdge],
    cfg: LayoutConfig,
) -> HashMap<String, Position> {
    let n = nodes.len();
    let mut pos: HashMap<String, Position> = HashMap::with_capacity(n);
    if n == 0 {
        return pos;
    }
    if n == 1 {
        pos.insert(nodes[0].id.clone(), Position { x: 0.0, y: 0.0 });
        return pos;
    }

    // Layout area scales with node count; `k` is the ideal edge length.
    let area = 1000.0_f32 * 1000.0 * cfg.spacing.max(0.2);
    let k = (area / n as f32).sqrt();

    // Index nodes; deterministic golden-angle spiral seed + id jitter.
    let mut p: Vec<Position> = Vec::with_capacity(n);
    let golden = std::f32::consts::PI * (3.0 - 5.0_f32.sqrt()); // ~2.399963
    for (i, node) in nodes.iter().enumerate() {
        let radius = k * (i as f32 + 1.0).sqrt();
        let angle = i as f32 * golden;
        let jitter = (id_hash(&node.id) % 1000) as f32 / 1000.0 - 0.5;
        p.push(Position {
            x: radius * angle.cos() + jitter * k * 0.1,
            y: radius * angle.sin() + jitter * k * 0.1,
        });
    }

    let index: HashMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, nd)| (nd.id.as_str(), i))
        .collect();
    let max_weight = edges.iter().map(|e| e.weight).fold(1.0_f32, f32::max);

    // Resolve edges to index pairs with normalized weight once.
    let edge_pairs: Vec<(usize, usize, f32)> = edges
        .iter()
        .filter_map(|e| {
            let s = *index.get(e.source.as_str())?;
            let t = *index.get(e.target.as_str())?;
            if s == t {
                return None;
            }
            Some((s, t, (e.weight / max_weight).clamp(0.1, 1.0)))
        })
        .collect();

    let iterations = cfg.iterations.unwrap_or_else(|| default_iterations(n));
    let mut temp = k * 2.0; // initial max displacement per step
    let cooling = temp / (iterations as f32 + 1.0);
    let gravity = 0.02; // pull toward origin (strongGravityMode analogue)

    let mut disp = vec![Position { x: 0.0, y: 0.0 }; n];

    for _ in 0..iterations {
        for d in &mut disp {
            d.x = 0.0;
            d.y = 0.0;
        }

        // Repulsive forces (all pairs). O(n^2) — fine at wiki scale.
        for i in 0..n {
            for j in (i + 1)..n {
                let dx = p[i].x - p[j].x;
                let dy = p[i].y - p[j].y;
                let dist = (dx * dx + dy * dy).sqrt().max(0.01);
                let force = k * k / dist;
                let fx = dx / dist * force;
                let fy = dy / dist * force;
                disp[i].x += fx;
                disp[i].y += fy;
                disp[j].x -= fx;
                disp[j].y -= fy;
            }
        }

        // Attractive forces along edges (scaled by weight).
        for &(s, t, w) in &edge_pairs {
            let dx = p[s].x - p[t].x;
            let dy = p[s].y - p[t].y;
            let dist = (dx * dx + dy * dy).sqrt().max(0.01);
            let force = dist * dist / k * w;
            let fx = dx / dist * force;
            let fy = dy / dist * force;
            disp[s].x -= fx;
            disp[s].y -= fy;
            disp[t].x += fx;
            disp[t].y += fy;
        }

        // Apply displacement (capped by temperature) + gravity.
        for i in 0..n {
            disp[i].x -= p[i].x * gravity * k;
            disp[i].y -= p[i].y * gravity * k;
            let len = (disp[i].x * disp[i].x + disp[i].y * disp[i].y)
                .sqrt()
                .max(0.01);
            let capped = len.min(temp);
            p[i].x += disp[i].x / len * capped;
            p[i].y += disp[i].y / len * capped;
        }

        temp = (temp - cooling).max(k * 0.05);
    }

    for (i, node) in nodes.iter().enumerate() {
        pos.insert(node.id.clone(), p[i]);
    }
    pos
}

/// Bounding box of a position set: `(min_x, min_y, max_x, max_y)`.
/// Returns a unit box when empty.
pub fn bounds<S: std::hash::BuildHasher>(
    positions: &HashMap<String, Position, S>,
) -> (f32, f32, f32, f32) {
    let mut min_x = f32::MAX;
    let mut min_y = f32::MAX;
    let mut max_x = f32::MIN;
    let mut max_y = f32::MIN;
    for pos in positions.values() {
        min_x = min_x.min(pos.x);
        min_y = min_y.min(pos.y);
        max_x = max_x.max(pos.x);
        max_y = max_y.max(pos.y);
    }
    if positions.is_empty() {
        return (0.0, 0.0, 1.0, 1.0);
    }
    (min_x, min_y, max_x, max_y)
}

// --- Node sizing (port of `nodeSize` / `graphDensityScale`) ---
//
// Deliberately smaller than the llm_wiki original (3..11): compact
// nodes keep labels readable at fit-zoom; the edges carry the extra
// visual weight instead (see the stroke widths in graph_view).

const BASE_NODE_SIZE: f32 = 2.0;
const MAX_NODE_SIZE: f32 = 7.0;

fn density_scale(node_count: usize) -> f32 {
    if node_count <= 150 {
        1.0
    } else {
        (150.0 / node_count as f32).sqrt().max(0.35)
    }
}

/// Radius for a node given its degree, the max degree, total count, and
/// a user scale (matches `nodeSize` in graph-view.tsx).
pub fn node_radius(link_count: u32, max_links: u32, node_count: usize, user_scale: f32) -> f32 {
    if max_links == 0 {
        return BASE_NODE_SIZE;
    }
    let ratio = link_count as f32 / max_links as f32;
    let size = BASE_NODE_SIZE + ratio.sqrt() * (MAX_NODE_SIZE - BASE_NODE_SIZE);
    size * density_scale(node_count) * user_scale
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str) -> GraphNode {
        GraphNode {
            id: id.to_string(),
            label: id.to_string(),
            kind: "entity".to_string(),
            path: String::new(),
            link_count: 1,
            community: 0,
        }
    }

    #[test]
    fn deterministic_layout() {
        let nodes = vec![node("a"), node("b"), node("c")];
        let edges = vec![GraphEdge::wikilink("a".into(), "b".into(), 1.0)];
        let a = layout(&nodes, &edges, LayoutConfig::default());
        let b = layout(&nodes, &edges, LayoutConfig::default());
        assert_eq!(a["a"], b["a"]);
        assert_eq!(a["c"], b["c"]);
    }

    #[test]
    fn positions_are_finite() {
        let nodes: Vec<GraphNode> = (0..30).map(|i| node(&format!("n{i}"))).collect();
        let edges: Vec<GraphEdge> = (0..29)
            .map(|i| GraphEdge::wikilink(format!("n{i}"), format!("n{}", i + 1), 1.0))
            .collect();
        let pos = layout(&nodes, &edges, LayoutConfig::default());
        for p in pos.values() {
            assert!(p.x.is_finite() && p.y.is_finite());
        }
    }

    #[test]
    fn radius_grows_with_degree() {
        assert!(node_radius(10, 10, 50, 1.0) > node_radius(1, 10, 50, 1.0));
    }
}
