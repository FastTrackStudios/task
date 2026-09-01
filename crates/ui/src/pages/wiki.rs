//! `/wiki` — knowledge-graph view of the org's knowledge.
//!
//! Two data sources, toggled in the header:
//!
//! - **Wiki** (default): the curated wiki tree
//!   (`<org>/wiki/Knowledge/`) through the `wiki_proto` Graph
//!   service — the server-built 4-signal relevance graph +
//!   Louvain clusters ([`crate::feeds::fetch_wiki_service_graph`]).
//!   Clicking a node opens the page reader/editor
//!   (`/wiki/page?:path`).
//! - **Vault**: the raw vault's `[[wikilink]]` web, fetched via
//!   `VaultSyncClient` and built client-side with
//!   [`view_knowledge_graph::build_wiki_graph`]. Clicking a node
//!   deep-links into `/vault`. Kept because the vault is a
//!   different corpus (notes, not curated knowledge) — the wiki
//!   view doesn't replace it.
//!
//! Both render through the dumb [`KnowledgeGraphView`]; the
//! [`GraphLegend`] overlay keys the node colors and
//! double-clicking a legend row toggles that kind's visibility
//! (page-owned [`GraphFilterState`]). The graph is scoped to one
//! org (the selected org, or the home org when viewing All), so
//! the org switcher swaps wikis.

use std::collections::HashMap;

use architect_ui::prelude::*;
use dioxus::prelude::*;
use view_knowledge_graph::{
    GraphFilterState, GraphLegend, KnowledgeGraphView, WikiGraph, apply_filters, build_wiki_graph,
};

use crate::orgs::{OrgMeta, OrgSelection, selected_slugs};

/// The single wiki id the server hosts per org (mirrors
/// `wiki_page::WIKI_ID`).
const WIKI_ID: &str = "default";

/// Which corpus the graph shows.
///
/// `Subscriptions` is not a corpus — it swaps the body for the
/// management panel. It rides the same tab strip because subscribing
/// is what *changes* the graph: a subscribed source's pages become
/// resolvable in your own writing, so the two belong on one page
/// rather than behind a settings screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GraphSource {
    /// Curated wiki tree via the wiki Graph service.
    Wiki,
    /// Raw vault `[[wikilink]]` web via VaultSync.
    Vault,
    /// What this org subscribes to, and the controls for it.
    Subscriptions,
}

