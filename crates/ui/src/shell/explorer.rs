//! The persistent explorer — Obsidian's file tree as the app's main
//! sidebar (the vault is the navigation substrate; pages are views
//! over it).
//!
//! Self-contained: fetches a vault's folder index, renders the same
//! virtual-folder tree the vault page builds, and *navigates* on click
//! — selection is the current route, so the explorer, deep links,
//! wikilinks, and sidebar shortcuts all agree on what "open" means.
//! Editing affordances (create, move) stay on the vault page; this is
//! the map, not the workshop.
//!
//! **Which vault** is a prop. Bare, it is the org switcher's own vault
//! (`"default"`). Given a `wiki`, it is that wiki's pages served as the
//! vault `wiki:<slug>` — the same component, rows routing to the wiki
//! page instead of the vault page. The header switches between the two:
//! **Vault | Wikis**.
//!
//! **One view: by parent.** Every note names one parent (`folder:` /
//! `up:`), and the tree nests it there; a parent is a note itself — tap
//! opens it, the chevron folds it. There is no tag view: tags are found
//! by search, and one structure is all anyone has to keep in order.
//! Records and tasks are left out (`is_note`) — their apps list them.

use std::collections::HashSet;
use std::rc::Rc;

use architect_ui::lucide_dioxus::{
    BookOpen, Brain, Briefcase, Calendar, ChefHat, ChevronRight, Church, DollarSign, Dumbbell,
    FileText, Flame, Globe, GraduationCap, Hash, Heart, HeartPulse, House, Leaf, ListTodo, MapPin,
    Moon, Music, NotebookPen, Package, PenLine, Repeat, Sparkles, SquareKanban, Star, Sun, Users,
    Utensils, Wrench,
};
use architect_ui::prelude::*;
use dioxus::prelude::*;

use crate::document_session::{VAULT_ID, wiki_vault_id};
use crate::pages::vault::{TreeNode, build_tree, fetch_folder_index};
use crate::routes::Route;

/// Whether a page belongs in the notes navigator. Records — captures,
/// contacts, bookings, under `Records/` — and tasks are vault files the
/// apps own and list on their own pages; among the notes they buried
/// the notes (the seeded inbox alone filled "Unfiled").
fn is_note(page: &vault_proto::PageMeta) -> bool {
    !page.path.starts_with("Records/") && page.page_type != "task"
}

/// Which vault the explorer shows and where its rows go. `Clone`d into
/// every row closure.
#[derive(Clone, PartialEq)]
struct ExplorerScope {
    /// The route's org: empty for the org's own vault (the vault route
    /// follows the switcher), the owning org for a wiki.
    org: String,
    vault_id: String,
    /// The wiki slug when this is a wiki explorer.
    wiki: Option<String>,
}

impl ExplorerScope {
    fn route(&self, path: String) -> Route {
        crate::routes::note_route(&self.org, &self.vault_id, path)
    }

    fn is_wiki(&self) -> bool {
        self.wiki.is_some()
    }
}

/// Render the curated icon for a tag's frontmatter `icon:` name.
/// A plain match over compiled components — dynamic name lookup
/// isn't possible with compiled icons, and a curated set keeps the
/// sidebar coherent. Unknown/unset names get the `#` glyph.
fn tag_icon(name: &str) -> Element {
    match name {
        "flame" => rsx! { Flame { size: 13 } },
        "heart" => rsx! { Heart { size: 13 } },
        "heart-pulse" => rsx! { HeartPulse { size: 13 } },
        "sparkles" => rsx! { Sparkles { size: 13 } },
        "calendar" => rsx! { Calendar { size: 13 } },
        "pen-line" => rsx! { PenLine { size: 13 } },
        "dollar-sign" => rsx! { DollarSign { size: 13 } },
        "repeat" => rsx! { Repeat { size: 13 } },
        "sun" => rsx! { Sun { size: 13 } },
        "moon" => rsx! { Moon { size: 13 } },
        "book-open" => rsx! { BookOpen { size: 13 } },
        "dumbbell" => rsx! { Dumbbell { size: 13 } },
        "brain" => rsx! { Brain { size: 13 } },
        "music" => rsx! { Music { size: 13 } },
        "church" => rsx! { Church { size: 13 } },
        "graduation-cap" => rsx! { GraduationCap { size: 13 } },
        "utensils" => rsx! { Utensils { size: 13 } },
        "briefcase" => rsx! { Briefcase { size: 13 } },
        "leaf" => rsx! { Leaf { size: 13 } },
        "star" => rsx! { Star { size: 13 } },
        "list-todo" => rsx! { ListTodo { size: 13 } },
        "house" => rsx! { House { size: 13 } },
        "wrench" => rsx! { Wrench { size: 13 } },
        "globe" => rsx! { Globe { size: 13 } },
        "users" => rsx! { Users { size: 13 } },
        "package" => rsx! { Package { size: 13 } },
        "map-pin" => rsx! { MapPin { size: 13 } },
        "notebook-pen" => rsx! { NotebookPen { size: 13 } },
        "chef-hat" => rsx! { ChefHat { size: 13 } },
        _ => rsx! { Hash { size: 13 } },
    }
}

