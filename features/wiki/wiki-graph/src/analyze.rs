//! Post-extraction analysis: cross-file resolution + centrality + summary report.
//!
//! After [`crate::code_extract::scan_code_tree`] produces a
//! flat list of per-file extractions, this module:
//!
//! 1. **Resolves inferred edges** — collapse `name:foo` and
//!    `ext:foo::bar` placeholders into real `code:rust:…:foo`
//!    node ids when an unambiguous match exists. Multi-match
//!    cases stay as placeholders + get marked `Ambiguous`.
//! 2. **Computes centrality** — in-degree, out-degree,
//!    combined PageRank-ish score per node. Surfaces the
//!    "god-nodes" (high coupling) and the "leaves" (high
//!    out-degree, low in-degree — fan-out hubs).
//! 3. **Renders [`GraphReport`]** — a deterministic markdown
//!    summary suitable for `CODE_REPORT.md` (matches
//!    graphify's report shape). The LLM-augmented "suggested
//!    questions" addition is a follow-up.

use std::collections::{HashMap, HashSet};

use crate::code_extract::{CodeEdge, CodeExtraction, CodeNode, Confidence, Relation, SymbolKind};

/// One merged graph + analysis result.
#[derive(Debug, Clone, Default)]
pub struct AnalyzedGraph {
    pub nodes: Vec<CodeNode>,
    pub edges: Vec<CodeEdge>,
    /// `node_id → in-degree` (incoming edges).
    pub in_degree: HashMap<String, u32>,
    /// `node_id → out-degree` (outgoing edges).
    pub out_degree: HashMap<String, u32>,
    /// Combined importance score: 0.7 × in + 0.3 × out.
    /// Cheap stand-in for PageRank; correlates well for the
    /// god-node surface we care about.
    pub centrality: HashMap<String, f64>,
    /// Edges whose target was resolved to a concrete node id
    /// (formerly `name:foo` / `ext:foo`). Useful for stats.
    pub resolved_edges: u32,
    /// Edges still pointing to `name:` / `ext:` placeholders.
    /// Each one is a candidate for the AMBIGUOUS bucket.
    pub unresolved_edges: u32,
}

/// Merge a slice of per-file extractions, resolve placeholder
/// edge targets, and compute centrality.
#[must_use]
pub fn analyze(extractions: &[CodeExtraction]) -> AnalyzedGraph {
    let mut nodes: Vec<CodeNode> = Vec::new();
    let mut edges: Vec<CodeEdge> = Vec::new();
    for ex in extractions {
        nodes.extend(ex.nodes.iter().cloned());
        edges.extend(ex.edges.iter().cloned());
    }

    // Index nodes by id (exact match) and by label (for
    // resolving `name:foo` placeholders). The label map is
    // 1-to-many — store all candidates so we can mark
    // multi-match resolutions as Ambiguous.
    let by_id: HashMap<&str, &CodeNode> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut by_label: HashMap<&str, Vec<&CodeNode>> = HashMap::new();
    for n in &nodes {
        by_label.entry(n.label.as_str()).or_default().push(n);
    }

    let mut resolved = 0u32;
    let mut unresolved = 0u32;
    for edge in &mut edges {
        if by_id.contains_key(edge.target.as_str()) {
            continue; // already a concrete id
        }
        if let Some(label) = edge.target.strip_prefix("name:") {
            match by_label.get(label).map(Vec::as_slice) {
                Some([one]) => {
                    // Unambiguous resolution.
                    edge.target = one.id.clone();
                    if edge.confidence == Confidence::Inferred {
                        // Keep Inferred — we resolved the
                        // name but the call site type isn't
                        // proven. Upgrading to Extracted
                        // would over-promise.
                    }
                    resolved += 1;
                }
                Some(_multi) => {
                    edge.confidence = Confidence::Ambiguous;
                    unresolved += 1;
                }
                None => {
                    unresolved += 1;
                }
            }
        } else if edge.target.starts_with("ext:") {
            // External imports / impls — no in-tree node to
            // resolve to. Leave alone; downstream tools can
            // discriminate by the `ext:` prefix.
        } else {
            unresolved += 1;
        }
    }

    let (in_degree, out_degree, centrality) = compute_centrality(&nodes, &edges);

    AnalyzedGraph {
        nodes,
        edges,
        in_degree,
        out_degree,
        centrality,
        resolved_edges: resolved,
        unresolved_edges: unresolved,
    }
}

