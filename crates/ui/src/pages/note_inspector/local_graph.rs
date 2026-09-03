//! The **Local graph** tab: the focused note at the centre, what it
//! links to and what links to it around it, one or two hops out.
//!
//! Built client-side from the `VaultGraph` RPC — the same `links` /
//! `backlinks` calls the Links tab makes, fanned out to the
//! neighbours for a second hop — so it works over any vault id: the
//! org's own vault or a wiki's. The builder ([`build_local_graph`]) is
//! a pure function over an already-fetched [`Neighbourhood`], so it is
//! unit-tested here; [`fetch_neighbourhood`] is the transport.
//!
//! Rendering goes through the shared
//! [`KnowledgeGraphView`](view_knowledge_graph::KnowledgeGraphView), the
//! renderer the `/graph` route uses — and behind the same kind of
//! wasm-split boundary: the surface is mounted through
//! [`task_plugin_ui::lazy_element_with!`], so the layout engine and
//! the SVG renderer download the first time somebody opens the tab,
//! not with the shell.

use std::collections::{HashMap, HashSet};

use architect_ui::prelude::*;
use dioxus::prelude::*;
use vault_proto::{GraphLink, PageMeta};
use view_knowledge_graph::{GraphEdge, GraphNode, KnowledgeGraphView, WikiGraph};

use crate::pages::vault::{FileMeta, basename_of, fetch_backlinks, fetch_links};

/// The most nodes a local graph draws. Past this the second hop is
/// cut, and the panel says so — a hub's two-hop neighbourhood in a
/// large vault is the whole vault, which is what `/graph` is for.
pub(crate) const MAX_NODES: usize = 60;

/// Node-id prefix of a link that resolves to no page. The rest is the
/// link path as written; such a node is drawn dimmed and is not
/// navigable.
pub(crate) const UNRESOLVED_PREFIX: &str = "unresolved:";

/// What one page contributes: its outgoing wikilinks and the pages
/// linking to it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct PageLinks {
    pub(crate) links: Vec<GraphLink>,
    pub(crate) backlinks: Vec<String>,
}

/// The fetched neighbourhood of a focus page: the focus and every
/// neighbour whose links were pulled (only the focus for one hop).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Neighbourhood {
    pub(crate) focus: String,
    pub(crate) pages: HashMap<String, PageLinks>,
}

/// A built local graph plus what the panel says about it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct LocalGraph {
    pub(crate) graph: WikiGraph,
    /// Ids of the unresolved-link nodes (drawn dimmed).
    pub(crate) unresolved: Vec<String>,
    /// The node cap cut the neighbourhood short.
    pub(crate) truncated: bool,
}

