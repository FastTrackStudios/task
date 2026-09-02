//! `/wiki/w/:wiki` — one wiki's home: what it is for, who edits it, and
//! its pages.
//!
//! The pages are listed as the tree they are on disk, the same shape the
//! sidebar shows while you are inside this wiki. Opening a page goes to
//! `WikiDocRoute`; the graph over this wiki is one click away and no
//! longer the first thing you see.

use std::collections::BTreeMap;

use architect_ui::prelude::*;
use dioxus::prelude::*;

use crate::routes::Route;

/// A directory in a wiki's tree: subdirectories and the pages at this
/// level. Physical layout, which is what an outside editor sees too
/// (`wiki.local.mount`).
#[derive(Default)]
pub struct DirNode {
    pub dirs: BTreeMap<String, DirNode>,
    pub pages: Vec<wiki_proto::pages::PageInfo>,
}

/// Build the directory tree from a flat page list.
#[must_use]
pub fn build_tree(pages: &[wiki_proto::pages::PageInfo]) -> DirNode {
    let mut root = DirNode::default();
    for page in pages {
        let mut node = &mut root;
        let mut segs: Vec<&str> = page.path.split('/').collect();
        let _file = segs.pop();
        for seg in segs {
            node = node.dirs.entry(seg.to_owned()).or_default();
        }
        node.pages.push(page.clone());
    }
    fn sort(node: &mut DirNode) {
        node.pages
            .sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
        for child in node.dirs.values_mut() {
            sort(child);
        }
    }
    sort(&mut root);
    root
}

/// The wiki's own documents — schema, purpose, index, log — are shown
/// apart from the pages people wrote.
fn is_scaffold(path: &str) -> bool {
    matches!(
        path,
        "schema.md" | "purpose.md" | "index.md" | "log.md" | "overview.md"
    )
}