fn compute_centrality(
    nodes: &[CodeNode],
    edges: &[CodeEdge],
) -> (
    HashMap<String, u32>,
    HashMap<String, u32>,
    HashMap<String, f64>,
) {
    let node_ids: HashSet<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
    let mut in_deg: HashMap<String, u32> = HashMap::new();
    let mut out_deg: HashMap<String, u32> = HashMap::new();
    for e in edges {
        if node_ids.contains(e.source.as_str()) {
            *out_deg.entry(e.source.clone()).or_insert(0) += 1;
        }
        if node_ids.contains(e.target.as_str()) {
            *in_deg.entry(e.target.clone()).or_insert(0) += 1;
        }
    }
    let max_in = in_deg.values().copied().max().unwrap_or(1).max(1) as f64;
    let max_out = out_deg.values().copied().max().unwrap_or(1).max(1) as f64;
    let centrality: HashMap<String, f64> = nodes
        .iter()
        .map(|n| {
            let ind = f64::from(in_deg.get(&n.id).copied().unwrap_or(0));
            let out = f64::from(out_deg.get(&n.id).copied().unwrap_or(0));
            let score = 0.7 * (ind / max_in) + 0.3 * (out / max_out);
            (n.id.clone(), score)
        })
        .collect();
    (in_deg, out_deg, centrality)
}

/// Get the top-N most-central nodes ("god nodes").
#[must_use]
pub fn god_nodes(g: &AnalyzedGraph, n: usize) -> Vec<&CodeNode> {
    let mut ranked: Vec<&CodeNode> = g.nodes.iter().collect();
    ranked.sort_by(|a, b| {
        let sa = g.centrality.get(&a.id).copied().unwrap_or(0.0);
        let sb = g.centrality.get(&b.id).copied().unwrap_or(0.0);
        sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
    });
    ranked.into_iter().take(n).collect()
}

/// Get the top-N "leaf hubs": nodes that call many things
/// but are called by few. These are typically `main()`,
/// entrypoint handlers, top-level orchestrators.
#[must_use]
pub fn fan_out_hubs(g: &AnalyzedGraph, n: usize) -> Vec<&CodeNode> {
    let mut ranked: Vec<(&CodeNode, i32)> = g
        .nodes
        .iter()
        .map(|node| {
            let out = g.out_degree.get(&node.id).copied().unwrap_or(0) as i32;
            let ind = g.in_degree.get(&node.id).copied().unwrap_or(0) as i32;
            (node, out - ind)
        })
        .collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1));
    ranked.into_iter().take(n).map(|(n, _)| n).collect()
}

/// Per-kind histogram. Useful for the report header.
#[must_use]
pub fn kind_counts(g: &AnalyzedGraph) -> HashMap<SymbolKind, usize> {
    let mut out: HashMap<SymbolKind, usize> = HashMap::new();
    for n in &g.nodes {
        *out.entry(n.kind).or_insert(0) += 1;
    }
    out
}

/// Per-relation histogram for edges.
#[must_use]
pub fn relation_counts(g: &AnalyzedGraph) -> HashMap<Relation, usize> {
    let mut out: HashMap<Relation, usize> = HashMap::new();
    for e in &g.edges {
        *out.entry(e.relation).or_insert(0) += 1;
    }
    out
}

