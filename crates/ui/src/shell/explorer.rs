//! The persistent vault explorer — Obsidian's file tree as the app's
//! main sidebar (the vault is the navigation substrate; pages are
//! views over it).
//!
//! Self-contained: fetches the home org's folder index, renders the
//! same virtual-folder tree the vault page builds, and *navigates*
//! on click (`VaultRoute { path }`) — selection is the current route,
//! so the explorer, deep links, wikilinks, and sidebar shortcuts all
//! agree on what "open" means. Editing affordances (create, move)
//! stay on the vault page; this is the map, not the workshop.

use std::rc::Rc;

use architect_ui::lucide_dioxus::{
    BookOpen, Brain, Briefcase, Calendar, ChefHat, ChevronRight, Church, DollarSign, Dumbbell,
    FileText, Flame, Globe, GraduationCap, Hash, Heart, HeartPulse, House, Leaf, ListTodo, MapPin,
    Moon, Music, NotebookPen, Package, PenLine, Repeat, Sparkles, SquareKanban, Star, Sun, Users,
    Utensils, Wrench,
};
use architect_ui::prelude::*;
use dioxus::prelude::*;

use crate::pages::vault::{TreeNode, build_tree, fetch_folder_index};
use crate::routes::Route;

/// How the explorer organizes the vault. `Folders` is the default —
/// the virt-folder model (obsidian-virt-folder): hierarchy from each
/// note's `folder:`/`up:` wikilink property, folder notes carrying
/// their own `icon:`. `Tags` groups by hierarchical tags instead.
#[derive(Clone, Copy, PartialEq)]
enum ExplorerMode {
    Tags,
    Folders,
}