/// The sidebar tree over one vault. Bare, the org switcher's vault;
/// with `org` + `wiki`, that wiki's pages (vault `wiki:<slug>`), with
/// the way back to the wiki list and the wiki's name above the tree.
#[component]
pub fn VaultExplorer(
    #[props(default)] org: String,
    #[props(default)] wiki: String,
    /// Buttons at the right of the Vault | Wikis row (the phone drawer's
    /// New note), so the switch is the header rather than a row under one.
    #[props(default)]
    actions: Option<Element>,
    /// Under that row, above the tree (the drawer's search bar).
    #[props(default)]
    below: Option<Element>,
) -> Element {
    let org_list = use_context::<Signal<Vec<crate::orgs::OrgMeta>>>();
    let selection = use_context::<Signal<crate::orgs::OrgSelection>>();
    // A wiki belongs to one org — the route's — and under "All" the
    // switcher has none; the vault follows the switcher. The shell
    // keeps one explorer mounted across routes, so the org is a
    // reactive prop, not a value captured at first mount: an explorer
    // born on `/wiki` (no org) that later shows a wiki would otherwise
    // route every row to `/wiki/w//<wiki>/...`.
    let route_org = use_memo(use_reactive!(|org| org));
    let active = use_memo(move || {
        let fixed = route_org();
        if fixed.is_empty() {
            crate::orgs::active_slug(&selection.read(), &org_list.read())
        } else {
            fixed
        }
    });
    let scope = use_memo(use_reactive!(|wiki, org| {
        if wiki.is_empty() {
            ExplorerScope {
                org: String::new(),
                vault_id: VAULT_ID.to_owned(),
                wiki: None,
            }
        } else {
            ExplorerScope {
                org,
                vault_id: wiki_vault_id(&wiki),
                wiki: Some(wiki),
            }
        }
    }));
    let is_wiki = scope.read().is_wiki();
    let mut files = use_resource(move || {
        let slug = active();
        let vault_id = scope.read().vault_id.clone();
        async move {
            fetch_folder_index(slug, vault_id)
                .await
                .map(|pages| pages.into_iter().filter(is_note).collect::<Vec<_>>())
        }
    });
    // No asset filter here, and its absence is the point. ADR 0004
    // moved charts and songs out of the vault entirely, onto
    // `<org>/assets/<kind>/` shelves of their own — so `folder_index`
    // over the vault never sees one, and a filter would be code
    // guarding a case that can no longer arise. The earlier draft that
    // filed them at `<vault>/Assets/` needed one precisely because it
    // had put a shelf in the middle of somebody's notes.
    let tree = use_memo(move || match &*files.read_unchecked() {
        Some(Ok(pages)) => Some(Rc::new(build_tree(pages))),
        _ => None,
    });

    // Live: a note saved here, by another client, or by an external
    // writer (the wiki pipeline, the CLI) re-pulls the index — the
    // tree is the map, and a map that lags the territory teaches
    // people to distrust it. The stream is unfiltered across vault
    // ids; keep the one this explorer shows.
    architect::use_stream(
        move |tx| {
            let slug = active();
            async move {
                if slug.is_empty() {
                    return false;
                }
                let Ok(client) =
                    crate::vox_clients::establish_for::<vault_proto::VaultSyncStreamClient>(&slug)
                        .await
                else {
                    return false;
                };
                client.changes(tx).await.is_ok()
            }
        },
        move |change: vault_proto::VaultChange| {
            let mut files = files;
            if change.vault_id != scope.peek().vault_id {
                return;
            }
            let path = match &change.event {
                vault_proto::VaultEvent::Put { path, .. }
                | vault_proto::VaultEvent::Delete { path } => path.as_str(),
                vault_proto::VaultEvent::Resync => {
                    files.restart();
                    return;
                }
            };
            let ext = std::path::Path::new(path)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or_default();
            if ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("base") {
                files.restart();
            }
        },
    );

    // Unfiled starts collapsed — it exists to be ignorable.
    let collapsed = use_signal(|| HashSet::<String>::from(["unfiled".to_string()]));
    // Parent notes start COLLAPSED (an overview, not a wall) — this set
    // holds what the person has opened.
    let folder_expanded = use_signal(HashSet::<String>::new);

    // The wiki's own name, for the header of a wiki explorer.
    let wiki_title = use_resource(move || {
        let s = scope.read().clone();
        let slug = active();
        async move {
            let wiki = s.wiki?;
            crate::feeds::fetch_wikis(&slug)
                .await
                .ok()
                .and_then(|list| list.into_iter().find(|w| w.slug == wiki))
                .map(|w| if w.title.is_empty() { w.slug } else { w.title })
        }
    });

    // The org's wikis — reference material, AI summaries, skills:
    // everything that isn't the person's own writing. The header's
    // Vault | Wikis switch lists them; each opens its home, and its
    // pages then fill this same tree.
    let wikis = use_resource(move || {
        let slug = active();
        async move { crate::feeds::fetch_wikis(&slug).await }
    });
    // Showing the list of wikis rather than a tree.
    let mut wiki_list = use_signal(|| false);
    let nav = use_navigator();

    // Selection = the current route's path in THIS vault.
    let route = use_route::<Route>();
    let selected = match (&route, &*scope.read()) {
        (Route::VaultRoute { path, .. }, s) if !s.is_wiki() => path.clone(),
        (Route::WikiDocRoute { wiki, path, .. }, s) if s.wiki.as_deref() == Some(wiki) => {
            path.clone()
        }
        _ => String::new(),
    };

    let scope_now = scope.read().clone();
    let heading = wiki_title
        .read()
        .clone()
        .flatten()
        .or_else(|| scope_now.wiki.clone())
        .unwrap_or_default();
    let on_wikis = is_wiki || wiki_list();
    let seg = |on: bool| {
        if on {
            "rounded-full bg-accent px-3.5 py-1.5 text-[17px] font-semibold text-foreground md:px-2 md:py-0.5 md:text-xs md:font-medium"
        } else {
            "rounded-full px-3.5 py-1.5 text-[17px] font-semibold text-muted-foreground hover:text-foreground md:px-2 md:py-0.5 md:text-xs md:font-medium"
        }
    };

    rsx! {
        div { class: "flex h-full min-h-0 flex-col",
            // Vault | Wikis — the same kind of thing, kept apart: your
            // own writing, and the reference shelves beside it.
            div { class: "flex items-center gap-1 px-3 pb-2 pt-3",
                button {
                    r#type: "button",
                    "data-testid": "explorer-vault",
                    "aria-pressed": "{!on_wikis}",
                    class: seg(!on_wikis),
                    onclick: move |_| {
                        wiki_list.set(false);
                        if is_wiki {
                            nav.push(Route::VaultRoute { path: String::new(), org: String::new() });
                        }
                    },
                    "Vault"
                }
                button {
                    r#type: "button",
                    "data-testid": "explorer-wikis",
                    "aria-pressed": "{on_wikis}",
                    class: seg(on_wikis),
                    onclick: move |_| wiki_list.set(true),
                    "Wikis"
                }
                if let Some(a) = actions {
                    div { class: "ml-auto flex items-center gap-1", {a} }
                }
            }
            if let Some(b) = below {
                {b}
            }
            // Inside a wiki: its name, and the way back to the list.
            if let (Some(wiki), false) = (scope_now.wiki.clone(), wiki_list()) {
                div { class: "flex items-center gap-1 px-2 pb-1",
                    button {
                        r#type: "button",
                        class: "flex size-9 shrink-0 items-center justify-center rounded-full text-muted-foreground hover:bg-accent/40 md:size-6",
                        aria_label: "All wikis",
                        onclick: move |_| wiki_list.set(true),
                        span { class: "rotate-180", ChevronRight { size: 14 } }
                    }
                    Link {
                        to: Route::WikiHomeRoute { org: scope_now.org.clone(), wiki: wiki.clone() },
                        class: "flex min-w-0 items-center gap-1.5 text-[15px] font-semibold text-foreground md:text-[13px]",
                        span { class: "truncate", "{heading}" }
                    }
                }
            }
            div { class: "min-h-0 flex-1 overflow-y-auto pb-2",
                if wiki_list() {
                    nav { class: "flex flex-col gap-px px-1.5",
                        match &*wikis.read_unchecked() {
                            Some(Ok(list)) if list.is_empty() => rsx! {
                                div { class: "px-3 py-2 text-sm text-muted-foreground", "No wikis in this org yet." }
                            },
                            Some(Ok(list)) => rsx! {
                                for w in list.iter() {
                                    Link {
                                        key: "{w.slug}",
                                        "data-testid": "wiki-shelf-wiki",
                                        to: Route::WikiHomeRoute { org: active(), wiki: w.slug.clone() },
                                        onclick: move |_| wiki_list.set(false),
                                        class: "flex w-full items-center gap-2 rounded-lg px-2 min-h-11 text-left text-[16px] text-foreground/85 hover:bg-accent/40 md:min-h-0 md:rounded-md md:py-1 md:text-[13px]",
                                        span { class: "flex size-5 shrink-0 items-center justify-center text-muted-foreground [&_svg]:size-4 md:[&_svg]:size-3",
                                            BookOpen { size: 12 }
                                        }
                                        span { class: "truncate",
                                            if w.title.is_empty() { "{w.slug}" } else { "{w.title}" }
                                        }
                                        span { class: "ml-auto text-[0.7rem] tabular-nums text-muted-foreground/60", "{w.pages}" }
                                    }
                                }
                            },
                            Some(Err(e)) => rsx! {
                                div { class: "px-3 py-2 text-sm text-muted-foreground", "{e}" }
                            },
                            None => rsx! {
                                div { class: "px-3 py-2", Spinner { size: SpinnerSize::Small } }
                            },
                        }
                    }
                } else {
                match &*files.read_unchecked() {
                    Some(Ok(_)) => {
                        let t = tree().expect("tree follows files");
                        let nodes = Rc::new(t.0.clone());
                        // Folder notes first; parentless plain notes go
                        // to a collapsed Unfiled dropdown (same idea as
                        // Untagged — ignorable by default).
                        let (folder_roots, loose): (Vec<usize>, Vec<usize>) = t
                            .1
                            .iter()
                            .partition(|&&i| nodes[i].is_folder);
                        let loose_pages: Vec<vault_proto::PageMeta> =
                            loose.iter().map(|&i| nodes[i].meta.clone()).collect();
                        rsx! {
                            nav { class: "flex flex-col gap-px px-1.5",
                                if folder_roots.is_empty() && loose_pages.is_empty() {
                                    div { class: "px-3 py-2 text-xs text-muted-foreground", "No pages yet." }
                                }
                                for &root in folder_roots.iter() {
                                    {explorer_node(nodes.clone(), root, 0, folder_expanded, selected.clone(), &scope_now)}
                                }
                                if !loose_pages.is_empty() {
                                    {loose_section("unfiled", "Unfiled", &loose_pages, collapsed, selected.clone(), &scope_now)}
                                }
                            }
                        }
                    }
                    Some(Err(e)) => rsx! {
                        div { class: "px-1.5 py-1",
                            crate::states::InlineError {
                                message: e.clone(),
                                label: if is_wiki { "Wiki".to_string() } else { "Vault".to_string() },
                                on_retry: move |()| files.restart(),
                            }
                        }
                    },
                    None => rsx! {
                        div { class: "flex items-center gap-2 px-3 py-2 text-xs text-muted-foreground",
                            Spinner { size: SpinnerSize::Small }
                            if is_wiki { "Loading pages…" } else { "Loading vault…" }
                        }
                    },
                }
                }
            }
            // Presence lives in the top bar (avatar group); account,
            // theme, and org switching live at the rail's foot — the
            // explorer is pure vault tree now.
        }
    }
}

