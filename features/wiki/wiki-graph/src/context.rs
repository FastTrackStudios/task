//! Token-budgeted **context subgraph** for LLM prompting.
//!
//! Given a query (free text or node id) + a token budget,
//! returns a relevance-ranked subgraph rendered as markdown
//! suitable for stuffing into an LLM system prompt. The
//! agent receives a structural view of the wiki — page
//! summaries + outbound connections — instead of having to
//! grep for full files.
//!
//! This is the headline LLM-context-feeding capability
//! borrowed from graphify (whose pitch is "query the graph
//! instead of grepping through files"). For us it works on
//! the existing markdown wiki today; once code extraction
//! lands the same surface returns mixed page/symbol nodes.
//!
//! # Algorithm
//!
//! 1. Build the relevance-scored graph (existing
//!    [`crate::build::build_graph`] with `query` filter).
//! 2. Walk nodes in relevance order — `link_count` is a
//!    reasonable proxy for centrality.
//! 3. For each candidate: fetch its body, extract a 1-3
//!    sentence summary, list its outbound wikilinks.
//! 4. Accumulate into the markdown output until the token
//!    budget is exhausted. Token estimate is `chars / 4`
//!    (close enough for prompt sizing; swap for a real
//!    tokenizer when we ingest API limits matter).
//!
//! # Output shape
//!
//! ```text
//! # Wiki context for: <query>
//!
//! <N> nodes selected (~<M> tokens, budget <B>).
//!
//! ## [[Page Title]] · <type> · cluster <id>
//! <summary>
//! **Connects to**: [[A]] · [[B]] · [[C]]
//!
//! ## [[Another Page]] · <type>
//! …
//! ```

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use wiki_proto::graph::{GraphEdge, GraphOpts, RelevanceScore, RelevanceWeights};

use crate::scan::{ScanError, scan_notes, scan_wiki};
use crate::scoring::{Indices, score_graph};

/// Node-id prefix for prose notes merged in via
/// [`ContextOpts::notes_root`]. Callers injecting
/// [`ContextOpts::extra_edges`] address a note as
/// `format!("{NOTE_ID_PREFIX}{vault_rel_path}")`.
pub const NOTE_ID_PREFIX: &str = "note:/";

/// Caller knobs for [`build_context`].
#[derive(Debug, Clone)]
pub struct ContextOpts {
    /// Free-text query or node id. Empty string returns
    /// the top global-centrality nodes (a "what is this
    /// wiki" view).
    pub query: String,
    /// Filter by `type:` frontmatter — `entity` /
    /// `concept` / `source` / etc. Empty = all types.
    pub node_type: String,
    /// Soft token cap. Estimator is `chars / 4`; the actual
    /// LLM token count will be close but not exact. The
    /// builder stops adding nodes once the estimate exceeds
    /// this. A summary already in progress is still
    /// included (no mid-paragraph cutoff).
    pub budget_tokens: usize,
    /// Hard ceiling on nodes regardless of budget. `0` =
    /// unlimited. Useful when the caller wants "top 10
    /// relevant pages" and doesn't care about token cost.
    pub max_nodes: usize,
    /// How many leading body characters to keep per page.
    /// Default 600 (≈ 150 tokens) — enough for a couple
    /// paragraphs and an example.
    pub summary_chars: usize,
    /// Also scan this prose-notes tree (typically the org
    /// vault) and merge its pages into the graph as
    /// `note:/<rel_path>` nodes. Notes and wiki pages link
    /// to each other through the same case-insensitive
    /// title/stem resolution wikilinks already use, so
    /// cross-tree edges emerge without extra plumbing.
    /// `None` = wiki-only (previous behavior).
    pub notes_root: Option<PathBuf>,
    /// Extra edges to overlay, as `(node_id, node_id)`
    /// pairs — the caller's channel for typed-link-store
    /// edges (user-asserted links that aren't `[[wikilinks]]`
    /// in any body). Pairs whose endpoints aren't in the
    /// graph, self-pairs, and already-connected pairs are
    /// dropped; survivors score as a direct link.
    pub extra_edges: Vec<(String, String)>,
}

