//! The sidebar while you are inside one wiki: that wiki's page tree.
//!
//! The vault explorer is the right companion to vault pages; inside a
//! wiki it is the wrong map. Same idea as the project sidebar on a
//! project page — the shell swaps the left column by route
//! (`app_shell`), and this is the wiki's column: a way back to the list
//! of wikis, the wiki's name, and its pages as the directories they live
//! in. Selection follows the route, so a page opened from the graph, a
//! link, or this tree all read as the same "open".

use std::collections::HashSet;

use architect_ui::lucide_dioxus::{BookOpen, ChevronRight, FileText, Sparkles};
use architect_ui::prelude::*;
use dioxus::prelude::*;

use crate::pages::wiki_home::{DirNode, build_tree};
use crate::routes::Route;

#[component]
pub fn WikiSidebar(org: String, wiki: String, selected: String) -> Element {
    let account = use_context::<Signal<Option<crate::auth::ActiveAccount>>>();
    // The org comes from the route, not the switcher: under "All" the
    // list spans every org, and a wiki opened from it must read from
    // the org that holds it.
    let org_sig = use_signal(|| org.clone());
    let active = use_memo(move || org_sig());

    let wiki_id = wiki.clone();
    let org_for_pages = org.clone();
    let mut pages = use_resource(use_reactive!(|(wiki_id, org_for_pages)| async move {
        let _session = account.read().as_ref().map(|a| a.user_id);
        crate::feeds::fetch_wiki_pages_of(&org_for_pages, &wiki_id).await
    }));
    let title_wiki = wiki.clone();
    let title_org = org.clone();
    let title = use_resource(use_reactive!(|(title_wiki, title_org)| async move {
        let slug = title_org;
        crate::feeds::fetch_wikis(&slug)
            .await
            .ok()
            .and_then(|list| list.into_iter().find(|w| w.slug == title_wiki))
            .map(|w| if w.title.is_empty() { w.slug } else { w.title })
    }));

    // Live: a page written from this client (Add page, a save) or by
    // the wiki pipeline shows up here without a reload — the tree is
    // the map, and a map that lags the territory teaches people to
    // distrust it.
    let live_wiki = wiki.clone();
    architect::use_stream(
        move |tx| {
            let slug = active();
            async move {
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
            let mut pages = pages;
            if change.wiki_id != live_wiki {
                return;
            }
            if matches!(
                change.event,
                wiki_proto::WikiEvent::PageWritten { .. }
                    | wiki_proto::WikiEvent::PageDeleted { .. }
                    | wiki_proto::WikiEvent::Resync
            ) {
                pages.restart();
            }
        },
    );

    let expanded = use_signal(HashSet::<String>::new);
    let heading = title
        .read()
        .clone()
        .flatten()
        .unwrap_or_else(|| wiki.clone());

    rsx! {
        div { class: "flex h-full min-h-0 flex-col",
            div { class: "flex flex-col gap-1 px-3 pb-1 pt-3",
                Link {
                    to: Route::WikiRoute {},
                    class: "text-[0.7rem] text-muted-foreground hover:text-foreground",
                    "← Wikis"
                }
                Link {
                    to: Route::WikiHomeRoute { org: org.clone(), wiki: wiki.clone() },
                    class: "flex items-center gap-1.5 text-[0.7rem] font-semibold uppercase tracking-[0.18em] text-muted-foreground hover:text-foreground",
                    span { class: "flex h-3.5 w-3.5 items-center justify-center", BookOpen { size: 13 } }
                    span { class: "truncate", "{heading}" }
                }
            }
            div { class: "min-h-0 flex-1 overflow-y-auto pb-2",
                match &*pages.read_unchecked() {
                    Some(Ok(list)) if list.is_empty() => rsx! {
                        div { class: "px-3 py-2 text-xs text-muted-foreground", "No pages yet." }
                    },
                    Some(Ok(list)) => {
                        let tree = build_tree(list);
                        rsx! {
                            nav { class: "flex flex-col gap-px px-1.5",
                                {dir_children(&tree, String::new(), 0, expanded, &org, &wiki, &selected)}
                            }
                        }
                    },
                    Some(Err(e)) => rsx! {
                        div { class: "px-1.5 py-1",
                            crate::states::InlineError {
                                message: e.clone(),
                                label: "Wiki".to_string(),
                                on_retry: move |()| pages.restart(),
                            }
                        }
                    },
                    None => rsx! {
                        div { class: "flex items-center gap-2 px-3 py-2 text-xs text-muted-foreground",
                            Spinner { size: SpinnerSize::Small }
                            "Loading pages…"
                        }
                    },
                }
            }
        }
    }
}

fn dir_children(
    node: &DirNode,
    prefix: String,
    depth: usize,
    expanded: Signal<HashSet<String>>,
    org: &str,
    wiki: &str,
    selected: &str,
) -> Element {
    rsx! {
        for page in &node.pages {
            {page_row(page, depth, org, wiki, selected)}
        }
        for (seg, child) in &node.dirs {
            {dir_node(seg, child, prefix.clone(), depth, expanded, org, wiki, selected)}
        }
    }
}

/// One directory row + its children. Directories open by default when
/// the selected page is inside them, so a deep link lands expanded.
fn dir_node(
    name: &str,
    node: &DirNode,
    prefix: String,
    depth: usize,
    mut expanded: Signal<HashSet<String>>,
    org: &str,
    wiki: &str,
    selected: &str,
) -> Element {
    let key = if prefix.is_empty() {
        name.to_owned()
    } else {
        format!("{prefix}/{name}")
    };
    let holds_selected = !selected.is_empty() && selected.starts_with(&format!("{key}/"));
    let open = holds_selected || expanded.read().contains(&key);
    let chevron = if open { "rotate-90" } else { "" };
    let toggle_key = key.clone();
    let indent = depth * 12;
    rsx! {
        div { key: "{key}",
            button {
                r#type: "button",
                class: "flex w-full items-center gap-1.5 rounded-md px-1.5 py-1 text-left text-[13px] text-muted-foreground hover:bg-accent/40 hover:text-foreground",
                style: "padding-left: {indent + 6}px",
                onclick: move |_| {
                    expanded.with_mut(|e| {
                        if !e.remove(&toggle_key) {
                            e.insert(toggle_key.clone());
                        }
                    });
                },
                span { class: "flex h-3 w-3 shrink-0 items-center justify-center transition-transform {chevron}",
                    ChevronRight { size: 11 }
                }
                span { class: "truncate font-medium", "{name}" }
            }
            if open {
                {dir_children(node, key.clone(), depth + 1, expanded, org, wiki, selected)}
            }
        }
    }
}

fn page_row(
    page: &wiki_proto::pages::PageInfo,
    depth: usize,
    org: &str,
    wiki: &str,
    selected: &str,
) -> Element {
    let nav = use_navigator();
    let is_selected = !selected.is_empty() && page.path == selected;
    let indent = depth * 12;
    let row_cls = if is_selected {
        "flex w-full items-center gap-1.5 rounded-md bg-accent px-1.5 py-1 text-left text-[13px] text-foreground"
    } else {
        "flex w-full items-center gap-1.5 rounded-md px-1.5 py-1 text-left text-[13px] text-muted-foreground hover:bg-accent/40 hover:text-foreground"
    };
    let path = page.path.clone();
    let wiki = wiki.to_owned();
    let org = org.to_owned();
    let title = if page.title.is_empty() {
        page.path.clone()
    } else {
        page.title.clone()
    };
    let ai = page.ai_generated;
    rsx! {
        button {
            key: "{page.path}",
            r#type: "button",
            class: "{row_cls}",
            style: "padding-left: {indent + 6}px",
            onclick: move |_| {
                nav.push(Route::WikiDocRoute { org: org.clone(), wiki: wiki.clone(), path: path.clone() });
            },
            if ai {
                span { class: "flex h-3.5 w-3.5 shrink-0 items-center justify-center text-primary",
                    Sparkles { size: 12 }
                }
            } else {
                span { class: "flex h-3.5 w-3.5 shrink-0 items-center justify-center",
                    FileText { size: 12 }
                }
            }
            span { class: "truncate", "{title}" }
        }
    }
}