/// One virtual folder in the tag tree.
#[derive(Default)]
struct TagNode {
    children: std::collections::BTreeMap<String, TagNode>,
    pages: Vec<vault_proto::PageMeta>,
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

/// Display form of a tag segment: first letter capitalized.
fn capitalize(seg: &str) -> String {
    let mut chars = seg.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Icon names declared by tag notes: any page with `type: tag` maps
/// its basename (lowercased — matches tag segments) to its `icon:`.
/// Stored in the vault, not in tagged documents — edit the tag note
/// (e.g. `Tags/Finance.md`) to change the icon everywhere.
fn tag_icon_map(pages: &[vault_proto::PageMeta]) -> std::collections::HashMap<String, String> {
    pages
        .iter()
        .filter(|p| p.page_type == "tag" && !p.icon.is_empty())
        .map(|p| (p.basename.to_lowercase(), p.icon.clone()))
        .collect()
}

/// Build the tag tree: every page lands under each of its tags
/// (hierarchical on `/`); untagged pages are returned separately.
fn build_tag_tree(pages: &[vault_proto::PageMeta]) -> (TagNode, Vec<vault_proto::PageMeta>) {
    let mut root = TagNode::default();
    let mut untagged = Vec::new();
    for page in pages {
        if page.tags.is_empty() {
            untagged.push(page.clone());
            continue;
        }
        for tag in &page.tags {
            let mut node = &mut root;
            for seg in tag.split('/').filter(|s| !s.is_empty()) {
                node = node.children.entry(seg.to_string()).or_default();
            }
            node.pages.push(page.clone());
        }
    }
    fn sort(node: &mut TagNode) {
        node.pages
            .sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
        for child in node.children.values_mut() {
            sort(child);
        }
    }
    sort(&mut root);
    untagged.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
    (root, untagged)
}

/// One physical folder of the wiki tree (the wiki uses real
/// directories — `Concepts/…`, `People/…` — unlike the vault's
/// virtual folders).
#[derive(Default)]
struct WikiDirNode {
    dirs: std::collections::BTreeMap<String, WikiDirNode>,
    pages: Vec<wiki_proto::pages::PageInfo>,
}

/// Nest the flat, path-sorted wiki page list into its directory tree.
fn build_wiki_tree(pages: &[wiki_proto::pages::PageInfo]) -> WikiDirNode {
    let mut root = WikiDirNode::default();
    for page in pages {
        let mut node = &mut root;
        let mut segs: Vec<&str> = page.path.split('/').collect();
        let _file = segs.pop();
        for seg in segs {
            node = node.dirs.entry(seg.to_string()).or_default();
        }
        node.pages.push(page.clone());
    }
    fn sort(node: &mut WikiDirNode) {
        node.pages
            .sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
        for child in node.dirs.values_mut() {
            sort(child);
        }
    }
    sort(&mut root);
    root
}

#[component]
pub fn VaultExplorer() -> Element {
    let org_list = use_context::<Signal<Vec<crate::orgs::OrgMeta>>>();
    let selection = use_context::<Signal<crate::orgs::OrgSelection>>();
    let active = use_memo(move || crate::orgs::active_slug(&selection.read(), &org_list.read()));
    let mut files = use_resource(move || {
        let slug = active();
        async move { fetch_folder_index(slug).await }
    });
    let tree = use_memo(move || match &*files.read_unchecked() {
        Some(Ok(pages)) => Some(Rc::new(build_tree(pages))),
        _ => None,
    });

    // The leftover sections (Untagged / Unfiled) start collapsed —
    // they exist to be ignorable.
    let collapsed = use_signal(|| {
        std::collections::HashSet::<String>::from(["untagged".to_string(), "unfiled".to_string()])
    });
    // Folders and tag buckets both start COLLAPSED (an overview, not
    // a wall) — these sets hold what the user has opened.
    let folder_expanded = use_signal(std::collections::HashSet::<String>::new);
    let tag_expanded = use_signal(std::collections::HashSet::<String>::new);
    // Virtual-folder organization: folder/up properties by default,
    // tags on toggle. FUTURE: persist on the prefs entity.
    let mut mode = use_signal(|| ExplorerMode::Folders);

    // The org wiki (`<org>/wiki/Knowledge/`) — reference material,
    // AI-generated summaries, skills: everything that ISN'T the
    // user's own writing. Its own section below the vault tree so
    // the vault stays purely personal.
    let wiki_files = use_resource(move || {
        let slug = active();
        async move { crate::feeds::fetch_wiki_pages(&slug).await }
    });
    let wiki_expanded = use_signal(std::collections::HashSet::<String>::new);
    // The whole section starts collapsed — the vault is the primary
    // navigation substrate; the wiki is the reference shelf.
    let mut wiki_open = use_signal(|| false);

    // Selection = the current route's vault path.
    let route = use_route::<Route>();
    let (selected, wiki_selected) = match &route {
        Route::VaultRoute { path, .. } => (path.clone(), String::new()),
        Route::WikiPageRoute { path } => (String::new(), path.clone()),
        _ => (String::new(), String::new()),
    };
    // Auto-open the section when a wiki page is the current route
    // (deep link / graph click), so the selection is visible.
    if !wiki_selected.is_empty() && !*wiki_open.peek() {
        wiki_open.set(true);
    }

    rsx! {
        div { class: "flex h-full min-h-0 flex-col",
            div { class: "flex items-center justify-between px-3 pb-1 pt-3",
                span { class: "text-[0.7rem] font-semibold uppercase tracking-[0.18em] text-muted-foreground",
                    "Vault"
                }
                div { class: "flex items-center gap-0.5 rounded-md bg-muted/40 p-0.5",
                    for (m, label) in [(ExplorerMode::Folders, "Folders"), (ExplorerMode::Tags, "Tags")] {
                        button {
                            key: "{label}",
                            r#type: "button",
                            class: if mode() == m {
                                "rounded px-1.5 py-0.5 text-[0.65rem] font-medium bg-accent text-foreground"
                            } else {
                                "rounded px-1.5 py-0.5 text-[0.65rem] text-muted-foreground hover:text-foreground"
                            },
                            onclick: move |_| mode.set(m),
                            "{label}"
                        }
                    }
                }
            }
            div { class: "min-h-0 flex-1 overflow-y-auto pb-2",
                match &*files.read_unchecked() {
                    Some(Ok(pages)) if mode() == ExplorerMode::Tags => {
                        let (root, untagged) = build_tag_tree(pages);
                        let icons = tag_icon_map(pages);
                        rsx! {
                            nav { class: "flex flex-col gap-px px-1.5",
                                for (seg, node) in &root.children {
                                    {tag_node(seg, node, String::new(), 0, tag_expanded, selected.clone(), &icons)}
                                }
                                if !untagged.is_empty() {
                                    {loose_section("untagged", "Untagged", &untagged, collapsed, selected.clone())}
                                }
                            }
                        }
                    }
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
                                for &root in folder_roots.iter() {
                                    {explorer_node(nodes.clone(), root, 0, folder_expanded, selected.clone())}
                                }
                                if !loose_pages.is_empty() {
                                    {loose_section("unfiled", "Unfiled", &loose_pages, collapsed, selected.clone())}
                                }
                            }
                        }
                    }
                    Some(Err(e)) => rsx! {
                        div { class: "px-1.5 py-1",
                            crate::states::InlineError {
                                message: e.clone(),
                                label: "Vault".to_string(),
                                on_retry: move |()| files.restart(),
                            }
                        }
                    },
                    None => rsx! {
                        div { class: "flex items-center gap-2 px-3 py-2 text-xs text-muted-foreground",
                            Spinner { size: SpinnerSize::Small }
                            "Loading vault…"
                        }
                    },
                }
                // ── Wiki: the org's knowledge shelf (not personal notes) ──
                if let Some(Ok(pages)) = &*wiki_files.read_unchecked() {
                    if !pages.is_empty() {
                        {
                            let chevron = if wiki_open() { "rotate-90" } else { "" };
                            let count = pages.len();
                            let tree = build_wiki_tree(pages);
                            rsx! {
                                div { class: "mt-2 border-t border-border/40 pt-1 px-1.5",
                                    button {
                                        r#type: "button",
                                        class: "flex w-full items-center gap-1.5 rounded-md px-1.5 py-1 text-left text-[0.7rem] font-semibold uppercase tracking-[0.18em] text-muted-foreground hover:bg-accent/40 hover:text-foreground",
                                        onclick: move |_| {
                                            let now = *wiki_open.peek();
                                            wiki_open.set(!now);
                                        },
                                        span { class: "flex h-3 w-3 shrink-0 items-center justify-center transition-transform {chevron}",
                                            ChevronRight { size: 11 }
                                        }
                                        span { class: "flex h-3.5 w-3.5 shrink-0 items-center justify-center text-muted-foreground/80",
                                            BookOpen { size: 13 }
                                        }
                                        span { class: "truncate", "Wiki" }
                                        span { class: "ml-auto text-[0.65rem] tabular-nums text-muted-foreground/60", "{count}" }
                                    }
                                    if wiki_open() {
                                        nav { class: "flex flex-col gap-px",
                                            {wiki_dir_children(&tree, String::new(), 0, wiki_expanded, wiki_selected.clone())}
                                        }
                                    }
                                }
                            }
                        }
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
    mut expanded: Signal<std::collections::HashSet<String>>,
    selected: String,
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
        "flex w-full items-center gap-1.5 rounded-md bg-accent px-1.5 py-1 text-left text-[13px] text-foreground"
    } else {
        "flex w-full items-center gap-1.5 rounded-md px-1.5 py-1 text-left text-[13px] text-muted-foreground hover:bg-accent/40 hover:text-foreground"
    };
    let toggle_key = key.clone();
    let row_key = key.clone();
    let path = node.meta.path.clone();
    let title = node.meta.title.clone();
    let chevron = if is_collapsed { "" } else { "rotate-90" };

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
                    nav.push(Route::VaultRoute { path: path.clone(), org: String::new() });
                },
                if is_folder {
                    span {
                        class: "flex h-3 w-3 shrink-0 items-center justify-center transition-transform {chevron}",
                        onclick: move |e| {
                            e.stop_propagation();
                            let mut set = expanded.write();
                            if !set.remove(&toggle_key) {
                                set.insert(toggle_key.clone());
                            }
                        },
                        ChevronRight { size: 11 }
                    }
                    span { class: "flex h-3.5 w-3.5 shrink-0 items-center justify-center text-muted-foreground/80",
                        {tag_icon(&node.meta.icon)}
                    }
                } else if is_base {
                    // Base views get a board glyph — they open as live
                    // views, not text.
                    span { class: "flex h-3.5 w-3.5 shrink-0 items-center justify-center text-primary",
                        SquareKanban { size: 12 }
                    }
                } else {
                    span { class: "flex h-3.5 w-3.5 shrink-0 items-center justify-center",
                        FileText { size: 12 }
                    }
                }
                span { class: "truncate", "{title}" }
            }
            if is_folder && !is_collapsed {
                for &child in node.children.iter() {
                    {explorer_node(nodes.clone(), child, depth + 1, expanded, selected.clone())}
                }
            }
        }
    }
}