#[component]
pub fn WikiHomeView(org: String, wiki: String) -> Element {
    let account = use_context::<Signal<Option<crate::auth::ActiveAccount>>>();

    // The org is the route's, not the switcher's: under "All" the wiki
    // list spans every org, and this wiki belongs to exactly one.
    let org_sig = use_signal(|| org.clone());
    let org = use_memo(move || Some(org_sig()));
    let route_org = org_sig();

    let wiki_id = wiki.clone();
    let description = use_resource(use_reactive!(|(wiki_id,)| async move {
        let _session = account.read().as_ref().map(|a| a.user_id);
        let slug = org().ok_or_else(|| "no organization selected".to_owned())?;
        let client = crate::vox_clients::establish_for::<
            wiki_proto::service::registry::RegistryClient,
        >(&slug)
        .await?;
        client
            .describe_wiki(wiki_id.clone())
            .await
            .map_err(|e| format!("describe_wiki: {e:?}"))
    }));

    let wiki_id2 = wiki.clone();
    let pages = use_resource(use_reactive!(|(wiki_id2,)| async move {
        let _session = account.read().as_ref().map(|a| a.user_id);
        let slug = org().ok_or_else(|| "no organization selected".to_owned())?;
        crate::feeds::fetch_wiki_pages_of(&slug, &wiki_id2).await
    }));

    // Live: a page written or removed in this wiki re-lists.
    let live_wiki = wiki.clone();
    architect::use_stream(
        move |tx| {
            let slug = org();
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
            // Signals are `Copy`; the hook takes `Fn`, so take a fresh
            // mutable handle per call.
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

    // New page: a title becomes `<Title>.md` at the wiki root, written
    // through the same lane every edit uses.
    let mut new_page = use_signal(String::new);
    let mut page_error = use_signal(|| Option::<String>::None);
    let nav = use_navigator();
    let wiki_for_new = wiki.clone();
    let org_for_nav = route_org.clone();
    let on_new_page = move |e: Event<FormData>| {
        e.prevent_default();
        let title = new_page.read().trim().to_owned();
        if title.is_empty() {
            return;
        }
        let Some(slug) = org() else {
            page_error.set(Some("no organization selected".to_owned()));
            return;
        };
        let wiki_id = wiki_for_new.clone();
        let org_for_nav = org_for_nav.clone();
        spawn(async move {
            let path = format!("{}.md", title.replace('/', "-"));
            let markdown = format!("---\ntitle: \"{title}\"\n---\n\n# {title}\n\n");
            let client = match crate::vox_clients::establish_for::<
                wiki_proto::service::pages::PagesClient,
            >(&slug)
            .await
            {
                Ok(c) => c,
                Err(err) => {
                    page_error.set(Some(err));
                    return;
                }
            };
            match client
                .write_page(wiki_id.clone(), path.clone(), markdown, String::new())
                .await
            {
                Ok(_) => {
                    new_page.set(String::new());
                    page_error.set(None);
                    nav.push(Route::WikiDocRoute {
                        org: org_for_nav.clone(),
                        wiki: wiki_id,
                        path,
                    });
                }
                Err(err) => page_error.set(Some(format!("write_page: {err:?}"))),
            }
        });
    };

    let header = match &*description.read() {
        Some(Ok(d)) => {
            let title = if d.summary.title.is_empty() {
                d.summary.slug.clone()
            } else {
                d.summary.title.clone()
            };
            let vis = d.config.visibility.as_str();
            let editors = d.config.editors.len();
            let gate = d.config.proposers.as_str();
            let purpose = d.summary.purpose.clone();
            let source = d.config.source.clone();
            rsx! {
                header { class: "flex flex-col gap-2",
                    Link {
                        to: Route::WikiRoute {},
                        class: "text-xs text-muted-foreground hover:text-foreground",
                        "← Wikis"
                    }
                    div { class: "flex flex-wrap items-baseline justify-between gap-3",
                        Heading { level: HeadingLevel::H1, class: "tracking-tight", "{title}" }
                        div { class: "flex items-center gap-2 text-xs text-muted-foreground",
                            span { class: "rounded-full border border-border/70 px-2 py-0.5", "{vis}" }
                            span { title: "Who may open an Edit Request",
                                "proposals: {gate}"
                            }
                            span { title: "Accounts holding Editor on this wiki",
                                if editors == 1 { "1 editor" } else { "{editors} editors" }
                            }
                            Link {
                                to: Route::GraphRoute {},
                                class: "underline decoration-border underline-offset-2 hover:text-foreground",
                                "Graph →"
                            }
                        }
                    }
                    if !purpose.is_empty() {
                        Text { variant: TextVariant::Muted, "{purpose}" }
                    }
                    if let Some(src) = source {
                        div { class: "flex flex-wrap items-center gap-2 rounded-lg border border-sky-500/30 bg-sky-500/5 px-3 py-2 text-xs",
                            span { class: "font-medium", "Mirrors a repository" }
                            span { class: "font-mono text-muted-foreground", "{src.url}" }
                            if !src.path.is_empty() {
                                span { class: "font-mono text-muted-foreground", "/{src.path}" }
                            }
                            if src.commit.is_empty() {
                                span { class: "text-amber-600 dark:text-amber-400", "not fetched yet" }
                            } else {
                                span { class: "font-mono text-muted-foreground", title: "{src.commit}",
                                    "@ {src.commit.chars().take(10).collect::<String>()}"
                                }
                            }
                            if !src.last_error.is_empty() {
                                span { class: "basis-full text-destructive", "last fetch failed: {src.last_error}" }
                            }
                        }
                    }
                }
            }
        }
        Some(Err(e)) => rsx! {
            header { class: "flex flex-col gap-2",
                Link {
                    to: Route::WikiRoute {},
                    class: "text-xs text-muted-foreground hover:text-foreground",
                    "← Wikis"
                }
                div { class: "rounded-xl border border-destructive/40 bg-destructive/10 px-4 py-3 text-sm",
                    "Couldn't open this wiki: {e}"
                }
            }
        },
        None => rsx! {
            header { class: "flex flex-col gap-2",
                Link {
                    to: Route::WikiRoute {},
                    class: "text-xs text-muted-foreground hover:text-foreground",
                    "← Wikis"
                }
                Heading { level: HeadingLevel::H1, class: "tracking-tight", "{wiki}" }
            }
        },
    };

    let wiki_for_rows = wiki.clone();
    let org_for_rows = route_org.clone();
    let body = match &*pages.read() {
        Some(Ok(list)) => {
            let (scaffold, content): (Vec<_>, Vec<_>) =
                list.iter().cloned().partition(|p| is_scaffold(&p.path));
            let tree = build_tree(&content);
            rsx! {
                section { class: "flex flex-col gap-3",
                    form { class: "flex items-center gap-2", onsubmit: on_new_page,
                        input {
                            class: "min-w-0 flex-1 rounded-lg border border-border/70 bg-background px-2 py-1 text-sm",
                            placeholder: "New page title",
                            value: "{new_page}",
                            oninput: move |e| new_page.set(e.value()),
                        }
                        button {
                            r#type: "submit",
                            class: "rounded-lg border border-border/70 px-3 py-1 text-sm hover:bg-accent",
                            "Add page"
                        }
                        if let Some(err) = page_error() {
                            span { class: "text-xs text-destructive", "{err}" }
                        }
                    }
                    if content.is_empty() {
                        div { class: "rounded-xl border border-dashed border-border/70 px-6 py-10 text-center text-sm text-muted-foreground",
                            "No pages yet. Add the first one above, or open the wiki's folder in your file sync client and start writing."
                        }
                    } else {
                        div { class: "rounded-xl border border-border/70 bg-card/30 p-2",
                            {dir_rows(&tree, &org_for_rows, &wiki_for_rows, 0)}
                        }
                    }
                    if !scaffold.is_empty() {
                        details { class: "text-sm",
                            summary { class: "cursor-pointer text-xs text-muted-foreground", "About this wiki (schema, purpose, index, log)" }
                            div { class: "mt-2 rounded-xl border border-border/70 bg-card/30 p-2",
                                for p in scaffold.iter() {
                                    {page_row(p, &org_for_rows, &wiki_for_rows, 0)}
                                }
                            }
                        }
                    }
                }
            }
        }
        Some(Err(e)) => rsx! {
            div { class: "rounded-xl border border-destructive/40 bg-destructive/10 px-4 py-3 text-sm",
                "Couldn't list this wiki's pages: {e}"
            }
        },
        None => rsx! {
            div { class: "flex items-center justify-center rounded-xl border border-border/70 bg-card/30 py-16",
                Text { variant: TextVariant::Muted, "Loading pages…" }
            }
        },
    };

    rsx! {
        div { class: "mx-auto flex h-full w-full max-w-5xl flex-col gap-5 overflow-y-auto p-4 sm:p-6 lg:p-8",
            {header}
            {body}
        }
    }
}

fn dir_rows(node: &DirNode, org: &str, wiki: &str, depth: usize) -> Element {
    rsx! {
        for (name, child) in node.dirs.iter() {
            div { key: "{name}",
                div {
                    class: "flex items-center gap-1.5 px-1.5 py-1 text-xs font-semibold uppercase tracking-wide text-muted-foreground",
                    style: "padding-left: {depth * 12 + 6}px",
                    "{name}"
                }
                {dir_rows(child, org, wiki, depth + 1)}
            }
        }
        for p in node.pages.iter() {
            {page_row(p, org, wiki, depth)}
        }
    }
}

fn page_row(page: &wiki_proto::pages::PageInfo, org: &str, wiki: &str, depth: usize) -> Element {
    let title = if page.title.is_empty() {
        page.path.clone()
    } else {
        page.title.clone()
    };
    rsx! {
        Link {
            key: "{page.path}",
            to: Route::WikiDocRoute { org: org.to_owned(), wiki: wiki.to_owned(), path: page.path.clone() },
            class: "flex w-full items-center justify-between gap-3 rounded-md px-1.5 py-1 text-sm text-foreground hover:bg-accent/40",
            style: "padding-left: {depth * 12 + 6}px",
            span { class: "truncate", "{title}" }
            span { class: "shrink-0 font-mono text-[0.65rem] text-muted-foreground", "{page.path}" }
        }
    }
}
