//! Knowledge-gap detection over the page set.
//!
//! Today: **orphan** (degree ≤ 1) and **missing page**
//! (wikilink target that doesn't resolve to any page on
//! disk). `SparseCluster` + `Bridge` follow once Louvain
//! lands.

use std::collections::HashMap;
use std::path::Path;

use wiki_proto::graph::{GapKind, KnowledgeGap};

use wiki_proto::graph::RelevanceWeights;

use crate::louvain::{cluster_gaps, louvain_clusters};
use crate::scan::{ScanError, scan_wiki};
use crate::scoring::{Indices, edge_weight_matrix};

/// Compute knowledge gaps for a vault. Returns an empty
/// list when the wiki is empty or perfectly connected.
pub fn find_gaps(vault_root: &Path) -> Result<Vec<KnowledgeGap>, ScanError> {
    let pages = scan_wiki(vault_root)?;
    let idx = Indices::build(&pages);
    let weights = RelevanceWeights::default();
    let matrix = edge_weight_matrix(&pages, &idx, &weights);
    let clusters = louvain_clusters(&pages, &idx, &matrix);
    let mut out = cluster_gaps(&clusters, &pages, &idx);

    // ── Orphans ───────────────────────────────────────
    for (i, p) in pages.iter().enumerate() {
        if idx.neighbors[i].len() <= 1 {
            out.push(KnowledgeGap {
                id: format!("orphan-{i}"),
                kind: GapKind::Orphan,
                subjects: vec![p.rel_path.clone()],
                explanation: format!(
                    "Page \"{}\" has degree {} — nothing links to it (or only a single tangential link).",
                    p.title,
                    idx.neighbors[i].len()
                ),
            });
        }
    }

    // ── Missing pages ─────────────────────────────────
    // Tally every wikilink target the body references;
    // any target unresolved by `Indices::build` is a
    // missing-page candidate. Score by mention count so
    // heavily-referenced gaps surface first.
    let mut mention_count: HashMap<String, u32> = HashMap::new();
    let mut mention_sources: HashMap<String, Vec<String>> = HashMap::new();
    for p in &pages {
        for target in &p.outlinks {
            let key = target.to_lowercase();
            if idx.by_title.contains_key(&key) || idx.by_stem.contains_key(&key) {
                continue;
            }
            *mention_count.entry(target.clone()).or_default() += 1;
            mention_sources
                .entry(target.clone())
                .or_default()
                .push(p.rel_path.clone());
        }
    }
    let mut missing: Vec<(String, u32)> = mention_count.into_iter().collect();
    missing.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    for (target, count) in missing {
        let mentions = mention_sources.remove(&target).unwrap_or_default();
        out.push(KnowledgeGap {
            id: format!("missing-{}", slug(&target)),
            kind: GapKind::MissingPage,
            subjects: mentions.clone(),
            explanation: format!(
                "[[{target}]] is referenced {count} time(s) but has no dedicated page."
            ),
        });
    }

    Ok(out)
}

fn slug(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if ch.is_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}
