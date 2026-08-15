//! `GraphFilters` — the filter / display-tuning panel.
//!
//! Port of the `showFilters` popover in graph-view.tsx. Dumb: the
//! current [`GraphFilterState`] + scalars come in via props, every edit
//! goes back out via a callback with the *next* value. The host owns
//! the state and re-feeds it to both this panel and the renderer.
//!
//! Sections: quick toggles (hide structural / isolated), a max-links
//! cap, node-size + spacing sliders, per-kind visibility checkboxes, a
//! list of explicitly-hidden nodes, and a live count footer.

use std::collections::BTreeMap;

use dioxus::prelude::*;
use architect_ui::lucide_dioxus::Funnel;

use crate::filters::{GraphFilterState, apply_filters};
use crate::model::{GraphEdge, GraphNode, kind_label};

#[derive(Props, Clone, PartialEq)]
pub struct GraphFiltersProps {
    pub filters: GraphFilterState,
    pub nodes: Vec<GraphNode>,
    #[props(default)]
    pub edges: Vec<GraphEdge>,
    #[props(default = 1.0)]
    pub node_scale: f32,
    #[props(default = 1.0)]
    pub spacing: f32,
    /// Fired with the next filter state on any filter edit.
    pub on_filters_change: EventHandler<GraphFilterState>,
    /// Fired with the next node-size multiplier.
    pub on_node_scale_change: EventHandler<f32>,
    /// Fired with the next spacing multiplier.
    pub on_spacing_change: EventHandler<f32>,
    /// Fired when "Reset" is clicked.
    pub on_reset: EventHandler<()>,
}