#[component]
pub fn WikiView() -> Element {
    let nav = use_navigator();
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();

    let mut source = use_signal(|| GraphSource::Wiki);

    // Page-owned visibility filters, driven from the legend. Start
    // from everything-visible (not the crate default, which hides
    // structural pages) so the page keeps showing the whole corpus.
    let mut filters = use_signal(|| GraphFilterState {
        hide_structural: false,
        ..GraphFilterState::default()
    });

    // Fetch + build once per (org, source) change; the live stream
    // below restarts it when the corpus changes under us.
    let graph = use_resource(move || async move {
        let slugs = selected_slugs(&selection.read(), &org_list.read());
        let slug = slugs
            .first()
            .cloned()
            .ok_or_else(|| "no organization selected".to_string())?;
        match source() {
            GraphSource::Wiki => crate::feeds::fetch_wiki_service_graph(&slug).await,
            GraphSource::Vault => {
                let files = crate::feeds::fetch_wiki_files(&slug).await?;
                Ok::<WikiGraph, String>(build_wiki_graph(&files))
            }
        }
    });

    // ── Live wiki changes ─────────────────────────────────────
    // The `Events` `#[subscribe]` stream: the LLM-wiki pipeline
    // writes pages in the background (ingest, review actions), so
    // the graph used to go stale until you navigated away and back.
    //
    // The relevance graph is server-built from parsed page content,
    // which a `PageWritten` can't carry — so a page event re-runs
    // the fetch rather than folding. The event is the trigger, the
    // rpc stays the source of truth. Re-subscribing (org switch,
    // reconnect) also re-fetches: events published while we were
    // detached are gone from the sliding mailbox.
    let subscribed_once = use_signal(|| false);
    architect::use_stream(
        move |tx| {
            // Signals are `Copy`; the hook takes `Fn`, so take fresh
            // mutable handles per call.
            let (mut graph, mut subscribed_once) = (graph, subscribed_once);
            let slug = selected_slugs(&selection.read(), &org_list.read())
                .first()
                .cloned();
            if *subscribed_once.peek() {
                graph.restart();
            }
            subscribed_once.set(true);
            async move {
                let Some(slug) = slug else {
                    return false;
                };
                let Ok(client) = crate::vox_clients::establish_for::<
                    wiki_proto::service::events::EventsStreamClient,
                >(&slug)
                .await
                else {
                    return false;
                };
                client.changes(tx).await.is_ok()
            }
        },
        move |change: wiki_proto::WikiChange| {
            let mut graph = graph;
            // The stream is unfiltered — one backend can serve
            // several wikis. Keep the one this page shows.
            if change.wiki_id != WIKI_ID {
                return;
            }
            // Only corpus changes move the graph; queue traffic
            // (ingest / review) doesn't until it produces a page.
            if matches!(
                change.event,
                wiki_proto::WikiEvent::PageWritten { .. }
                    | wiki_proto::WikiEvent::PageDeleted { .. }
                    | wiki_proto::WikiEvent::PeerPulled { .. }
                    | wiki_proto::WikiEvent::Resync
            ) {
                graph.restart();
            }
        },
    );

    let discovering = org_list.read().is_empty();
    let body = if source() == GraphSource::Subscriptions {
        rsx! { crate::pages::wiki_subscriptions::SubscriptionsPanel {} }
    } else if discovering {
        render_loading()
    } else {
        match &*graph.read() {
            Some(Ok(g)) if g.nodes.is_empty() => render_empty(source()),
            Some(Ok(g)) => {
                // Apply the legend's kind toggles; communities stay
                // from the full build so the legend keeps its counts.
                let filtered = apply_filters(&g.nodes, &g.edges, &filters.read());
                let shown = WikiGraph {
                    nodes: filtered.nodes,
                    edges: filtered.edges,
                    communities: g.communities.clone(),
                };
                // node id → path, for click navigation.
                let path_of: HashMap<String, String> = shown
                    .nodes
                    .iter()
                    .map(|n| (n.id.clone(), n.path.clone()))
                    .collect();
                let legend_nodes = g.nodes.clone();
                let legend_comms = g.communities.clone();
                let src = source();
                rsx! {
                    div { class: "relative min-h-0 flex-1 overflow-hidden rounded-xl border border-border/70 bg-card/30",
                        KnowledgeGraphView {
                            graph: shown,
                            spacing: 1.6,
                            on_node_click: move |id: String| {
                                if let Some(path) = path_of.get(&id) {
                                    let route = match src {
                                        GraphSource::Wiki => crate::routes::Route::WikiPageRoute {
                                            path: path.clone(),
                                        },
                                        GraphSource::Vault => crate::routes::Route::VaultRoute {
                                            path: path.clone(),
                                            org: String::new(),
                                        },
                                    };
                                    nav.push(route);
                                }
                            },
                        }
                        div { class: "absolute bottom-3 left-3",
                            GraphLegend {
                                nodes: legend_nodes,
                                communities: legend_comms,
                                hidden_kinds: filters.read().hidden_kinds.clone(),
                                on_toggle_kind: move |kind: String| {
                                    filters.with_mut(|f| {
                                        if !f.hidden_kinds.remove(&kind) {
                                            f.hidden_kinds.insert(kind);
                                        }
                                    });
                                },
                                on_show_all: move |()| filters.with_mut(|f| f.hidden_kinds.clear()),
                            }
                        }
                    }
                }
            }
            Some(Err(e)) => rsx! {
                div { class: "rounded-xl border border-destructive/40 bg-destructive/10 px-4 py-3 text-sm",
                    "Couldn't build the knowledge graph: {e}"
                }
            },
            None => render_loading(),
        }
    };

    let subtitle = match source() {
        GraphSource::Wiki => "The curated wiki — pages are nodes, relevance signals are edges.",
        GraphSource::Vault => {
            "The wikilink web of your vault — pages are nodes, `[[links]]` are edges."
        }
        GraphSource::Subscriptions => {
            "Sources this org holds. A subscribed wiki's pages resolve inside your own writing."
        }
    };

    rsx! {
        div { class: "mx-auto flex h-full w-full max-w-6xl flex-col gap-4 p-4 sm:p-6 lg:p-8",
            header { class: "flex flex-col gap-1",
                span { class: "text-[0.7rem] font-semibold uppercase tracking-[0.18em] text-muted-foreground",
                    "Workspace"
                }
                div { class: "flex items-baseline justify-between gap-3",
                    div { class: "flex items-baseline gap-3",
                        Heading { level: HeadingLevel::H1, class: "tracking-tight", "Knowledge graph" }
                        div { class: "flex items-center gap-1 rounded-lg border border-border/70 bg-card/40 p-0.5 text-xs",
                            {source_tab("Wiki", GraphSource::Wiki, source(), move |s| {
                                source.set(s);
                                filters.with_mut(|f| f.hidden_kinds.clear());
                            })}
                            {source_tab("Vault", GraphSource::Vault, source(), move |s| {
                                source.set(s);
                                filters.with_mut(|f| f.hidden_kinds.clear());
                            })}
                            {source_tab("Subscriptions", GraphSource::Subscriptions, source(), move |s| {
                                source.set(s);
                            })}
                        }
                    }
                    Link {
                        to: crate::routes::Route::WikiSourcesRoute {},
                        class: "shrink-0 text-xs text-muted-foreground underline decoration-border underline-offset-2 hover:text-foreground",
                        "Archived sources →"
                    }
                }
                Text { variant: TextVariant::Muted, "{subtitle}" }
            }
            {body}
        }
    }
}