/// `expanded` is inverted relative to the loose sections' `collapsed`
/// set: folders start closed, opened basenames are recorded.
fn explorer_node(
    nodes: Rc<Vec<TreeNode>>,
    idx: usize,
    depth: usize,
    mut expanded: Signal<HashSet<String>>,
    selected: String,
    scope: &ExplorerScope,
) -> Element {
    let node = nodes[idx].clone();
    let nav = use_navigator();
    let is_folder = node.is_folder;
    let is_base = std::path::Path::new(&node.meta.path)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("base"));
    let key = node.meta.basename.to_lowercase();
    let is_collapsed = !expanded.read().contains(&key);
    let is_selected = !selected.is_empty() && node.meta.path == selected;
    let indent = depth * 12;

    let row_cls = if is_selected {
        "flex w-full items-center gap-1.5 rounded-lg bg-accent px-1.5 min-h-11 text-left text-[16px] md:min-h-0 md:rounded-md md:py-1 md:text-[13px] text-foreground"
    } else {
        "flex w-full items-center gap-1.5 rounded-lg px-1.5 min-h-11 text-left text-[16px] md:min-h-0 md:rounded-md md:py-1 md:text-[13px] text-muted-foreground hover:bg-accent/40 hover:text-foreground"
    };
    let toggle_key = key.clone();
    let row_key = key.clone();
    let path = node.meta.path.clone();
    let title = node.meta.title.clone();
    let chevron = if is_collapsed { "" } else { "rotate-90" };
    let row_scope = scope.clone();

    rsx! {
        div { key: "{node.meta.path}",
            button {
                r#type: "button",
                class: "{row_cls}",
                style: "padding-left: {indent + 6}px",
                onclick: move |_| {
                    // Clicking the row OPENS the note and reveals a folder's
                    // children. Clicking the row of the note that's ALREADY
                    // open toggles the dropdown instead — so collapsing
                    // doesn't require hunting the tiny chevron.
                    if is_folder {
                        if is_selected {
                            let mut set = expanded.write();
                            if !set.remove(&row_key) {
                                set.insert(row_key.clone());
                            }
                            return;
                        }
                        expanded.write().insert(row_key.clone());
                    }
                    nav.push(row_scope.route(path.clone()));
                },
                if is_folder {
                    span {
                        class: "-my-2 flex size-9 shrink-0 items-center justify-center transition-transform md:my-0 md:size-3 {chevron} [&_svg]:size-4 md:[&_svg]:size-[11px]",
                        onclick: move |e| {
                            e.stop_propagation();
                            let mut set = expanded.write();
                            if !set.remove(&toggle_key) {
                                set.insert(toggle_key.clone());
                            }
                        },
                        ChevronRight { size: 11 }
                    }
                    span { class: "flex size-5 shrink-0 items-center justify-center [&_svg]:size-4 md:size-3.5 md:[&_svg]:size-3 text-muted-foreground/80",
                        {tag_icon(&node.meta.icon)}
                    }
                } else if is_base {
                    // Base views get a board glyph — they open as live
                    // views, not text.
                    span { class: "flex size-5 shrink-0 items-center justify-center [&_svg]:size-4 md:size-3.5 md:[&_svg]:size-3 text-primary",
                        SquareKanban { size: 12 }
                    }
                } else {
                    span { class: "flex size-5 shrink-0 items-center justify-center [&_svg]:size-4 md:size-3.5 md:[&_svg]:size-3",
                        FileText { size: 12 }
                    }
                }
                span { class: "truncate", "{title}" }
            }
            if is_folder && !is_collapsed {
                for &child in node.children.iter() {
                    {explorer_node(nodes.clone(), child, depth + 1, expanded, selected.clone(), scope)}
                }
            }
        }
    }
}

