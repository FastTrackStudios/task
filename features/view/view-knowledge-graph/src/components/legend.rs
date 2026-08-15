//! `GraphLegend` — the color key shown over the canvas.
//!
//! Two modes mirror the renderer's [`ColorMode`]: by **kind** (one row
//! per wiki page type, with counts; double-click a row to toggle that
//! type's visibility) and by **community** (one row per detected
//! cluster, flagging low-cohesion ones). Dumb: counts come from the
//! `nodes`/`communities` props, toggles go out via `on_toggle_kind` /
//! `on_show_all`. The host positions it (e.g. absolute bottom-left over
//! the graph).

use std::collections::{BTreeMap, HashSet};

use dioxus::prelude::*;

use crate::model::{
    ColorMode, CommunityInfo, GraphNode, community_bg_class, kind_bg_class, kind_label,
};

#[derive(Props, Clone, PartialEq)]
pub struct GraphLegendProps {
    pub nodes: Vec<GraphNode>,
    #[props(default)]
    pub communities: Vec<CommunityInfo>,
    #[props(default = ColorMode::Kind)]
    pub color_mode: ColorMode,
    /// Kinds currently hidden (rendered dimmed in kind mode).
    #[props(default)]
    pub hidden_kinds: HashSet<String>,
    /// Fired with a kind when its legend row is double-clicked.
    pub on_toggle_kind: EventHandler<String>,
    /// Fired when "Show all" is clicked (kind mode only).
    pub on_show_all: EventHandler<()>,
}

#[component]
pub fn GraphLegend(props: GraphLegendProps) -> Element {
    let mut collapsed = use_signal(|| false);
    let mut hovered: Signal<Option<String>> = use_signal(|| None);

    // Stable, sorted per-kind counts.
    let mut counts: BTreeMap<String, u32> = BTreeMap::new();
    for n in &props.nodes {
        *counts.entry(n.kind.clone()).or_insert(0) += 1;
    }

    let is_collapsed = *collapsed.read();
    let title = match props.color_mode {
        ColorMode::Kind => "Node types",
        ColorMode::Community => "Communities",
    };
    let show_all_visible = props.color_mode == ColorMode::Kind && !props.hidden_kinds.is_empty();

    rsx! {
        div { class: "rounded-lg border border-border bg-background/90 px-3 py-2 text-xs shadow-sm backdrop-blur-sm max-w-[260px]",
            div { class: "mb-1.5 flex items-center justify-between gap-2",
                span { class: "font-semibold text-foreground", "{title}" }
                div { class: "flex items-center gap-1",
                    if show_all_visible {
                        button {
                            r#type: "button",
                            class: "rounded px-1 py-0.5 text-[10px] text-muted-foreground hover:bg-accent hover:text-foreground",
                            title: "Show all types",
                            onclick: move |_| props.on_show_all.call(()),
                            "Show all"
                        }
                    }
                    button {
                        r#type: "button",
                        class: "rounded px-1 py-0.5 text-muted-foreground hover:bg-accent hover:text-foreground",
                        title: if is_collapsed { "Expand legend" } else { "Collapse legend" },
                        onclick: move |_| collapsed.toggle(),
                        if is_collapsed { "▶" } else { "▼" }
                    }
                }
            }

            if !is_collapsed {
                div { class: "flex max-h-48 flex-col gap-0.5 overflow-y-auto",
                    match props.color_mode {
                        ColorMode::Kind => rsx! {
                            for (kind, count) in counts.iter() {
                                {
                                    let kind = kind.clone();
                                    let count = *count;
                                    let hidden = props.hidden_kinds.contains(&kind);
                                    let swatch = if hidden { "bg-slate-400" } else { kind_bg_class(&kind) };
                                    let is_hovered = hovered.read().as_deref() == Some(kind.as_str());
                                    let label_class = if is_hovered {
                                        "font-medium text-foreground"
                                    } else {
                                        "text-muted-foreground"
                                    };
                                    let row_class = if hidden {
                                        "flex items-center gap-2 rounded px-1 py-0.5 transition-colors hover:bg-accent/50 opacity-40"
                                    } else {
                                        "flex items-center gap-2 rounded px-1 py-0.5 transition-colors hover:bg-accent/50"
                                    };
                                    let label = kind_label(&kind);
                                    let kind_enter = kind.clone();
                                    let kind_toggle = kind.clone();
                                    rsx! {
                                        div {
                                            key: "{kind}",
                                            class: "{row_class}",
                                            title: "Double-click to toggle visibility",
                                            onmouseenter: move |_| hovered.set(Some(kind_enter.clone())),
                                            onmouseleave: move |_| hovered.set(None),
                                            ondoubleclick: move |_| props.on_toggle_kind.call(kind_toggle.clone()),
                                            span { class: "inline-block h-3 w-3 shrink-0 rounded-full shadow-sm {swatch}" }
                                            span { class: "truncate {label_class}", "{label}" }
                                            span { class: "ml-auto text-muted-foreground/60", "{count}" }
                                            if hidden {
                                                span { class: "text-[10px] text-muted-foreground/60", "hidden" }
                                            }
                                        }
                                    }
                                }
                            }
                        },
                        ColorMode::Community => rsx! {
                            for c in props.communities.iter() {
                                {
                                    let swatch = community_bg_class(c.id);
                                    let label = c.top_nodes.first().cloned().unwrap_or_else(|| format!("Cluster {}", c.id));
                                    let tooltip = c.top_nodes.join(", ");
                                    let low_cohesion = c.cohesion < 0.15 && c.node_count >= 3;
                                    rsx! {
                                        div {
                                            key: "c{c.id}",
                                            class: "flex items-center gap-2 rounded px-1 py-0.5 transition-colors hover:bg-accent/50",
                                            span { class: "inline-block h-3 w-3 shrink-0 rounded-full shadow-sm {swatch}" }
                                            span { class: "truncate text-muted-foreground", title: "{tooltip}", "{label}" }
                                            span { class: "ml-auto shrink-0 text-muted-foreground/60", "{c.node_count}" }
                                            if low_cohesion {
                                                span {
                                                    class: "shrink-0 text-amber-500",
                                                    title: "Low cohesion: {c.cohesion:.2}",
                                                    "!"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        },
                    }
                }
            }
        }
    }
}