fn source_tab(
    label: &'static str,
    value: GraphSource,
    current: GraphSource,
    mut on_pick: impl FnMut(GraphSource) + 'static,
) -> Element {
    let active = value == current;
    let class = if active {
        "rounded-md bg-accent px-2 py-0.5 font-medium text-foreground"
    } else {
        "rounded-md px-2 py-0.5 text-muted-foreground hover:text-foreground"
    };
    rsx! {
        button { class, onclick: move |_| on_pick(value), "{label}" }
    }
}

fn render_loading() -> Element {
    rsx! {
        div { class: "flex min-h-0 flex-1 items-center justify-center rounded-xl border border-border/70 bg-card/30",
            Text { variant: TextVariant::Muted, "Building the graph…" }
        }
    }
}

fn render_empty(source: GraphSource) -> Element {
    let (title, hint) = match source {
        // The panel owns its own empty state; this arm exists so the
        // match stays exhaustive rather than being papered over with a
        // wildcard that would swallow a future variant.
        GraphSource::Subscriptions => ("", ""),
        GraphSource::Wiki => (
            "The wiki is empty",
            "Bootstrap it with `task wiki init`, then ingest a source — pages land in `wiki/Knowledge/` and show up here.",
        ),
        GraphSource::Vault => (
            "Nothing to graph yet",
            "Add some `[[wikilinks]]` between vault notes and they'll appear here.",
        ),
    };
    rsx! {
        div { class: "flex min-h-0 flex-1 flex-col items-center justify-center gap-2 rounded-2xl border border-dashed border-border/70 bg-card/40 py-16 text-center",
            Heading { level: HeadingLevel::H3, "{title}" }
            Text { variant: TextVariant::Muted, "{hint}" }
        }
    }
}
