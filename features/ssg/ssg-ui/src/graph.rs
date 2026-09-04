//! The link graph, drawn as static SVG.
//!
//! `view-knowledge-graph`'s own `KnowledgeGraphView` pans, zooms, hovers
//! and dims, all through hooks and signals — so it renders differently
//! on its first frame than after a wheel event, which is exactly what a
//! hydrated pre-render must not do. What that crate *also* has is a
//! layout: Fruchterman–Reingold, deterministic, no RNG, no worker.
//!
//! So the layout is shared and only the rendering differs. Here every
//! node is an `<a>` and every edge a `<line>`: a few kilobytes of markup
//! that paint with the rest of the page, before the bundle has arrived.
//!
//! What is lost is hover-to-highlight and drag-to-pan. What is kept is
//! what the graph is for on a guide page: seeing what a concept touches,
//! and clicking through to it. A site that wants the full interactive
//! map can still put `KnowledgeGraphView` on a route of its own — that
//! route just should not be in the pre-rendered set.

use dioxus::prelude::*;
use ssg_vault::StaticVault;
use view_knowledge_graph::build::build_wiki_graph;
use view_knowledge_graph::layout::{LayoutConfig, bounds, layout, node_radius};
use view_knowledge_graph::model::{ColorMode, WikiGraph, community_color, kind_color};
use view_knowledge_graph::parse::WikiFile;

/// Build the whole vault's link graph.
///
/// Reads each page's `source` — the note verbatim, frontmatter and all —
/// because that is what the graph builder classifies nodes from (`type:`)
/// and draws edges from (`[[wikilinks]]`, including the ones in a nav
/// footer that the rendered prose drops).
///
/// This runs during the server render, once per page baked, and never in
/// a reader's browser.
#[must_use]
pub fn vault_graph(vault: StaticVault) -> WikiGraph {
    let files: Vec<WikiFile> = vault
        .pages
        .iter()
        .map(|page| WikiFile {
            name: format!("{}.md", page.slug),
            path: format!("{}.md", page.slug),
            content: page.source.to_owned(),
        })
        .collect();
    build_wiki_graph(&files)
}

/// The subgraph of `graph` within one hop of `slug`.
///
/// This is the view that belongs beside a note: the concept you are
/// reading and everything it touches. The whole graph beside a single
/// page is a picture of the vault, not of the page.
#[must_use]
pub fn local_graph(graph: &WikiGraph, slug: &str) -> WikiGraph {
    let mut keep: Vec<String> = vec![slug.to_owned()];
    for edge in &graph.edges {
        if edge.source == slug && !keep.contains(&edge.target) {
            keep.push(edge.target.clone());
        }
        if edge.target == slug && !keep.contains(&edge.source) {
            keep.push(edge.source.clone());
        }
    }

    WikiGraph {
        nodes: graph
            .nodes
            .iter()
            .filter(|n| keep.contains(&n.id))
            .cloned()
            .collect(),
        // Every edge between kept nodes, not just those touching `slug`:
        // an edge between two neighbours is exactly the "these two are
        // also related" fact that makes a local graph worth drawing.
        edges: graph
            .edges
            .iter()
            .filter(|e| keep.contains(&e.source) && keep.contains(&e.target))
            .cloned()
            .collect(),
        communities: graph.communities.clone(),
    }
}

/// A link graph as static, clickable SVG.
///
/// Renders nothing when the graph has fewer than two nodes: an empty box
/// beside the prose reads as broken, where absence reads as "this page
/// stands alone".
#[component]
pub fn VaultGraph(
    /// The graph to draw — [`vault_graph`] for the whole vault, or
    /// [`local_graph`] for one page's neighbourhood.
    graph: WikiGraph,
    /// URL prefix the vault is published under; a node links to
    /// `{base}/{id}`.
    base: &'static str,
    /// The page being read, drawn larger and always labelled.
    #[props(default)]
    current: Option<String>,
    /// Colour by page kind or by detected community.
    #[props(default = ColorMode::Community)]
    color_mode: ColorMode,
    /// Layout spacing multiplier. Below 1.0 tightens the layout, which
    /// is usually right for a local graph in a narrow rail.
    #[props(default = 1.0)]
    spacing: f32,
    /// Node size multiplier.
    #[props(default = 1.0)]
    node_scale: f32,
    /// Class for the wrapping `<svg>`.
    #[props(default = "ssg-graph")]
    class: &'static str,
) -> Element {
    if graph.nodes.len() < 2 {
        return rsx! {};
    }

    let positions = layout(
        &graph.nodes,
        &graph.edges,
        LayoutConfig {
            spacing,
            iterations: None,
        },
    );
    let (min_x, min_y, max_x, max_y) = bounds(&positions);

    // Labels are drawn beside their node, so the frame has to leave room
    // for them or the outermost ones clip. A fraction of the extent
    // rather than a fixed number of units: the layout's scale varies
    // with node count and spacing.
    let pad = ((max_x - min_x).max(max_y - min_y) * 0.12).max(24.0);
    let view_box = format!(
        "{} {} {} {}",
        min_x - pad,
        min_y - pad,
        (max_x - min_x) + pad * 2.0,
        (max_y - min_y) + pad * 2.0
    );

    let max_links = graph.nodes.iter().map(|n| n.link_count).max().unwrap_or(0);
    let node_count = graph.nodes.len();
    let current = current.unwrap_or_default();

    rsx! {
        svg {
            class,
            view_box: "{view_box}",
            role: "img",
            // Without a title the graph is an unlabelled image to a
            // screen reader; the node list below it is the real content
            // either way, but the frame should say what it is.
            title { "Links between pages in this guide" }

            g { class: "ssg-graph-edges",
                for (i , edge) in graph.edges.iter().enumerate() {
                    if let (Some(from), Some(to)) = (positions.get(&edge.source), positions.get(&edge.target)) {
                        line {
                            key: "{i}",
                            x1: "{from.x}",
                            y1: "{from.y}",
                            x2: "{to.x}",
                            y2: "{to.y}",
                            class: "ssg-graph-edge",
                        }
                    }
                }
            }

            g { class: "ssg-graph-nodes",
                for node in graph.nodes.iter() {
                    if let Some(pos) = positions.get(&node.id) {
                        {
                            let is_current = node.id == current;
                            let radius = node_radius(node.link_count, max_links, node_count, node_scale)
                                * if is_current { 1.35 } else { 1.0 };
                            let fill = match color_mode {
                                ColorMode::Kind => kind_color(&node.kind),
                                ColorMode::Community => community_color(node.community),
                            };
                            rsx! {
                                a {
                                    key: "{node.id}",
                                    href: "{base}/{node.id}",
                                    class: if is_current { "ssg-graph-node ssg-graph-node-current" } else { "ssg-graph-node" },
                                    circle {
                                        cx: "{pos.x}",
                                        cy: "{pos.y}",
                                        r: "{radius}",
                                        fill: "{fill}",
                                    }
                                    text {
                                        x: "{pos.x + radius + 4.0}",
                                        y: "{pos.y + 4.0}",
                                        class: "ssg-graph-label",
                                        "{node.label}"
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