#[component]
pub fn GraphFilters(props: GraphFiltersProps) -> Element {
    let filters = props.filters.clone();
    let on_change = props.on_filters_change;
    let on_scale = props.on_node_scale_change;
    let on_spacing = props.on_spacing_change;
    let on_reset = props.on_reset;

    // Per-kind counts (stable order), kinds with at least one node.
    let mut counts: BTreeMap<String, u32> = BTreeMap::new();
    for n in &props.nodes {
        *counts.entry(n.kind.clone()).or_insert(0) += 1;
    }

    // Live counts for the footer.
    let visible = apply_filters(&props.nodes, &props.edges, &filters);
    let total_nodes = props.nodes.len();
    let total_edges = props.edges.len();
    let shown_nodes = visible.nodes.len();
    let shown_edges = visible.edges.len();

    let scale_pct = (props.node_scale * 100.0).round() as i32;
    let spacing_pct = (props.spacing * 100.0).round() as i32;
    let max_links_value = props
        .filters
        .max_links
        .map(|m| m.to_string())
        .unwrap_or_default();

    // Hidden-node rows (label looked up from nodes).
    let hidden_rows: Vec<(String, String)> = props
        .filters
        .hidden_node_ids
        .iter()
        .map(|id| {
            let label = props
                .nodes
                .iter()
                .find(|n| &n.id == id)
                .map_or_else(|| id.clone(), |n| n.label.clone());
            (id.clone(), label)
        })
        .collect();

    rsx! {
        div { class: "w-72 rounded-lg border border-border bg-background/95 p-3 text-xs shadow-lg backdrop-blur-sm",
            div { class: "mb-3 flex items-center justify-between",
                div { class: "flex items-center gap-1.5 font-semibold text-foreground",
                    Funnel { size: 14 }
                    "Graph filters"
                }
                button {
                    r#type: "button",
                    class: "rounded px-1.5 py-0.5 text-[10px] text-muted-foreground hover:bg-accent hover:text-foreground",
                    onclick: move |_| on_reset.call(()),
                    "Reset"
                }
            }

            div { class: "space-y-3",
                // Quick filters
                div { class: "space-y-1.5",
                    div { class: "font-medium text-muted-foreground", "Quick filters" }
                    label { class: "flex items-center gap-2",
                        input {
                            r#type: "checkbox",
                            checked: filters.hide_structural,
                            onchange: {
                                let f0 = filters.clone();
                                move |_| {
                                    let mut f = f0.clone();
                                    f.hide_structural = !f.hide_structural;
                                    on_change.call(f);
                                }
                            },
                        }
                        span { "Hide index / overview pages" }
                    }
                    label { class: "flex items-center gap-2",
                        input {
                            r#type: "checkbox",
                            checked: filters.hide_isolated,
                            onchange: {
                                let f0 = filters.clone();
                                move |_| {
                                    let mut f = f0.clone();
                                    f.hide_isolated = !f.hide_isolated;
                                    on_change.call(f);
                                }
                            },
                        }
                        span { "Hide isolated pages" }
                    }
                }

                // Max links
                div { class: "space-y-1.5",
                    div { class: "font-medium text-muted-foreground", "Max links" }
                    div { class: "flex items-center gap-2",
                        input {
                            r#type: "number",
                            min: "0",
                            class: "h-7 w-20 rounded border border-border bg-background px-2 text-xs",
                            value: "{max_links_value}",
                            placeholder: "Any",
                            oninput: {
                                let f0 = filters.clone();
                                move |evt: Event<FormData>| {
                                    let mut f = f0.clone();
                                    let raw = evt.value();
                                    f.max_links = raw.trim().parse::<u32>().ok();
                                    on_change.call(f);
                                }
                            },
                        }
                        span { class: "text-muted-foreground", "Hide hubs above this degree" }
                    }
                }

                // Link quality — the confidence / typed-link filters.
                div { class: "space-y-1.5",
                    div { class: "font-medium text-muted-foreground", "Link quality" }
                    label { class: "flex items-center gap-2",
                        input {
                            r#type: "checkbox",
                            checked: filters.typed_only,
                            onchange: {
                                let f0 = filters.clone();
                                move |_| {
                                    let mut f = f0.clone();
                                    f.typed_only = !f.typed_only;
                                    on_change.call(f);
                                }
                            },
                        }
                        span { "Only typed links (hide plain wikilinks)" }
                    }
                    label { class: "block space-y-1",
                        span { "Minimum confidence" }
                        select {
                            class: "h-7 w-full rounded border border-border bg-background px-2 text-xs",
                            value: filters.min_confidence.map(|c| c.to_string()).unwrap_or_default(),
                            onchange: {
                                let f0 = filters.clone();
                                move |evt: Event<FormData>| {
                                    let mut f = f0.clone();
                                    f.min_confidence = evt.value().trim().parse::<u8>().ok();
                                    on_change.call(f);
                                }
                            },
                            option { value: "", "Any" }
                            option { value: "2", "Possible and up" }
                            option { value: "3", "Likely and up" }
                            option { value: "4", "Certain only" }
                        }
                    }
                }

                // Display tuning
                div { class: "space-y-2",
                    div { class: "font-medium text-muted-foreground", "Display tuning" }
                    label { class: "block space-y-1",
                        div { class: "flex items-center justify-between",
                            span { "Node size" }
                            span { class: "text-muted-foreground", "{scale_pct}%" }
                        }
                        input {
                            r#type: "range",
                            min: "0.5",
                            max: "1.5",
                            step: "0.05",
                            value: "{props.node_scale}",
                            class: "w-full",
                            oninput: move |evt: Event<FormData>| {
                                if let Ok(v) = evt.value().parse::<f32>() {
                                    on_scale.call(v);
                                }
                            },
                        }
                    }
                    label { class: "block space-y-1",
                        div { class: "flex items-center justify-between",
                            span { "Spacing" }
                            span { class: "text-muted-foreground", "{spacing_pct}%" }
                        }
                        input {
                            r#type: "range",
                            min: "0.6",
                            max: "2.2",
                            step: "0.05",
                            value: "{props.spacing}",
                            class: "w-full",
                            oninput: move |evt: Event<FormData>| {
                                if let Ok(v) = evt.value().parse::<f32>() {
                                    on_spacing.call(v);
                                }
                            },
                        }
                    }
                    p { class: "text-[11px] leading-relaxed text-muted-foreground",
                        "Node size scales with link count; spacing spreads the layout apart."
                    }
                }

                // Node types
                div { class: "space-y-1.5",
                    div { class: "font-medium text-muted-foreground", "Node types" }
                    div { class: "grid grid-cols-2 gap-1",
                        for (kind, count) in counts.iter() {
                            {
                                let kind = kind.clone();
                                let count = *count;
                                let visible_kind = !filters.hidden_kinds.contains(&kind);
                                let label = kind_label(&kind);
                                rsx! {
                                    label {
                                        key: "{kind}",
                                        class: "flex min-w-0 items-center gap-1.5",
                                        input {
                                            r#type: "checkbox",
                                            checked: visible_kind,
                                            onchange: {
                                                let f0 = filters.clone();
                                                let kind = kind.clone();
                                                move |_| {
                                                    let mut f = f0.clone();
                                                    if f.hidden_kinds.contains(&kind) {
                                                        f.hidden_kinds.remove(&kind);
                                                    } else {
                                                        f.hidden_kinds.insert(kind.clone());
                                                    }
                                                    on_change.call(f);
                                                }
                                            },
                                        }
                                        span { class: "truncate", "{label}" }
                                        span { class: "text-muted-foreground/60", "{count}" }
                                    }
                                }
                            }
                        }
                    }
                }

                // Hidden nodes
                if !hidden_rows.is_empty() {
                    div { class: "space-y-1.5",
                        div { class: "font-medium text-muted-foreground", "Hidden nodes" }
                        div { class: "max-h-24 space-y-1 overflow-y-auto",
                            for (id, label) in hidden_rows.iter() {
                                {
                                    let id = id.clone();
                                    let label = label.clone();
                                    rsx! {
                                        div {
                                            key: "{id}",
                                            class: "flex items-center justify-between gap-2 rounded bg-muted/50 px-2 py-1",
                                            span { class: "truncate", "{label}" }
                                            button {
                                                r#type: "button",
                                                class: "text-muted-foreground hover:text-foreground",
                                                onclick: {
                                                    let f0 = filters.clone();
                                                    let id = id.clone();
                                                    move |_| {
                                                        let mut f = f0.clone();
                                                        f.hidden_node_ids.remove(&id);
                                                        on_change.call(f);
                                                    }
                                                },
                                                "Show"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                div { class: "rounded bg-muted/50 px-2 py-1.5 text-muted-foreground",
                    "Showing {shown_nodes}/{total_nodes} pages, {shown_edges}/{total_edges} links"
                }
            }
        }
    }
}
