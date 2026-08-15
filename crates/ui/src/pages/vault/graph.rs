//! Local-graph construction + verse-reference helpers for the vault
//! page's right-sidebar Links tab.
//!
//! Pure functions over already-fetched data — no signals, no RPC — so
//! they are unit-testable and keep `vault/mod.rs` focused on wiring.

use std::collections::{HashMap, HashSet};

use view_knowledge_graph::{GraphEdge, GraphNode, WikiGraph};

use super::basename_of;

/// The focused note + its 1-hop neighbourhood as a [`WikiGraph`], built
/// client-side from what the right panel already fetched: backlink
/// sources point AT `current`, resolved outgoing wikilinks point FROM
/// it. Node ids are vault-relative paths, so a node click maps
/// straight back onto the panel's `on_open` flow; labels come from the
/// folder-index title lookup (basename fallback). Unresolved links are
/// skipped — they have no note to open. Duplicate connections (a page
/// that both links here and is linked from here) collapse to one edge.
pub(super) fn build_local_graph(
    current: &str,
    backlinks: &[String],
    outlinks: &[vault_proto::GraphLink],
    titles: &HashMap<String, (String, String)>,
) -> WikiGraph {
    // Edge list, deduped as unordered pairs (self-links dropped).
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut edges: Vec<(String, String)> = Vec::new();
    let mut push = |source: String, target: String| {
        if source == target {
            return;
        }
        let key = if source < target {
            (source.clone(), target.clone())
        } else {
            (target.clone(), source.clone())
        };
        if seen.insert(key) {
            edges.push((source, target));
        }
    };
    for b in backlinks {
        push(b.clone(), current.to_owned());
    }
    for l in outlinks {
        if let Some(t) = &l.resolved {
            push(current.to_owned(), t.clone());
        }
    }

    // Node set = focal + everything an edge touches; link_count is
    // the in-graph degree (sizes the focal node as the hub).
    let mut degree: HashMap<&str, u32> = HashMap::new();
    for (s, t) in &edges {
        *degree.entry(s.as_str()).or_default() += 1;
        *degree.entry(t.as_str()).or_default() += 1;
    }
    let mut paths: Vec<&str> = std::iter::once(current)
        .chain(edges.iter().flat_map(|(s, t)| [s.as_str(), t.as_str()]))
        .collect();
    paths.sort_unstable();
    paths.dedup();
    let nodes = paths
        .into_iter()
        .map(|p| GraphNode {
            id: p.to_owned(),
            label: titles
                .get(p)
                .map(|(title, _)| title.clone())
                .unwrap_or_else(|| basename_of(p).to_owned()),
            kind: "other".to_owned(),
            path: p.to_owned(),
            link_count: degree.get(p).copied().unwrap_or(0),
            community: 0,
        })
        .collect();
    let edges = edges
        .into_iter()
        .map(|(s, t)| GraphEdge::wikilink(s, t, 1.0))
        .collect();
    WikiGraph {
        nodes,
        edges,
        communities: Vec::new(),
    }
}

/// OSIS verse id → a human reference the scripture service parses
/// (`John.3.16` → `John 3:16`; a range keeps its start). Best-effort.
pub(super) fn osis_to_ref(osis: &str) -> String {
    let first = osis.split('-').next().unwrap_or(osis);
    let mut it = first.rsplitn(3, '.');
    match (it.next(), it.next(), it.next()) {
        (Some(v), Some(c), Some(b)) => format!("{b} {c}:{v}"),
        _ => first.to_string(),
    }
}