/// One tag virtual folder row + its children (pages, then subtags).
/// `expanded` is inverted relative to the folder tree's `collapsed`
/// set: tag buckets start closed, opened paths are recorded.
fn tag_node(
    seg: &str,
    node: &TagNode,
    prefix: String,
    depth: usize,
    mut expanded: Signal<std::collections::HashSet<String>>,
    selected: String,
    icons: &std::collections::HashMap<String, String>,
) -> Element {
    let tag_path = if prefix.is_empty() {
        seg.to_string()
    } else {
        format!("{prefix}/{seg}")
    };
    let key = format!("tag:{tag_path}");
    let is_collapsed = !expanded.read().contains(&key);
    let indent = depth * 12;
    let count = node.pages.len();
    let chevron = if is_collapsed { "" } else { "rotate-90" };
    let toggle_key = key.clone();
    let icon_name = icons.get(&seg.to_lowercase()).cloned().unwrap_or_default();
    let label = capitalize(seg);

    rsx! {
        div { key: "{tag_path}",
            button {
                r#type: "button",
                class: "flex w-full items-center gap-1.5 rounded-md px-1.5 py-1 text-left text-[13px] text-muted-foreground hover:bg-accent/40 hover:text-foreground",
                style: "padding-left: {indent + 6}px",
                onclick: move |_| {
                    let mut set = expanded.write();
                    if !set.remove(&toggle_key) {
                        set.insert(toggle_key.clone());
                    }
                },
                span { class: "flex h-3 w-3 shrink-0 items-center justify-center transition-transform {chevron}",
                    ChevronRight { size: 11 }
                }
                span { class: "flex h-3.5 w-3.5 shrink-0 items-center justify-center text-muted-foreground/80",
                    {tag_icon(&icon_name)}
                }
                span { class: "truncate", "{label}" }
                if count > 0 {
                    span { class: "ml-auto text-[0.65rem] tabular-nums text-muted-foreground/60", "{count}" }
                }
            }
            if !is_collapsed {
                for page in &node.pages {
                    {page_row(page, depth + 1, selected.clone())}
                }
                for (child_seg, child) in &node.children {
                    {tag_node(child_seg, child, tag_path.clone(), depth + 1, expanded, selected.clone(), icons)}
                }
            }
        }
    }
}

