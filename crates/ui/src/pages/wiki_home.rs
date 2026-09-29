//! `/wiki/w/:wiki` — one wiki's home: what it is for, who edits it, and
//! its pages.
//!
//! The pages are listed as the tree they are on disk, the same shape the
//! sidebar shows while you are inside this wiki. Opening a page goes to
//! `WikiDocRoute`; the graph over this wiki is one click away and no
//! longer the first thing you see.
//!
//! The right sidebar is the same
//! [`NoteInspector`](crate::pages::note_inspector) a page of this wiki
//! shows, centred on the wiki's index (`index.md`) — its links, its
//! local graph, its share links — so the home is not a third layout.
//! No note is open here, so Properties has nothing to edit and says
//! so; the Graph tab is the one shown first.

use std::collections::BTreeMap;

use architect_ui::prelude::*;
use dioxus::prelude::*;

use crate::document_session::wiki_vault_id;
use crate::pages::note_inspector::{InspectorTab, NoteInspector};
use crate::pages::vault::{FileMeta, fetch_folder_index};
use crate::routes::Route;

/// The pages the wiki home's inspector centres on, first present wins:
/// the index, else the overview, else the purpose, else the log —
/// the wiki's own front matter, in order of how much of the wiki
/// each one reaches.
const INDEX_PAGES: [&str; 4] = ["index.md", "overview.md", "purpose.md", "log.md"];

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
    // Reactive on the prop (the route re-renders this view in place),
    // never a value captured once at mount.
    let org_sig = {
        let org = org.clone();
        use_memo(use_reactive!(|org| org))
    };
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

    // ── The inspector, centred on the index ───────────────────
    // The vault view of this wiki (`wiki:<slug>`): the folder index
    // the inspector's rows take their titles from, and the index page
    // it centres on — `None` until the index says the page exists.
    let vault_id = wiki_vault_id(&wiki);
    let vault_sig = use_signal(|| vault_id.clone());
    let index_org = route_org.clone();
    let folder_index = use_resource(move || {
        let _session = account.read().as_ref().map(|a| a.user_id);
        let slug = index_org.clone();
        let vault = vault_sig();
        async move { fetch_folder_index(slug, vault).await }
    });
    let index_pages = use_memo(move || match &*folder_index.read_unchecked() {
        Some(Ok(pages)) => pages.clone(),
        _ => Vec::new(),
    });
    let index_path = use_memo(move || {
        let pages = index_pages.read();
        INDEX_PAGES
            .iter()
            .find(|want| pages.iter().any(|p| p.path == **want))
            .map(|p| (*p).to_owned())
    });
    // The inspector's Links/Graph tabs re-pull on this; the wiki
    // stream below bumps it when a page is written.
    let wiki_tick = use_signal(|| 0u64);
    let refresh_key = use_memo(move || wiki_tick());
    // No note is open on the home, so the Properties tab has no doc;
    // the panel reads this context and says so.
    use_context_provider(|| Signal::new(None::<crate::pages::note_properties::FocusedDoc>));
    let shell_right = use_context::<Signal<crate::chrome::RightPanelOpen>>();
    let panel_open = shell_right.read().0;
    let right_tab = use_signal(|| InspectorTab::Graph);
    let home_org = use_memo(move || org_sig());

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
                let (mut folder_index, mut wiki_tick) = (folder_index, wiki_tick);
                folder_index.restart();
                wiki_tick += 1;
            }
        },
    );

    // New page: a title becomes `<folder>/<Title>.md`, written through
    // the same vault path the editor saves by — create-only, so a page
    // already at that path is a message, never an overwrite.
    let mut new_page = use_signal(String::new);
    let mut new_folder = use_signal(String::new);
    let mut page_error = use_signal(|| Option::<String>::None);
    let access = crate::pages::wiki_access::use_wiki_access(org_sig, wiki.clone());
    let names = crate::pages::wiki_access::use_member_names(org_sig);
    let nav = use_navigator();
    // A row or node in the inspector: that page's route, by vault id.
    let on_open = use_callback(move |meta: FileMeta| {
        nav.push(crate::routes::note_route(
            &org_sig.peek(),
            &vault_sig.peek(),
            meta.path,
        ));
    });
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
        let Some(path) = crate::pages::wiki_page::new_page_path(&new_folder.read(), &title) else {
            page_error.set(Some("Give the page a name.".to_owned()));
            return;
        };
        let wiki_id = wiki_for_new.clone();
        let org_for_nav = org_for_nav.clone();
        spawn(async move {
            let seed = crate::pages::wiki_page::page_seed(&title);
            match crate::pages::wiki_page::create_page(
                slug,
                wiki_vault_id(&wiki_id),
                path.clone(),
                seed,
            )
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
                Err(err) => page_error.set(Some(err)),
            }
        });
    };

    // ── Gaps: pages that rest on one voice ────────────────────
    let wiki_for_gaps = wiki.clone();
    let gaps = use_resource(move || {
        let _tick = wiki_tick();
        let slug = org_sig();
        let wiki = wiki_for_gaps.clone();
        async move { crate::feeds::fetch_wiki_gaps(&slug, &wiki).await }
    });

    // ── Edit Requests ─────────────────────────────────────────
    // Open ones, for everyone who can read the wiki: Editors review
    // them, a proposer sees where theirs stands.
    let wiki_for_requests = wiki.clone();
    let requests = use_resource(move || {
        let _tick = wiki_tick();
        let slug = org_sig();
        let wiki = wiki_for_requests.clone();
        async move {
            let client =
                crate::vox_clients::establish_for::<wiki_proto::service::edits::EditsClient>(&slug)
                    .await?;
            client
                .list_edit_requests(wiki, false)
                .await
                .map_err(|e| format!("{e:?}"))
        }
    });

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
                                to: Route::WikiScopedSourcesRoute { org: route_org.clone(), wiki: wiki.clone() },
                                class: "underline decoration-border underline-offset-2 hover:text-foreground",
                                "Sources"
                            }
                            Link {
                                to: Route::GraphRoute { org: route_org.clone(), wiki: wiki.clone() },
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
            let folders = folder_paths(&tree);
            let can_edit = access.read().as_ref().is_some_and(|a| a.can_edit);
            rsx! {
                section { class: "flex flex-col gap-3",
                    {requests_section(&requests.read(), &names.read(), &org_for_rows, &wiki_for_rows)}
                    {paths_section(&content, &org_for_rows, &wiki_for_rows)}
                    {one_voice_section(&gaps.read(), &content, &org_for_rows, &wiki_for_rows)}
                    if can_edit {
                    form { class: "flex flex-wrap items-center gap-2", onsubmit: on_new_page,
                        input {
                            class: "min-w-0 flex-1 rounded-lg border border-border/70 bg-background px-2 py-1 text-sm",
                            placeholder: "New page title",
                            value: "{new_page}",
                            oninput: move |e| new_page.set(e.value()),
                        }
                        if !folders.is_empty() {
                            select {
                                class: "rounded-lg border border-border/70 bg-background px-2 py-1 text-sm",
                                title: "Folder",
                                value: "{new_folder}",
                                onchange: move |e| new_folder.set(e.value()),
                                option { value: "", "(top level)" }
                                for f in folders.iter() {
                                    option { key: "{f}", value: "{f}", "{f}" }
                                }
                            }
                        }
                        button {
                            r#type: "submit",
                            class: "rounded-lg border border-border/70 px-3 py-1 text-sm hover:bg-accent",
                            "Add page"
                        }
                        if let Some(err) = page_error() {
                            span { class: "basis-full text-xs text-destructive", "{err}" }
                        }
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
        div { class: "flex h-full min-h-0 w-full",
            div { class: "mx-auto flex h-full min-w-0 w-full max-w-5xl flex-1 flex-col gap-5 overflow-y-auto p-4 sm:p-6 lg:p-8",
                {header}
                {body}
            }
            // ── Right sidebar (md+): the inspector, on the index ──
            if panel_open {
                aside {
                    class: "hidden w-72 shrink-0 flex-col overflow-y-auto border-l border-border bg-muted/30 md:flex",
                    "data-testid": "note-inspector",
                    NoteInspector {
                        org: home_org,
                        vault_id: vault_id.clone(),
                        path: index_path,
                        refresh_key,
                        pages: index_pages,
                        on_open,
                        tab: right_tab,
                        on_hide: move |()| {
                            let mut o = shell_right;
                            o.set(crate::chrome::RightPanelOpen(false));
                        },
                    }
                }
            }
        }
    }
}

/// Every folder in the tree, as a wiki-relative path, parents first.
fn folder_paths(node: &DirNode) -> Vec<String> {
    fn walk(node: &DirNode, prefix: &str, out: &mut Vec<String>) {
        for (name, child) in &node.dirs {
            let path = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            out.push(path.clone());
            walk(child, &path, out);
        }
    }
    let mut out = Vec::new();
    walk(node, "", &mut out);
    out
}

/// The wiki's open Edit Requests, each a link to its review.
/// "Start here": the wiki's study paths (pages of `type: path`), first
/// thing on its home — a way in when there are thirty pages.
fn paths_section(pages: &[wiki_proto::pages::PageInfo], org: &str, wiki: &str) -> Element {
    let paths: Vec<&wiki_proto::pages::PageInfo> =
        pages.iter().filter(|p| p.page_type == "path").collect();
    if paths.is_empty() {
        return rsx! {};
    }
    rsx! {
        div { class: "flex flex-col gap-2", "data-testid": "wiki-study-paths",
            h2 { class: "text-xs font-semibold uppercase tracking-wider text-muted-foreground", "Start here" }
            div { class: "grid gap-2 sm:grid-cols-2",
                for p in paths {
                    Link {
                        key: "{p.path}",
                        to: Route::WikiDocRoute { org: org.to_owned(), wiki: wiki.to_owned(), path: p.path.clone() },
                        class: "rounded-lg border border-primary/30 bg-primary/5 px-4 py-3 hover:bg-primary/10",
                        div { class: "text-[10px] font-semibold uppercase tracking-wider text-primary", "Study path" }
                        div { class: "font-medium text-foreground", "{p.title}" }
                    }
                }
            }
        }
    }
}

/// "Needs a second voice": pages whose sources are all one author (the
/// `gaps` check). Folded — it is a to-do list, not the front page.
fn one_voice_section(
    gaps: &Option<Result<Vec<wiki_proto::graph::KnowledgeGap>, String>>,
    pages: &[wiki_proto::pages::PageInfo],
    org: &str,
    wiki: &str,
) -> Element {
    let Some(Ok(gaps)) = gaps else {
        return rsx! {};
    };
    let one: Vec<(String, String, String)> = gaps
        .iter()
        .filter(|g| matches!(g.kind, wiki_proto::graph::GapKind::OneVoice))
        .filter_map(|g| {
            let path = g.subjects.first()?.clone();
            let title = pages
                .iter()
                .find(|p| p.path == path)
                .map_or_else(|| path.clone(), |p| p.title.clone());
            Some((path, title, g.explanation.clone()))
        })
        .collect();
    if one.is_empty() {
        return rsx! {};
    }
    let n = one.len();
    rsx! {
        details { class: "rounded-lg border border-amber-500/30 bg-amber-500/5 px-4 py-2", "data-testid": "wiki-one-voice",
            summary { class: "cursor-pointer text-sm",
                span { class: "font-medium", "Needs a second voice" }
                span { class: "ml-2 text-muted-foreground", "{n} pages rest on one author's sources" }
            }
            ul { class: "mt-2 flex flex-col gap-1 pb-1 text-sm",
                for (path , title , why) in one {
                    li { key: "{path}",
                        Link {
                            to: Route::WikiDocRoute { org: org.to_owned(), wiki: wiki.to_owned(), path: path.clone() },
                            class: "text-foreground hover:underline",
                            title: "{why}",
                            "{title}"
                        }
                    }
                }
            }
        }
    }
}

fn requests_section(
    requests: &Option<Result<Vec<wiki_proto::service::edits::EditRequest>, String>>,
    names: &std::collections::HashMap<String, String>,
    org: &str,
    wiki: &str,
) -> Element {
    let list = match requests {
        Some(Ok(list)) => list,
        Some(Err(e)) => {
            return rsx! {
                div { class: "text-xs text-muted-foreground", "Couldn't list Edit Requests: {e}" }
            };
        }
        None => return rsx! {},
    };
    if list.is_empty() {
        return rsx! {};
    }
    rsx! {
        div { class: "flex flex-col gap-1 rounded-xl border border-border/70 bg-card/30 p-2",
            "data-testid": "edit-requests",
            div { class: "px-1.5 pb-1 text-xs font-semibold uppercase tracking-wide text-muted-foreground",
                if list.len() == 1 { "1 open Edit Request" } else { "{list.len()} open Edit Requests" }
            }
            for r in list.iter() {
                Link {
                    key: "{r.id}",
                    to: Route::WikiRequestRoute { org: org.to_owned(), wiki: wiki.to_owned(), id: r.id.to_string() },
                    class: "flex w-full items-center justify-between gap-3 rounded-md px-1.5 py-1 text-sm hover:bg-accent/40",
                    span { class: "flex min-w-0 items-center gap-2",
                        {crate::pages::wiki_request::status_pill(r.status)}
                        span { class: "truncate", "{r.title}" }
                    }
                    span { class: "shrink-0 text-xs text-muted-foreground",
                        {crate::pages::wiki_access::name_of(names, &r.proposer)}
                    }
                }
            }
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