/// **Affected subgraph** — given a set of changed files,
/// return the nodes defined in those files plus every node
/// that references them (1-hop callers) and every node those
/// changed nodes call (1-hop dependencies). The graphify-style
/// "what does this change touch?" query.
///
/// Paths are compared by `source_file` exactly — pass them
/// relative to whatever root `analyze` was run against.
#[must_use]
pub fn affected_by<'a, P: AsRef<std::path::Path>>(
    g: &'a AnalyzedGraph,
    changed_files: &[P],
) -> AffectedResult<'a> {
    let changed_set: HashSet<&std::path::Path> = changed_files.iter().map(AsRef::as_ref).collect();
    let in_changed: Vec<&CodeNode> = g
        .nodes
        .iter()
        .filter(|n| changed_set.contains(n.source_file.as_path()))
        .collect();
    let in_changed_ids: HashSet<&str> = in_changed.iter().map(|n| n.id.as_str()).collect();
    let by_id: HashMap<&str, &CodeNode> = g.nodes.iter().map(|n| (n.id.as_str(), n)).collect();

    let mut callers: HashSet<&str> = HashSet::new();
    let mut deps: HashSet<&str> = HashSet::new();
    for e in &g.edges {
        if in_changed_ids.contains(e.target.as_str()) && !in_changed_ids.contains(e.source.as_str())
        {
            callers.insert(e.source.as_str());
        }
        if in_changed_ids.contains(e.source.as_str()) && !in_changed_ids.contains(e.target.as_str())
        {
            deps.insert(e.target.as_str());
        }
    }

    AffectedResult {
        in_changed,
        callers: callers
            .iter()
            .filter_map(|id| by_id.get(id).copied())
            .collect(),
        dependencies_internal: deps
            .iter()
            .filter_map(|id| by_id.get(id).copied())
            .collect(),
        dependencies_external: deps
            .iter()
            .filter(|id| !by_id.contains_key(*id))
            .map(|s| (*s).to_string())
            .collect(),
    }
}

/// Output of [`affected_by`] — what a code change touches.
#[derive(Debug, Clone)]
pub struct AffectedResult<'a> {
    /// Nodes whose `source_file` is in the changed set.
    pub in_changed: Vec<&'a CodeNode>,
    /// In-tree nodes (outside the changed files) that
    /// reference any changed node — i.e. would need to
    /// re-test / be re-reviewed after this change.
    pub callers: Vec<&'a CodeNode>,
    /// In-tree nodes that the changed nodes call — useful
    /// to know what abstractions the change depends on.
    pub dependencies_internal: Vec<&'a CodeNode>,
    /// External targets (stdlib / other crates) — kept as
    /// raw `ext:` / `name:` ids for caller inspection.
    pub dependencies_external: Vec<String>,
}