/// One tag virtual folder row + its children (pages, then subtags).
/// A single note row (tag mode) — same look as the folder tree's
/// file rows; clicking navigates.
fn page_row(
    page: &vault_proto::PageMeta,
    depth: usize,
    selected: String,
    scope: &ExplorerScope,
) -> Element {
    let nav = use_navigator();
    let is_base = std::path::Path::new(&page.path)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("base"));
    let is_selected = !selected.is_empty() && page.path == selected;
    let indent = depth * 12;
    let row_cls = if is_selected {
        "flex w-full items-center gap-1.5 rounded-lg bg-accent px-1.5 min-h-11 text-left text-[16px] md:min-h-0 md:rounded-md md:py-1 md:text-[13px] text-foreground"
    } else {
        "flex w-full items-center gap-1.5 rounded-lg px-1.5 min-h-11 text-left text-[16px] md:min-h-0 md:rounded-md md:py-1 md:text-[13px] text-muted-foreground hover:bg-accent/40 hover:text-foreground"
    };
    let path = page.path.clone();
    let title = page.title.clone();
    let row_scope = scope.clone();
    // Wiki pages are a distinct row kind from vault notes (different
    // route) — distinct testid, so a test can't match one for the other.
    let testid = if scope.is_wiki() {
        "wiki-page"
    } else {
        "vault-note"
    };

    rsx! {
        button {
            key: "{page.path}",
            // Test hooks: the multiplayer suite used to find note rows
            // with `aside button` + text, which silently matched the
            // MOBILE sidebar's hidden copy (this explorer renders no
            // <aside>) and waited forever on an invisible element.
            // Select by path, not by display title — two notes in
            // different folders can share a title.
            "data-testid": testid,
            "data-path": "{page.path}",
            r#type: "button",
            class: "{row_cls}",
            style: "padding-left: {indent + 6}px",
            onclick: move |_| {
                nav.push(row_scope.route(path.clone()));
            },
            if is_base {
                span { class: "flex size-5 shrink-0 items-center justify-center [&_svg]:size-4 md:size-3.5 md:[&_svg]:size-3 text-primary",
                    SquareKanban { size: 12 }
                }
            } else {
                span { class: "flex size-5 shrink-0 items-center justify-center [&_svg]:size-4 md:size-3.5 md:[&_svg]:size-3",
                    FileText { size: 12 }
                }
            }
            span { class: "truncate", "{title}" }
        }
    }
}