/// Build the local graph: a breadth-first walk from `n.focus` over the
/// fetched pages, `hops` deep, stopping at `cap` nodes.
///
/// Backlink sources point AT a page, resolved outgoing links point
/// FROM it; an outgoing link that resolves to nothing becomes an
/// [`UNRESOLVED_PREFIX`] node that is never expanded. Duplicate
/// connections (a page that both links here and is linked from here)
/// collapse to one edge; self-links are dropped. Node ids are
/// vault-relative paths, so a click maps straight onto the panel's
/// open flow; labels come from `titles` (path → (title, sha)), with
/// the basename as the fallback.
pub(crate) fn build_local_graph(
    n: &Neighbourhood,
    hops: u8,
    cap: usize,
    titles: &HashMap<String, (String, String)>,
) -> LocalGraph {
    let cap = cap.max(1);
    let mut nodes: Vec<String> = vec![n.focus.clone()];
    let mut known: HashSet<String> = HashSet::from([n.focus.clone()]);
    let mut seen_edges: HashSet<(String, String)> = HashSet::new();
    let mut edges: Vec<(String, String)> = Vec::new();
    let mut unresolved: Vec<String> = Vec::new();
    let mut truncated = false;

    // Admit a node, or refuse it at the cap. Returns whether it is in
    // the graph (already known or just added).
    let mut admit = |id: &str, nodes: &mut Vec<String>, truncated: &mut bool| -> bool {
        if known.contains(id) {
            return true;
        }
        if nodes.len() >= cap {
            *truncated = true;
            return false;
        }
        known.insert(id.to_owned());
        nodes.push(id.to_owned());
        true
    };
    let mut connect = |a: &str, b: &str| {
        if a == b {
            return;
        }
        let key = if a < b {
            (a.to_owned(), b.to_owned())
        } else {
            (b.to_owned(), a.to_owned())
        };
        if seen_edges.insert(key) {
            edges.push((a.to_owned(), b.to_owned()));
        }
    };

    let mut frontier: Vec<String> = vec![n.focus.clone()];
    let mut expanded: HashSet<String> = HashSet::new();
    for _ in 0..hops {
        let mut next: Vec<String> = Vec::new();
        for page in frontier {
            if !expanded.insert(page.clone()) {
                continue;
            }
            let Some(links) = n.pages.get(&page) else {
                continue;
            };
            for link in &links.links {
                match &link.resolved {
                    Some(target) => {
                        if admit(target, &mut nodes, &mut truncated) {
                            connect(&page, target);
                            next.push(target.clone());
                        }
                    }
                    None => {
                        let id = format!("{UNRESOLVED_PREFIX}{}", link.linkpath);
                        if admit(&id, &mut nodes, &mut truncated) {
                            if !unresolved.contains(&id) {
                                unresolved.push(id.clone());
                            }
                            connect(&page, &id);
                        }
                    }
                }
            }
            for source in &links.backlinks {
                if admit(source, &mut nodes, &mut truncated) {
                    connect(source, &page);
                    next.push(source.clone());
                }
            }
        }
        frontier = next;
    }

    // In-graph degree sizes the nodes (the focus reads as the hub).
    let mut degree: HashMap<&str, u32> = HashMap::new();
    for (s, t) in &edges {
        *degree.entry(s.as_str()).or_default() += 1;
        *degree.entry(t.as_str()).or_default() += 1;
    }
    // Stable order → stable layout between renders.
    nodes.sort_unstable();
    let graph_nodes = nodes
        .iter()
        .map(|p| {
            let (label, kind) = match p.strip_prefix(UNRESOLVED_PREFIX) {
                Some(linkpath) => (linkpath.to_owned(), "unresolved".to_owned()),
                None => (
                    titles
                        .get(p)
                        .map(|(title, _)| title.clone())
                        .unwrap_or_else(|| basename_of(p).to_owned()),
                    "other".to_owned(),
                ),
            };
            GraphNode {
                id: p.clone(),
                label,
                kind,
                path: p.clone(),
                link_count: degree.get(p.as_str()).copied().unwrap_or(0),
                community: 0,
            }
        })
        .collect();
    let graph_edges = edges
        .into_iter()
        .map(|(s, t)| GraphEdge::wikilink(s, t, 1.0))
        .collect();
    LocalGraph {
        graph: WikiGraph {
            nodes: graph_nodes,
            edges: graph_edges,
            communities: Vec::new(),
        },
        unresolved,
        truncated,
    }
}

/// Pull one page's links + backlinks over the vault graph RPC.
async fn fetch_page_links(slug: &str, vault_id: &str, path: &str) -> Result<PageLinks, String> {
    let (links, backlinks) = futures_util::future::join(
        fetch_links(slug.to_owned(), vault_id.to_owned(), path.to_owned()),
        fetch_backlinks(slug.to_owned(), vault_id.to_owned(), path.to_owned()),
    )
    .await;
    Ok(PageLinks {
        links: links?,
        backlinks: backlinks?,
    })
}