/// A single note row (tag mode) — same look as the folder tree's
/// file rows; clicking navigates.
fn page_row(page: &vault_proto::PageMeta, depth: usize, selected: String) -> Element {
    let nav = use_navigator();
    let is_base = std::path::Path::new(&page.path)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("base"));
    let is_selected = !selected.is_empty() && page.path == selected;
    let indent = depth * 12;
    let row_cls = if is_selected {
        "flex w-full items-center gap-1.5 rounded-md bg-accent px-1.5 py-1 text-left text-[13px] text-foreground"
    } else {
        "flex w-full items-center gap-1.5 rounded-md px-1.5 py-1 text-left text-[13px] text-muted-foreground hover:bg-accent/40 hover:text-foreground"
    };
    let path = page.path.clone();
    let title = page.title.clone();

    rsx! {
        button {
            key: "{page.path}",
            // Test hooks: the multiplayer suite used to find note rows
            // with `aside button` + text, which silently matched the
            // MOBILE sidebar's hidden copy (this explorer renders no
            // <aside>) and waited forever on an invisible element.
            // Select by path, not by display title — two notes in
            // different folders can share a title.
            "data-testid": "vault-note",
            "data-path": "{page.path}",
            r#type: "button",
            class: "{row_cls}",
            style: "padding-left: {indent + 6}px",
            onclick: move |_| {
                nav.push(Route::VaultRoute { path: path.clone(), org: String::new() });
            },
            if is_base {
                span { class: "flex h-3.5 w-3.5 shrink-0 items-center justify-center text-primary",
                    SquareKanban { size: 12 }
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

/// A wiki directory's children: pages first, then subdirectories —
/// both at the same depth. `prefix` is the dir path so expand keys
/// stay unique across same-named subdirs.
fn wiki_dir_children(
    node: &WikiDirNode,
    prefix: String,
    depth: usize,
    expanded: Signal<std::collections::HashSet<String>>,
    selected: String,
) -> Element {
    rsx! {
        for page in &node.pages {
            {wiki_page_row(page, depth, selected.clone())}
        }
        for (seg, child) in &node.dirs {
            {wiki_dir_node(seg, child, prefix.clone(), depth, expanded, selected.clone())}
        }
    }
}

/// One wiki directory row + its children. Directories are physical
/// (the wiki root's real layout), collapsed by default.
fn wiki_dir_node(
    seg: &str,
    node: &WikiDirNode,
    prefix: String,
    depth: usize,
    mut expanded: Signal<std::collections::HashSet<String>>,
    selected: String,
) -> Element {
    let dir_path = if prefix.is_empty() {
        seg.to_string()
    } else {
        format!("{prefix}/{seg}")
    };
    let key = format!("wiki-dir:{dir_path}");
    // Auto-expand ancestors of the selected page so deep links land
    // visible.
    let is_collapsed = !expanded.read().contains(&key)
        && !(!selected.is_empty() && selected.starts_with(&format!("{dir_path}/")));
    let indent = depth * 12;
    let chevron = if is_collapsed { "" } else { "rotate-90" };
    let toggle_key = key.clone();
    fn count_pages(n: &WikiDirNode) -> usize {
        n.pages.len() + n.dirs.values().map(count_pages).sum::<usize>()
    }
    let count = count_pages(node);

    rsx! {
        div { key: "{dir_path}",
            button {
                r#type: "button",
                class: "flex w-full items-center gap-1.5 rounded-md px-1.5 py-1 text-left text-[13px] text-muted-foreground hover:bg-accent/40 hover:text-foreground",
                style: "padding-left: {indent + 6}px",
                onclick: move |_| {
                    let mut set = expanded.write();
                    if !set.remove(&toggle_key) {
                        set.insert(toggle_key.clone());
                    }
                },
                span { class: "flex h-3 w-3 shrink-0 items-center justify-center transition-transform {chevron}",
                    ChevronRight { size: 11 }
                }
                span { class: "flex h-3.5 w-3.5 shrink-0 items-center justify-center text-muted-foreground/80",
                    BookOpen { size: 13 }
                }
                span { class: "truncate", "{seg}" }
                span { class: "ml-auto text-[0.65rem] tabular-nums text-muted-foreground/60", "{count}" }
            }
            if !is_collapsed {
                {wiki_dir_children(node, dir_path.clone(), depth + 1, expanded, selected.clone())}
            }
        }
    }
}

/// A single wiki page row — clicking opens the wiki page view.
/// AI-generated pages (frontmatter `ai_generated: true`) carry a
/// sparkles glyph: machine-produced content, not the user's writing.
fn wiki_page_row(page: &wiki_proto::pages::PageInfo, depth: usize, selected: String) -> Element {
    let nav = use_navigator();
    let is_selected = !selected.is_empty() && page.path == selected;
    let indent = depth * 12;
    let row_cls = if is_selected {
        "flex w-full items-center gap-1.5 rounded-md bg-accent px-1.5 py-1 text-left text-[13px] text-foreground"
    } else {
        "flex w-full items-center gap-1.5 rounded-md px-1.5 py-1 text-left text-[13px] text-muted-foreground hover:bg-accent/40 hover:text-foreground"
    };
    let path = page.path.clone();
    let title = page.title.clone();
    let ai = page.ai_generated;
    let tooltip = if ai {
        if page.generated_by.is_empty() {
            "AI generated".to_string()
        } else {
            format!("AI generated · {}", page.generated_by)
        }
    } else {
        String::new()
    };

    rsx! {
        button {
            key: "{page.path}",
            // Wiki pages are a distinct row kind from vault notes
            // (different route, different tree) — distinct testid, so a
            // test can't accidentally match one for the other.
            "data-testid": "wiki-page",
            "data-path": "{page.path}",
            r#type: "button",
            class: "{row_cls}",
            style: "padding-left: {indent + 6}px",
            title: "{tooltip}",
            onclick: move |_| {
                nav.push(Route::WikiPageRoute { path: path.clone() });
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

/// A collapsed-by-default dropdown for the leftovers (Untagged in
/// tag mode, Unfiled in folder mode) — it exists to be ignorable.
fn loose_section(
    key: &'static str,
    label: &'static str,
    pages: &[vault_proto::PageMeta],
    mut collapsed: Signal<std::collections::HashSet<String>>,
    selected: String,
) -> Element {
    let is_collapsed = collapsed.read().contains(key);
    let chevron = if is_collapsed { "" } else { "rotate-90" };
    let count = pages.len();

    rsx! {
        div { class: "mt-1 border-t border-border/40 pt-1",
            button {
                r#type: "button",
                class: "flex w-full items-center gap-1.5 rounded-md px-1.5 py-1 text-left text-[13px] text-muted-foreground/70 hover:bg-accent/40 hover:text-foreground",
                onclick: move |_| {
                    let mut set = collapsed.write();
                    if !set.remove(key) {
                        set.insert(key.to_string());
                    }
                },
                span { class: "flex h-3 w-3 shrink-0 items-center justify-center transition-transform {chevron}",
                    ChevronRight { size: 11 }
                }
                span { class: "truncate", "{label}" }
                span { class: "ml-auto text-[0.65rem] tabular-nums text-muted-foreground/60", "{count}" }
            }
            if !is_collapsed {
                for page in pages {
                    {page_row(page, 1, selected.clone())}
                }
            }
        }
    }
}