/// A collapsed-by-default dropdown for the leftovers (Untagged in
/// tag mode, Unfiled in folder mode) — it exists to be ignorable.
fn loose_section(
    key: &'static str,
    label: &'static str,
    pages: &[vault_proto::PageMeta],
    mut collapsed: Signal<HashSet<String>>,
    selected: String,
    scope: &ExplorerScope,
) -> Element {
    // The section holding the open page shows it, whatever the toggle.
    let holds_selected = !selected.is_empty() && pages.iter().any(|p| p.path == selected);
    let is_collapsed = collapsed.read().contains(key) && !holds_selected;
    let chevron = if is_collapsed { "" } else { "rotate-90" };
    let count = pages.len();

    rsx! {
        div { class: "mt-1 border-t border-border/40 pt-1",
            button {
                r#type: "button",
                class: "flex w-full items-center gap-1.5 rounded-lg px-1.5 min-h-11 text-left text-[16px] md:min-h-0 md:rounded-md md:py-1 md:text-[13px] text-muted-foreground/70 hover:bg-accent/40 hover:text-foreground",
                onclick: move |_| {
                    let mut set = collapsed.write();
                    if !set.remove(key) {
                        set.insert(key.to_string());
                    }
                },
                span { class: "-my-2 flex size-9 shrink-0 items-center justify-center transition-transform md:my-0 md:size-3 {chevron} [&_svg]:size-4 md:[&_svg]:size-[11px]",
                    ChevronRight { size: 11 }
                }
                span { class: "truncate", "{label}" }
                span { class: "ml-auto text-[0.65rem] tabular-nums text-muted-foreground/60", "{count}" }
            }
            if !is_collapsed {
                for page in pages {
                    {page_row(page, 1, selected.clone(), scope)}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ExplorerScope, is_note};
    use crate::document_session::{VAULT_ID, wiki_vault_id};
    use crate::routes::Route;

    fn page(path: &str, page_type: &str, tags: &[&str]) -> vault_proto::PageMeta {
        vault_proto::PageMeta {
            path: path.to_owned(),
            basename: path.trim_end_matches(".md").to_owned(),
            title: path.to_owned(),
            page_type: page_type.to_owned(),
            folder: String::new(),
            tags: tags.iter().map(|t| (*t).to_owned()).collect(),
            icon: String::new(),
            sha256: String::new(),
            aliases: Vec::new(),
        }
    }

    #[test]
    fn the_navigator_lists_notes_not_records_or_tasks() {
        assert!(is_note(&page("Home.md", "", &[])));
        assert!(is_note(&page("Studio Notes.md", "", &["studio"])));
        assert!(!is_note(&page("Records/inbox/x.md", "inbox-item", &[])));
        assert!(!is_note(&page("Records/contacts/y.md", "contact", &[])));
        assert!(!is_note(&page("Task/bounce-stems.md", "task", &[])));
    }

    #[test]
    fn rows_route_by_the_vault_they_belong_to() {
        let vault = ExplorerScope {
            org: String::new(),
            vault_id: VAULT_ID.to_owned(),
            wiki: None,
        };
        assert!(matches!(
            vault.route("Plans.md".into()),
            Route::VaultRoute { .. }
        ));
        let wiki = ExplorerScope {
            org: "acme-audio".into(),
            vault_id: wiki_vault_id("music-theory"),
            wiki: Some("music-theory".into()),
        };
        assert_eq!(
            wiki.route("Concepts/Modes.md".into()),
            Route::WikiDocRoute {
                org: "acme-audio".into(),
                wiki: "music-theory".into(),
                path: "Concepts/Modes.md".into(),
            }
        );
    }
}