/// Render a deterministic markdown report — drop-in for
/// graphify's `GRAPH_REPORT.md`. No LLM (the augmented form
/// with "suggested questions" lives in `agent-wiki`).
#[must_use]
pub fn render_report(g: &AnalyzedGraph, top_n: usize) -> String {
    let mut out = String::new();
    out.push_str("# Code Graph Report\n\n");
    out.push_str(&format!(
        "{} nodes · {} edges · {} resolved · {} unresolved\n\n",
        g.nodes.len(),
        g.edges.len(),
        g.resolved_edges,
        g.unresolved_edges
    ));

    // Kind histogram
    let kinds = kind_counts(g);
    if !kinds.is_empty() {
        out.push_str("## By kind\n\n");
        let mut items: Vec<_> = kinds.iter().collect();
        items.sort_by(|a, b| b.1.cmp(a.1));
        for (k, n) in items {
            out.push_str(&format!("- {}: {n}\n", k.as_str()));
        }
        out.push('\n');
    }

    // Relation histogram
    let rels = relation_counts(g);
    if !rels.is_empty() {
        out.push_str("## By relation\n\n");
        let mut items: Vec<_> = rels.iter().collect();
        items.sort_by(|a, b| b.1.cmp(a.1));
        for (r, n) in items {
            out.push_str(&format!("- {}: {n}\n", r.as_str()));
        }
        out.push('\n');
    }

    // God nodes
    out.push_str(&format!("## Top {top_n} god-nodes (most coupled)\n\n"));
    out.push_str("Highest centrality (0.7 × in + 0.3 × out). High-impact change risks.\n\n");
    for n in god_nodes(g, top_n) {
        let score = g.centrality.get(&n.id).copied().unwrap_or(0.0);
        let ind = g.in_degree.get(&n.id).copied().unwrap_or(0);
        let out_d = g.out_degree.get(&n.id).copied().unwrap_or(0);
        out.push_str(&format!(
            "- `{}` · {} · in={ind} out={out_d} · score={score:.3} · `{}:L{}`\n",
            n.label,
            n.kind.as_str(),
            n.source_file.display(),
            n.line_start + 1
        ));
    }
    out.push('\n');

    // Fan-out hubs
    out.push_str(&format!("## Top {top_n} fan-out hubs (entrypoints)\n\n"));
    out.push_str("High out / low in — typically `main()`, handlers, orchestrators.\n\n");
    for n in fan_out_hubs(g, top_n) {
        let ind = g.in_degree.get(&n.id).copied().unwrap_or(0);
        let out_d = g.out_degree.get(&n.id).copied().unwrap_or(0);
        out.push_str(&format!(
            "- `{}` · {} · in={ind} out={out_d} · `{}:L{}`\n",
            n.label,
            n.kind.as_str(),
            n.source_file.display(),
            n.line_start + 1
        ));
    }
    out.push('\n');

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn extract(src: &str) -> CodeExtraction {
        crate::code_extract::extract_source(
            Path::new("test.rs"),
            src,
            crate::code_extract::CodeLang::Rust,
        )
        .unwrap()
    }

    #[test]
    fn resolves_call_target_when_unambiguous() {
        let ex = extract(
            r"
                pub fn caller() { callee(); }
                pub fn callee() {}
            ",
        );
        let g = analyze(&[ex]);
        // `caller` should have a `Calls` edge resolved to
        // `callee`'s concrete id.
        let resolved: Vec<&CodeEdge> = g
            .edges
            .iter()
            .filter(|e| e.relation == Relation::Calls && e.target.starts_with("code:"))
            .collect();
        assert!(
            !resolved.is_empty(),
            "expected a resolved call edge, got edges: {:?}",
            g.edges
        );
    }

    #[test]
    fn marks_ambiguous_when_multiple_match() {
        // Two `mod`s each define `helper()`; a call to
        // `helper` from a top-level fn is Ambiguous.
        let ex = extract(
            r"
                pub fn caller() { helper(); }
                pub mod a { pub fn helper() {} }
                pub mod b { pub fn helper() {} }
            ",
        );
        let g = analyze(&[ex]);
        let ambig: Vec<&CodeEdge> = g
            .edges
            .iter()
            .filter(|e| e.confidence == Confidence::Ambiguous)
            .collect();
        assert!(!ambig.is_empty(), "expected an ambiguous edge");
    }

    #[test]
    fn god_nodes_ranks_callee_first() {
        // `target` is called by 3 fns. Should be top.
        let ex = extract(
            r"
                pub fn target() {}
                pub fn a() { target(); }
                pub fn b() { target(); }
                pub fn c() { target(); }
            ",
        );
        let g = analyze(&[ex]);
        let top = god_nodes(&g, 1);
        assert_eq!(top[0].label, "target");
    }

    #[test]
    fn affected_by_finds_callers_and_deps() {
        // Two files: `a.rs` defines `a_main`, `b.rs` defines
        // `b_helper`. `a_main` calls `b_helper`. Changing
        // `b.rs` → callers = [a_main]; changing `a.rs` →
        // deps include b_helper.
        let a_ex = crate::code_extract::extract_source(
            Path::new("a.rs"),
            "pub fn a_main() { b_helper(); }",
            crate::code_extract::CodeLang::Rust,
        )
        .unwrap();
        let b_ex = crate::code_extract::extract_source(
            Path::new("b.rs"),
            "pub fn b_helper() {}",
            crate::code_extract::CodeLang::Rust,
        )
        .unwrap();
        let g = analyze(&[a_ex, b_ex]);
        let aff = affected_by(&g, &[Path::new("b.rs")]);
        assert!(
            aff.callers.iter().any(|n| n.label == "a_main"),
            "expected a_main as caller, got: {:?}",
            aff.callers.iter().map(|n| &n.label).collect::<Vec<_>>()
        );
        let aff2 = affected_by(&g, &[Path::new("a.rs")]);
        assert!(
            aff2.dependencies_internal
                .iter()
                .any(|n| n.label == "b_helper"),
            "expected b_helper as dep"
        );
    }

    #[test]
    fn report_renders_sections() {
        let ex = extract("pub fn one() {} pub fn two() {} pub struct Foo;");
        let g = analyze(&[ex]);
        let report = render_report(&g, 5);
        assert!(report.contains("# Code Graph Report"));
        assert!(report.contains("## By kind"));
        assert!(report.contains("## Top 5 god-nodes"));
    }
}
