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

    out.extend(one_voice_gaps(&pages));
    Ok(out)
}

/// t[impl wiki.gaps.one-voice]
/// Pages that rest on one voice: every source they cite is by the same
/// author (read from the source pages' `author:`), or they cite exactly
/// one source whose author is unknown. A wiki built from one video, or
/// from several by the same person, looks well sourced and is not — this
/// says so, page by page.
fn one_voice_gaps(pages: &[crate::parse::Page]) -> Vec<KnowledgeGap> {
    // A source's voice by its file stem: `raw/sources/talk-ac25.md` and
    // the summary `Sources/talk-ac25.md` share the stem.
    let stem = |p: &str| {
        Path::new(p)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(p)
            .to_owned()
    };
    let authors: HashMap<String, String> = pages
        .iter()
        .filter(|p| p.page_type == "source" && !p.author.is_empty())
        .map(|p| (stem(&p.rel_path), p.author.clone()))
        .collect();
    let mut out = Vec::new();
    for p in pages {
        if p.page_type == "source" || p.sources.is_empty() {
            continue;
        }
        let voices: std::collections::BTreeSet<String> = p
            .sources
            .iter()
            .map(|s| {
                let st = stem(s);
                authors.get(&st).cloned().unwrap_or(st)
            })
            .collect();
        if voices.len() != 1 {
            continue;
        }
        let voice = voices.into_iter().next().unwrap_or_default();
        let known = authors.values().any(|a| *a == voice);
        let explanation = if known && p.sources.len() > 1 {
            format!(
                "\"{}\" rests on one voice: its {} sources are all by {voice}. An independent source would test it.",
                p.title,
                p.sources.len()
            )
        } else if known {
            format!(
                "\"{}\" rests on one voice: a single source, by {voice}. An independent source would test it.",
                p.title
            )
        } else {
            format!(
                "\"{}\" rests on a single source. An independent one would test it.",
                p.title
            )
        };
        out.push(KnowledgeGap {
            id: format!("one-voice-{}", slug(&p.rel_path)),
            kind: GapKind::OneVoice,
            subjects: vec![p.rel_path.clone()],
            explanation,
        });
    }
    out
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

#[cfg(test)]
mod one_voice_tests {
    use super::find_gaps;
    use wiki_proto::graph::GapKind;

    fn write(root: &std::path::Path, rel: &str, body: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    /// Two sources by one author: one voice. Add a second author: not.
    #[test]
    fn a_page_on_one_authors_sources_is_one_voice() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "Sources/a-1.md", "---\ntitle: A\ntype: source\nauthor: Nils Glenn\n---\n# A\n");
        write(root, "Sources/b-2.md", "---\ntitle: B\ntype: source\nauthor: Nils Glenn\n---\n# B\n");
        write(root, "Sources/c-3.md", "---\ntitle: C\ntype: source\nauthor: Mark Smith\n---\n# C\n");
        write(
            root,
            "Topics/One.md",
            "---\ntitle: One\ntype: topic\nsources: [\"raw/sources/a-1.md\", \"raw/sources/b-2.md\"]\n---\n# One\n[[Two]]\n",
        );
        write(
            root,
            "Topics/Two.md",
            "---\ntitle: Two\ntype: topic\nsources: [\"raw/sources/a-1.md\", \"raw/sources/c-3.md\"]\n---\n# Two\n[[One]]\n",
        );
        let gaps = find_gaps(root).unwrap();
        let one: Vec<_> = gaps.iter().filter(|g| matches!(g.kind, GapKind::OneVoice)).collect();
        assert_eq!(one.len(), 1, "{one:?}");
        assert_eq!(one[0].subjects, ["Topics/One.md"]);
        assert!(one[0].explanation.contains("all by Nils Glenn"), "{}", one[0].explanation);
    }
}