/// Fetch what [`build_local_graph`] needs for `hops` around `focus`:
/// the focus page's links, and — for a second hop — its neighbours'
/// (resolved targets + backlink sources, at most `cap`, fetched
/// concurrently). A neighbour whose fetch fails is left out rather
/// than failing the graph; the focus page's own failure is the error.
pub(crate) async fn fetch_neighbourhood(
    slug: String,
    vault_id: String,
    focus: String,
    hops: u8,
    cap: usize,
) -> Result<Neighbourhood, String> {
    let first = fetch_page_links(&slug, &vault_id, &focus).await?;
    let mut pages = HashMap::new();
    if hops >= 2 {
        let mut neighbours: Vec<String> = first
            .links
            .iter()
            .filter_map(|l| l.resolved.clone())
            .chain(first.backlinks.iter().cloned())
            .filter(|p| *p != focus)
            .collect();
        neighbours.sort_unstable();
        neighbours.dedup();
        neighbours.truncate(cap);
        let fetched = futures_util::future::join_all(
            neighbours
                .iter()
                .map(|p| fetch_page_links(&slug, &vault_id, p)),
        )
        .await;
        for (path, result) in neighbours.into_iter().zip(fetched) {
            if let Ok(links) = result {
                pages.insert(path, links);
            }
        }
    }
    pages.insert(focus.clone(), first);
    Ok(Neighbourhood { focus, pages })
}

/// What the local-graph surface is mounted with. A plain props struct
/// because the surface sits behind `lazy_element_with!`, whose
/// argument crosses the chunk boundary by value.
#[derive(Clone, PartialEq)]
pub(crate) struct LocalGraphArgs {
    pub(crate) org: String,
    pub(crate) vault_id: String,
    pub(crate) path: String,
    /// Bumped by the page after a save or a live change; re-fetches.
    pub(crate) refresh: u64,
    /// The folder index, for node titles.
    pub(crate) pages: ReadSignal<Vec<PageMeta>>,
    pub(crate) on_open: Callback<FileMeta>,
}

/// The chunk's entry point (see the module docs): what
/// `lazy_element_with!("local_graph", …)` calls once the code is here.
pub(crate) fn local_graph_surface(args: LocalGraphArgs) -> Element {
    rsx! { LocalGraphPanel { args } }
}

