//! Top-level entry points — `build_graph` for the 4-signal
//! snapshot, `build_clusters` for the Louvain partition.

use std::collections::HashMap;
use std::path::Path;

use wiki_proto::graph::{Cluster, GraphOpts, RelevanceWeights, WikiGraph};

use crate::louvain::louvain_clusters;
use crate::scan::{ScanError, scan_wiki};
use crate::scoring::{Indices, edge_weight_matrix, score_graph};

/// Walk `<vault_root>/Wiki/`, parse all pages, and return
/// the typed [`WikiGraph`] under the given options.
#[allow(clippy::needless_pass_by_value)]
pub fn build_graph(vault_root: &Path, opts: GraphOpts) -> Result<WikiGraph, ScanError> {
    let pages = scan_wiki(vault_root)?;
    let idx = Indices::build(&pages);
    let weights = opts.weights.unwrap_or_default();
    let mut graph = score_graph(&pages, &idx, &weights, &opts.query, &opts.node_type);
    // Decorate nodes with cluster ids — cheap, runs the
    // same Louvain pass `build_clusters` does.
    let matrix = edge_weight_matrix(&pages, &idx, &weights);
    let clusters = louvain_clusters(&pages, &idx, &matrix);
    let cluster_of: HashMap<String, String> = clusters
        .iter()
        .flat_map(|c| c.members.iter().map(move |m| (m.clone(), c.id.clone())))
        .collect();
    for n in &mut graph.nodes {
        if let Some(c) = cluster_of.get(&n.id) {
            n.community = c.clone();
        }
    }
    // Honor `limit` last so the cap applies to the
    // already-filtered set.
    if opts.limit > 0 && graph.nodes.len() > opts.limit as usize {
        graph
            .nodes
            .sort_by(|a, b| b.link_count.cmp(&a.link_count).then(a.id.cmp(&b.id)));
        graph.nodes.truncate(opts.limit as usize);
        let kept: std::collections::HashSet<String> =
            graph.nodes.iter().map(|n| n.id.clone()).collect();
        graph
            .edges
            .retain(|e| kept.contains(&e.source) && kept.contains(&e.target));
    }
    Ok(graph)
}

/// Run Louvain over the 4-signal graph and return the
/// communities (ordered by member count, descending).
pub fn build_clusters(vault_root: &Path) -> Result<Vec<Cluster>, ScanError> {
    let pages = scan_wiki(vault_root)?;
    let idx = Indices::build(&pages);
    let weights = RelevanceWeights::default();
    let matrix = edge_weight_matrix(&pages, &idx, &weights);
    Ok(louvain_clusters(&pages, &idx, &matrix))
}