impl Default for ContextOpts {
    fn default() -> Self {
        Self {
            query: String::new(),
            node_type: String::new(),
            budget_tokens: 8000,
            max_nodes: 0,
            summary_chars: 600,
            notes_root: None,
            extra_edges: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ContextResult {
    /// The rendered markdown — paste straight into an LLM
    /// system or user message.
    pub markdown: String,
    /// Node ids (vault-relative paths) included in the
    /// output, in the order they appear.
    pub included: Vec<String>,
    /// Estimated tokens of the rendered markdown.
    pub tokens_estimate: usize,
    /// How many nodes the relevance pass surfaced before
    /// the budget cap. `nodes_considered - included.len()`
    /// is the count that didn't fit.
    pub nodes_considered: usize,
}

/// Build a token-budgeted context subgraph for `vault_root`
/// per the given [`ContextOpts`]. Pure I/O + computation; no
/// LLM calls.
pub fn build_context(vault_root: &Path, opts: ContextOpts) -> Result<ContextResult, ScanError> {
    let mut pages = scan_wiki(vault_root)?;
    if let Some(notes_root) = &opts.notes_root {
        pages.extend(scan_notes(notes_root)?);
    }
    let by_path: HashMap<String, &crate::parse::Page> =
        pages.iter().map(|p| (p.rel_path.clone(), p)).collect();
    let by_title: HashMap<String, &crate::parse::Page> =
        pages.iter().map(|p| (p.title.to_lowercase(), p)).collect();

    // Build the (already-filtered) graph.
    let idx = Indices::build(&pages);
    let weights = RelevanceWeights::default();
    let graph_opts = GraphOpts {
        query: opts.query.clone(),
        node_type: opts.node_type.clone(),
        weights: Some(weights),
        limit: 0,
    };
    let mut graph = score_graph(
        &pages,
        &idx,
        &graph_opts.weights.unwrap_or_default(),
        &graph_opts.query,
        &graph_opts.node_type,
    );

    // Overlay caller-supplied edges (typed-link store). Scored
    // as a direct link — a user-asserted link is at least as
    // strong a relevance signal as a body wikilink.
    if !opts.extra_edges.is_empty() {
        let node_idx: HashMap<String, usize> = graph
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.id.clone(), i))
            .collect();
        let mut connected: HashSet<(String, String)> = graph
            .edges
            .iter()
            .map(|e| ordered_pair(&e.source, &e.target))
            .collect();
        let weights = RelevanceWeights::default();
        for (a, b) in &opts.extra_edges {
            if a == b {
                continue;
            }
            let (Some(&ia), Some(&ib)) = (node_idx.get(a.as_str()), node_idx.get(b.as_str()))
            else {
                continue;
            };
            if !connected.insert(ordered_pair(a, b)) {
                continue;
            }
            graph.edges.push(GraphEdge {
                source: a.clone(),
                target: b.clone(),
                weight: weights.direct_link,
                signals: RelevanceScore {
                    direct_link: weights.direct_link,
                    source_overlap: 0.0,
                    adamic_adar: 0.0,
                    type_affinity: 0.0,
                    total: weights.direct_link,
                },
            });
            graph.nodes[ia].link_count += 1;
            graph.nodes[ib].link_count += 1;
        }
    }

    // Sort by relevance (descending). With no query, this is
    // pure degree; with a query the filter already happened
    // inside `score_graph`. Ties break by id for stable output.
    graph
        .nodes
        .sort_by(|a, b| b.link_count.cmp(&a.link_count).then(a.id.cmp(&b.id)));

    let nodes_considered = graph.nodes.len();
    let mut included: Vec<String> = Vec::new();
    let mut out = String::new();

    let q_for_header = if opts.query.is_empty() {
        "(top-centrality view)".to_string()
    } else {
        opts.query.clone()
    };
    out.push_str(&format!("# Wiki context for: {q_for_header}\n\n"));

    // Reserve a few hundred chars of slack for the header
    // + footer so we don't over-shoot the budget rendering
    // the trailing summary line.
    let body_budget = opts.budget_tokens.saturating_mul(4).saturating_sub(400);
    let cap_nodes = if opts.max_nodes == 0 {
        usize::MAX
    } else {
        opts.max_nodes
    };
    let mut body = String::new();

    for n in &graph.nodes {
        if included.len() >= cap_nodes {
            break;
        }
        let Some(page) = by_path.get(&n.id) else {
            continue;
        };
        let summary = first_paragraph(&page.body, opts.summary_chars);
        let connects = render_connects(page, &by_title, &by_path);
        let entry = format_entry(n, &summary, &connects);
        if body.len() + entry.len() > body_budget {
            break;
        }
        body.push_str(&entry);
        included.push(n.id.clone());
    }

    let count_line = format!(
        "{} of {nodes_considered} nodes selected (~{} chars, budget {} chars).\n\n",
        included.len(),
        body.len(),
        opts.budget_tokens * 4,
    );
    out.push_str(&count_line);
    out.push_str(&body);

    let tokens_estimate = out.len() / 4;
    Ok(ContextResult {
        markdown: out,
        included,
        tokens_estimate,
        nodes_considered,
    })
}

/// Canonical undirected key for an edge (edges are scored
/// undirected; overlay dedup must match).
fn ordered_pair(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}

fn format_entry(n: &wiki_proto::graph::GraphNode, summary: &str, connects: &[String]) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "## [[{}]] · {}{}\n",
        n.label,
        if n.node_type.is_empty() {
            "untyped"
        } else {
            n.node_type.as_str()
        },
        if n.community.is_empty() {
            String::new()
        } else {
            format!(" · cluster {}", n.community)
        },
    ));
    s.push_str(summary);
    if !summary.ends_with('\n') {
        s.push('\n');
    }
    if !connects.is_empty() {
        s.push_str("**Connects to:** ");
        s.push_str(
            &connects
                .iter()
                .map(|t| format!("[[{t}]]"))
                .collect::<Vec<_>>()
                .join(" · "),
        );
        s.push('\n');
    }
    s.push('\n');
    s
}