/// The Local graph tab body: hop control, the graph, and what was
/// left out. Fetches on mount and whenever its args or the hop count
/// change — mounting it only while its tab shows is what keeps the
/// fetch lazy.
#[component]
fn LocalGraphPanel(args: LocalGraphArgs) -> Element {
    let mut hops = use_signal(|| 1u8);
    let LocalGraphArgs {
        org,
        vault_id,
        path,
        refresh,
        pages,
        on_open,
    } = args;
    let fetched = use_resource(use_reactive!(|(org, vault_id, path, refresh)| {
        let h = hops();
        async move {
            let _ = refresh;
            fetch_neighbourhood(org, vault_id, path, h, MAX_NODES).await
        }
    }));
    let titles = use_memo(move || {
        pages
            .read()
            .iter()
            .map(|p| (p.path.clone(), (p.title.clone(), p.sha256.clone())))
            .collect::<HashMap<String, (String, String)>>()
    });
    let built = use_memo(move || match &*fetched.read() {
        Some(Ok(n)) => Some(Ok(build_local_graph(n, hops(), MAX_NODES, &titles.read()))),
        Some(Err(e)) => Some(Err(e.clone())),
        None => None,
    });

    let focus = fetched
        .read()
        .as_ref()
        .and_then(|r| r.as_ref().ok())
        .map(|n| n.focus.clone())
        .unwrap_or_default();

    rsx! {
        div { class: "flex min-h-0 flex-1 flex-col",
            div { class: "flex items-center gap-2 border-b border-border/60 px-3 py-2",
                Heading { level: HeadingLevel::H3, "Local graph" }
                div { class: "ml-auto flex items-center gap-0.5 rounded-md border border-border/60 p-0.5",
                    for (n, label) in [(1u8, "1 hop"), (2u8, "2 hops")] {
                        button {
                            key: "{n}",
                            r#type: "button",
                            "data-testid": "local-graph-hops-{n}",
                            class: if hops() == n {
                                "rounded px-2 py-0.5 text-xs font-medium bg-accent text-foreground"
                            } else {
                                "rounded px-2 py-0.5 text-xs text-muted-foreground hover:text-foreground"
                            },
                            onclick: move |_| hops.set(n),
                            "{label}"
                        }
                    }
                }
            }
            match &*built.read() {
                Some(Ok(local)) => {
                    let graph = local.graph.clone();
                    let dimmed = local.unresolved.clone();
                    let n_nodes = graph.nodes.len();
                    let n_unresolved = local.unresolved.len();
                    let truncated = local.truncated;
                    let cur = focus.clone();
                    rsx! {
                        div {
                            class: "m-2 h-72 shrink-0 overflow-hidden rounded-lg border border-border/70",
                            "data-testid": "local-graph",
                            "data-nodes": "{n_nodes}",
                            KnowledgeGraphView {
                                graph,
                                node_scale: 0.35,
                                spacing: 1.5,
                                active: Some(focus.clone()),
                                dimmed,
                                on_node_click: move |id: String| {
                                    // The focus is already open; an
                                    // unresolved link has no page to open.
                                    if id == cur || id.starts_with(UNRESOLVED_PREFIX) {
                                        return;
                                    }
                                    let (_, sha) = titles.peek().get(&id).cloned().unwrap_or_default();
                                    on_open.call(FileMeta { path: id, sha256: sha });
                                },
                            }
                        }
                        div { class: "flex flex-wrap items-center gap-x-3 gap-y-1 px-3 pb-3 text-xs text-muted-foreground",
                            span { "{n_nodes} pages" }
                            if n_unresolved > 0 {
                                span { title: "Links to pages nobody has written yet, drawn dimmed",
                                    "{n_unresolved} unresolved"
                                }
                            }
                            if truncated {
                                span { title: "The neighbourhood is bigger than this panel draws; open the full graph for all of it",
                                    "cut at {MAX_NODES}"
                                }
                            }
                            if n_nodes <= 1 {
                                span { "Nothing links here yet." }
                            }
                        }
                    }
                }
                Some(Err(e)) => rsx! {
                    div { class: "px-2 py-2",
                        crate::states::InlineError {
                            message: e.clone(),
                            label: "Local graph".to_string(),
                        }
                    }
                },
                None => rsx! {
                    div { class: "m-2",
                        Skeleton { class: "h-72 w-full rounded-lg" }
                    }
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(linkpath: &str, resolved: Option<&str>) -> GraphLink {
        GraphLink {
            linkpath: linkpath.into(),
            resolved: resolved.map(str::to_owned),
            alias: None,
        }
    }

    fn page(links: Vec<GraphLink>, backlinks: &[&str]) -> PageLinks {
        PageLinks {
            links,
            backlinks: backlinks.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    /// Modes links to Scales and Keys (Keys links back), Ionian links
    /// to Modes; Scales links on to Intervals (a second-hop page) and
    /// to a page nobody wrote.
    fn neighbourhood() -> Neighbourhood {
        let mut pages = HashMap::new();
        pages.insert(
            "Concepts/Modes.md".to_owned(),
            page(
                vec![
                    link("Scales", Some("Concepts/Scales.md")),
                    link("Keys", Some("Concepts/Keys.md")),
                    link("Modes", Some("Concepts/Modes.md")),
                ],
                &["Concepts/Ionian.md", "Concepts/Keys.md"],
            ),
        );
        pages.insert(
            "Concepts/Scales.md".to_owned(),
            page(
                vec![
                    link("Intervals", Some("Concepts/Intervals.md")),
                    link("Tetrachords", None),
                    link("Modes", Some("Concepts/Modes.md")),
                ],
                &["Concepts/Modes.md"],
            ),
        );
        pages.insert(
            "Concepts/Keys.md".to_owned(),
            page(
                vec![link("Modes", Some("Concepts/Modes.md"))],
                &["Concepts/Modes.md"],
            ),
        );
        pages.insert(
            "Concepts/Ionian.md".to_owned(),
            page(vec![link("Modes", Some("Concepts/Modes.md"))], &[]),
        );
        Neighbourhood {
            focus: "Concepts/Modes.md".to_owned(),
            pages,
        }
    }

    fn ids(g: &LocalGraph) -> Vec<&str> {
        g.graph.nodes.iter().map(|n| n.id.as_str()).collect()
    }

    #[test]
    fn one_hop_is_the_focus_and_its_direct_neighbours_deduped() {
        let g = build_local_graph(&neighbourhood(), 1, MAX_NODES, &HashMap::new());
        assert_eq!(
            ids(&g),
            vec![
                "Concepts/Ionian.md",
                "Concepts/Keys.md",
                "Concepts/Modes.md",
                "Concepts/Scales.md"
            ]
        );
        // Keys both links here and is linked from here: ONE edge; the
        // self-link is dropped. Modes→Scales, Modes→Keys, Ionian→Modes.
        assert_eq!(g.graph.edges.len(), 3);
        assert!(!g.truncated);
        assert!(g.unresolved.is_empty(), "no second hop, no unresolved yet");
        let focus = g
            .graph
            .nodes
            .iter()
            .find(|n| n.id == "Concepts/Modes.md")
            .unwrap();
        assert_eq!(focus.link_count, 3, "the focus is the hub");
    }

    #[test]
    fn two_hops_reach_the_neighbours_links_and_mark_unresolved_ones() {
        let g = build_local_graph(&neighbourhood(), 2, MAX_NODES, &HashMap::new());
        let ids = ids(&g);
        assert!(ids.contains(&"Concepts/Intervals.md"), "{ids:?}");
        assert!(ids.contains(&"unresolved:Tetrachords"), "{ids:?}");
        assert_eq!(g.unresolved, vec!["unresolved:Tetrachords".to_owned()]);
        let ghost = g
            .graph
            .nodes
            .iter()
            .find(|n| n.id == "unresolved:Tetrachords")
            .unwrap();
        assert_eq!(ghost.kind, "unresolved");
        assert_eq!(ghost.label, "Tetrachords");
        // Scales→Modes is the same connection as Modes→Scales: still one.
        let scales_modes = g
            .graph
            .edges
            .iter()
            .filter(|e| {
                matches!(
                    (e.source.as_str(), e.target.as_str()),
                    ("Concepts/Modes.md", "Concepts/Scales.md")
                        | ("Concepts/Scales.md", "Concepts/Modes.md")
                )
            })
            .count();
        assert_eq!(scales_modes, 1);
        assert!(!g.truncated);
    }

    #[test]
    fn the_node_cap_cuts_the_walk_and_says_so() {
        let g = build_local_graph(&neighbourhood(), 2, 3, &HashMap::new());
        assert_eq!(g.graph.nodes.len(), 3);
        assert!(g.truncated);
        assert!(
            ids(&g).contains(&"Concepts/Modes.md"),
            "the focus always stays"
        );
        // Every edge is between admitted nodes.
        let known: HashSet<&str> = ids(&g).into_iter().collect();
        for e in &g.graph.edges {
            assert!(known.contains(e.source.as_str()) && known.contains(e.target.as_str()));
        }
        // A cap below one still keeps the focus.
        let g = build_local_graph(&neighbourhood(), 1, 0, &HashMap::new());
        assert_eq!(ids(&g), vec!["Concepts/Modes.md"]);
    }

    #[test]
    fn labels_come_from_the_index_then_the_basename() {
        let mut titles = HashMap::new();
        titles.insert(
            "Concepts/Scales.md".to_owned(),
            ("Scales & scale degrees".to_owned(), "sha".to_owned()),
        );
        let g = build_local_graph(&neighbourhood(), 1, MAX_NODES, &titles);
        let label = |id: &str| {
            g.graph
                .nodes
                .iter()
                .find(|n| n.id == id)
                .map(|n| n.label.clone())
                .unwrap()
        };
        assert_eq!(label("Concepts/Scales.md"), "Scales & scale degrees");
        assert_eq!(label("Concepts/Keys.md"), "Keys");
    }

    #[test]
    fn a_focus_nobody_fetched_is_a_lone_node() {
        let n = Neighbourhood {
            focus: "Lonely.md".to_owned(),
            pages: HashMap::new(),
        };
        let g = build_local_graph(&n, 2, MAX_NODES, &HashMap::new());
        assert_eq!(ids(&g), vec!["Lonely.md"]);
        assert!(g.graph.edges.is_empty());
    }
}
