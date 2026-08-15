//! `/connections` — the typed-link knowledge web.
//!
//! Where `/wiki` graphs the vault's `[[wikilink]]` pages, this graphs the
//! generalized `links` feature: every `TypedLink` in the org
//! (`verse↔song↔sermon↔topic`) fetched from `LinksService`, turned into a
//! force-directed graph with [`view_knowledge_graph::build_link_graph`]
//! and rendered through the dumb [`KnowledgeGraphView`]. The
//! [`GraphFilters`] panel is the differentiator: a confidence floor and a
//! "typed links only" toggle, so you can dial the web down to just the
//! strongly-established connections (the quality-filtered, publishable
//! view).

use std::collections::HashSet;

use architect_ui::prelude::*;
use dioxus::prelude::*;
use view_knowledge_graph::{
    GraphFilterState, GraphFilters, KnowledgeGraphView, WikiGraph, apply_filters, build_link_graph,
};

use task_ui_core::feeds;
use task_ui_core::orgs::{OrgMeta, OrgSelection, selected_slugs};

/// The focal node plus its 1-hop neighbourhood — every edge touching
/// `focal` and the nodes on either end. Empty edge set ⇒ just the node.
fn focal_subgraph(g: &WikiGraph, focal: &str) -> WikiGraph {
    let mut keep: HashSet<&str> = HashSet::from([focal]);
    let mut edges = Vec::new();
    for e in &g.edges {
        if e.source == focal || e.target == focal {
            keep.insert(e.source.as_str());
            keep.insert(e.target.as_str());
            edges.push(e.clone());
        }
    }
    let nodes = g
        .nodes
        .iter()
        .filter(|n| keep.contains(n.id.as_str()))
        .cloned()
        .collect();
    WikiGraph {
        nodes,
        edges,
        communities: Vec::new(),
    }
}

#[component]
pub fn ConnectionsView() -> Element {
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();

    // Page-owned display + quality filters.
    let mut filters = use_signal(GraphFilterState::default);
    let mut node_scale = use_signal(|| 1.0_f32);
    let mut spacing = use_signal(|| 1.5_f32);
    // Focal node: when set, the graph shows just it + its neighbours.
    let mut focal = use_signal(|| None::<String>);

    // Fetch all links + build the graph once per (org) change.
    let graph = use_resource(move || async move {
        let slugs = selected_slugs(&selection.read(), &org_list.read());
        let slug = slugs
            .first()
            .cloned()
            .ok_or_else(|| "no organization selected".to_string())?;
        let links = fetch_link_graph(&slug).await?;
        Ok::<WikiGraph, String>(build_link_graph(&links, true))
    });

    let discovering = org_list.read().is_empty();
    let body = if discovering {
        render_loading()
    } else {
        match &*graph.read() {
            Some(Ok(g)) if g.nodes.is_empty() => render_empty(),
            Some(Ok(g)) => {
                let f = filters.read().clone();
                // Focus first (subgraph), then quality-filter what's shown.
                let focal_id = focal();
                let base = match &focal_id {
                    Some(id) => focal_subgraph(g, id),
                    None => g.clone(),
                };
                let filtered = apply_filters(&base.nodes, &base.edges, &f);
                let shown = WikiGraph {
                    nodes: filtered.nodes,
                    edges: filtered.edges,
                    communities: g.communities.clone(),
                };
                // Panel counts reflect the in-scope (focused) graph.
                let all_nodes = base.nodes.clone();
                let all_edges = base.edges.clone();
                let scale = node_scale();
                let space = spacing();
                let focus_label = focal_id
                    .as_deref()
                    .map(|t| t.split_once(':').map_or(t, |(_, id)| id).to_string());
                let neighbour_count = shown.nodes.len().saturating_sub(1);
                rsx! {
                    div { class: "relative min-h-0 flex-1 overflow-hidden rounded-xl border border-border/70 bg-card/30",
                        KnowledgeGraphView {
                            graph: shown,
                            node_scale: scale,
                            spacing: space,
                            on_node_click: move |id: String| focal.set(Some(id)),
                        }
                        if let Some(label) = focus_label {
                            div { class: "absolute left-3 top-3 flex items-center gap-2 rounded-lg border border-primary/40 bg-background/95 px-2.5 py-1.5 text-xs shadow-sm backdrop-blur",
                                span { class: "font-medium text-foreground", "{label}" }
                                span { class: "text-muted-foreground", "· {neighbour_count} connections" }
                                button {
                                    r#type: "button",
                                    class: "rounded px-1.5 py-0.5 text-muted-foreground hover:bg-accent hover:text-foreground",
                                    onclick: move |_| focal.set(None),
                                    "Show all"
                                }
                            }
                        }
                        div { class: "absolute right-3 top-3",
                            GraphFilters {
                                filters: f.clone(),
                                nodes: all_nodes,
                                edges: all_edges,
                                node_scale: scale,
                                spacing: space,
                                on_filters_change: move |next: GraphFilterState| filters.set(next),
                                on_node_scale_change: move |v: f32| node_scale.set(v),
                                on_spacing_change: move |v: f32| spacing.set(v),
                                on_reset: move |()| {
                                    filters.set(GraphFilterState::default());
                                    node_scale.set(1.0);
                                    spacing.set(1.5);
                                },
                            }
                        }
                    }
                }
            }
            Some(Err(e)) => rsx! {
                div { class: "rounded-xl border border-destructive/40 bg-destructive/10 px-4 py-3 text-sm",
                    "Couldn't build the connections graph: {e}"
                }
            },
            None => render_loading(),
        }
    };

    rsx! {
        div { class: "mx-auto flex h-full w-full max-w-6xl flex-col gap-4 p-4 sm:p-6 lg:p-8",
            header { class: "flex flex-col gap-1",
                span { class: "text-[0.7rem] font-semibold uppercase tracking-[0.18em] text-muted-foreground",
                    "Knowledge"
                }
                Heading { level: HeadingLevel::H1, class: "tracking-tight", "Connections" }
                Text {
                    variant: TextVariant::Muted,
                    "The typed-link web — verses, songs, sermons and topics as nodes; allusions, citations and cross-references as edges. Click a node to focus on its neighbourhood; use the filters to keep only the strong links."
                }
            }
            {body}
        }
    }
}

fn render_loading() -> Element {
    rsx! {
        div { class: "flex min-h-0 flex-1 items-center justify-center rounded-xl border border-border/70 bg-card/30",
            Text { variant: TextVariant::Muted, "Building the web…" }
        }
    }
}

fn render_empty() -> Element {
    rsx! {
        div { class: "flex min-h-0 flex-1 flex-col items-center justify-center gap-2 rounded-2xl border border-dashed border-border/70 bg-card/40 py-16 text-center",
            Heading { level: HeadingLevel::H3, "No connections yet" }
            Text { variant: TextVariant::Muted, "Annotate a verse, song or sermon to start weaving the web." }
        }
    }
}

// ── data ────────────────────────────────────────────────────────────
//
// This slice's RPCs live with the page that calls them, not in the
// shell's `feeds` module — that is the point of the split. `feeds!` and
// the fan-out helpers come from `task-ui-core`; see its `feeds` module
// for the shape.

feeds! {
    links_proto::LinksServiceClient {
        /// Every typed link in an org's graph — the verse↔song↔sermon↔topic web
        /// for the `/connections` page. Fetches at the lowest confidence and
        /// includes private links; the page filters client-side.
        fetch_link_graph() -> Vec<links_proto::TypedLink>
            = graph(links_proto::Confidence::Speculative, true) as "link graph";
    }
}