/// Extract the first paragraph (up to a blank line) from
/// the page body, capped at `max_chars`. Strips the
/// leading `# Heading` line if present so the summary is
/// just the content.
fn first_paragraph(body: &str, max_chars: usize) -> String {
    let mut iter = body.lines().peekable();
    // Skip leading blanks + a single `# Title` line.
    while iter.peek().is_some_and(|l| l.trim().is_empty()) {
        iter.next();
    }
    if iter.peek().is_some_and(|l| l.trim_start().starts_with('#')) {
        iter.next();
    }
    while iter.peek().is_some_and(|l| l.trim().is_empty()) {
        iter.next();
    }
    // Accumulate until blank line or max_chars.
    let mut out = String::new();
    for line in iter {
        if line.trim().is_empty() {
            break;
        }
        if out.len() + line.len() + 1 > max_chars {
            // Hit the cap mid-paragraph — include what fits + ellipsis.
            let remaining = max_chars.saturating_sub(out.len()).saturating_sub(1);
            let trimmed: String = line.chars().take(remaining).collect();
            out.push_str(&trimmed);
            out.push('…');
            break;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Up to 6 outbound wikilinks resolved to existing pages.
/// Drops links that don't resolve (those show up as
/// gaps in `find_gaps`).
fn render_connects(
    page: &crate::parse::Page,
    by_title: &HashMap<String, &crate::parse::Page>,
    by_path: &HashMap<String, &crate::parse::Page>,
) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for raw in &page.outlinks {
        let normalized = raw.trim();
        // Resolve by title (most common) or by basename match.
        let resolved = by_title
            .get(&normalized.to_lowercase())
            .or_else(|| {
                by_path.values().find(|p| {
                    let stem = std::path::Path::new(&p.rel_path)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("");
                    stem.eq_ignore_ascii_case(normalized)
                })
            })
            .map(|p| p.title.clone());
        let label = resolved.unwrap_or_else(|| normalized.to_string());
        if seen.insert(label.clone()) {
            out.push(label);
        }
        if out.len() >= 6 {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_paragraph_strips_heading() {
        let body = "\n# My Title\n\nFirst paragraph here.\nSecond line of it.\n\nNot this.\n";
        let s = first_paragraph(body, 200);
        assert!(s.contains("First paragraph here."));
        assert!(s.contains("Second line of it."));
        assert!(!s.contains("Not this."));
        assert!(!s.contains("# My Title"));
    }

    fn write(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn notes_root_merges_and_extra_edges_overlay() {
        let dir = tempfile::tempdir().unwrap();
        let wiki = dir.path().join("wiki");
        let vault = dir.path().join("vault");
        write(
            &wiki,
            "Concepts/Grace.md",
            "---\ntype: concept\n---\nUnmerited favor.\n",
        );
        write(
            &vault,
            "Notes/study.md",
            "Reflections on [[Grace]] from Sunday.\n",
        );
        write(&vault, "Notes/prayer.md", "No wikilinks here.\n");

        let result = build_context(
            &wiki,
            ContextOpts {
                notes_root: Some(vault),
                // User-asserted link between the two notes —
                // no [[wikilink]] exists between them.
                extra_edges: vec![(
                    format!("{NOTE_ID_PREFIX}Notes/study.md"),
                    format!("{NOTE_ID_PREFIX}Notes/prayer.md"),
                )],
                ..ContextOpts::default()
            },
        )
        .unwrap();

        // Wiki page + both notes are all in the graph; the
        // note→wiki wikilink and the overlaid note↔note edge
        // give every node degree ≥ 1.
        assert!(result.included.contains(&"Concepts/Grace.md".to_string()));
        assert!(
            result
                .included
                .contains(&format!("{NOTE_ID_PREFIX}Notes/study.md"))
        );
        assert!(
            result
                .included
                .contains(&format!("{NOTE_ID_PREFIX}Notes/prayer.md"))
        );
        assert!(result.markdown.contains("[[Grace]]"));
    }

    #[test]
    fn first_paragraph_caps_at_max_chars() {
        let body = "Aaaaaaaaaaaaaaaaaaaa Bbbbbbbbbbbbbbbbbbbb.\n";
        let s = first_paragraph(body, 10);
        assert!(s.contains('…'));
        assert!(s.len() <= 12);
    }
}
