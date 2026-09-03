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
//! vault `wiki:<slug>` — the same component, the same two views
//! (Folders by `folder:` frontmatter, Tags by every tag a note carries),
//! rows routing to the wiki page instead of the vault page. A wiki
//! opens in Tags (its pages are typed and tagged, rarely filed under
//! folder notes), the vault in Folders; the toggle is remembered per
//! vault id in `localStorage`.

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

/// How the explorer organizes the vault. `Folders` is the virt-folder
/// model (obsidian-virt-folder): hierarchy from each note's
/// `folder:`/`up:` wikilink property, folder notes carrying their own
/// `icon:`. `Tags` groups by hierarchical tags instead.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ExplorerMode {
    Tags,
    Folders,
}

impl ExplorerMode {
    /// What a vault opens in when nothing was remembered: a wiki in
    /// Tags — its pages are typed and tagged, seldom filed under
    /// folder notes — and the org's own vault in Folders.
    fn default_for(is_wiki: bool) -> Self {
        if is_wiki { Self::Tags } else { Self::Folders }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Tags => "tags",
            Self::Folders => "folders",
        }
    }

    // Read only where a browser remembers a choice (and by the tests).
    #[cfg_attr(not(any(target_arch = "wasm32", test)), allow(dead_code))]
    fn parse(s: &str) -> Option<Self> {
        match s {
            "tags" => Some(Self::Tags),
            "folders" => Some(Self::Folders),
            _ => None,
        }
    }
}

/// The `localStorage` key the toggle is remembered under — one per
/// vault id, so the vault and each wiki keep their own choice.
#[cfg_attr(not(any(target_arch = "wasm32", test)), allow(dead_code))]
fn mode_key(vault_id: &str) -> String {
    format!("task.explorer.mode.{vault_id}")
}

/// The remembered toggle, or the default for this kind of vault.
fn initial_mode(vault_id: &str, is_wiki: bool) -> ExplorerMode {
    #[cfg(target_arch = "wasm32")]
    {
        let stored = web_sys::window()
            .and_then(|w| w.local_storage().ok().flatten())
            .and_then(|s| s.get_item(&mode_key(vault_id)).ok().flatten())
            .and_then(|v| ExplorerMode::parse(&v));
        if let Some(m) = stored {
            return m;
        }
    }
    let _ = vault_id;
    ExplorerMode::default_for(is_wiki)
}

fn remember_mode(vault_id: &str, mode: ExplorerMode) {
    #[cfg(target_arch = "wasm32")]
    if let Some(s) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = s.set_item(&mode_key(vault_id), mode.as_str());
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (vault_id, mode);
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

/// Structural `type:` values that describe the tree rather than the
/// page — never a grouping of their own.
const STRUCTURAL_TYPES: &[&str] = &["folder", "index", "tag"];

/// The buckets a page lands in: every tag it carries and, when
/// `type_counts`, its `type:` too. A wiki's pages are typed
/// (`concept`, `person`, `source`) far more often than tagged — the
/// wiki schema *is* the type — so a wiki explorer in Tags mode would
/// otherwise show one long Untagged list. The vault keeps tags alone:
/// its `type:` is the note's widget (`song`, `setlist`), not a topic.
fn group_keys(page: &vault_proto::PageMeta, type_counts: bool) -> Vec<String> {
    let mut keys: Vec<String> = page.tags.clone();
    if type_counts {
        let t = page.page_type.trim().to_lowercase();
        if !t.is_empty() && !STRUCTURAL_TYPES.contains(&t.as_str()) && !keys.contains(&t) {
            keys.push(t);
        }
    }
    keys
}

/// Build the tag tree: every page lands under each of its groups
/// (hierarchical on `/`); pages in no group are returned separately.
fn build_tag_tree(
    pages: &[vault_proto::PageMeta],
    type_counts: bool,
) -> (TagNode, Vec<vault_proto::PageMeta>) {
    let mut root = TagNode::default();
    let mut untagged = Vec::new();
    for page in pages {
        let keys = group_keys(page, type_counts);
        if keys.is_empty() {
            untagged.push(page.clone());
            continue;
        }
        for tag in &keys {
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

/// One physical folder of the wiki shelf's tree (the default wiki
/// uses real directories — `Concepts/…`, `People/…` — unlike the
/// vault's virtual folders).
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

/// The sidebar tree over one vault. Bare, the org switcher's vault;
/// with `org` + `wiki`, that wiki's pages (vault `wiki:<slug>`), with
/// the way back to the wiki list and the wiki's name above the tree.
#[component]
pub fn VaultExplorer(#[props(default)] org: String, #[props(default)] wiki: String) -> Element {
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
        async move { fetch_folder_index(slug, vault_id).await }
    });
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

    // The leftover sections (Untagged / Unfiled) start collapsed —
    // they exist to be ignorable.
    let collapsed =
        use_signal(|| HashSet::<String>::from(["untagged".to_string(), "unfiled".to_string()]));
    // Folders and tag buckets both start COLLAPSED (an overview, not
    // a wall) — these sets hold what the user has opened.
    let folder_expanded = use_signal(HashSet::<String>::new);
    let tag_expanded = use_signal(HashSet::<String>::new);
    // Tags for a wiki, Folders for the vault, unless this browser
    // remembers a choice for this vault id.
    let mode = use_signal(|| {
        let s = scope.peek();
        initial_mode(&s.vault_id, s.is_wiki())
    });
    let set_mode = move |m: ExplorerMode| {
        // Signals are `Copy`; a fresh handle keeps this closure `Fn`.
        let mut mode = mode;
        mode.set(m);
        remember_mode(&scope.peek().vault_id, m);
    };

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

    // The org wiki (`<org>/wiki/Knowledge/`) — reference material,
    // AI-generated summaries, skills: everything that ISN'T the
    // user's own writing. Its own section below the vault tree so
    // the vault stays purely personal. A wiki explorer is already
    // inside a wiki and has no shelf.
    let wiki_files = use_resource(move || {
        let slug = active();
        let shelf = !scope.read().is_wiki();
        async move {
            if !shelf {
                return Ok(Vec::new());
            }
            crate::feeds::fetch_wiki_pages(&slug).await
        }
    });
    let wiki_expanded = use_signal(HashSet::<String>::new);
    // The whole section starts collapsed — the vault is the primary
    // navigation substrate; the wiki is the reference shelf.
    let mut wiki_open = use_signal(|| false);

    // Selection = the current route's path in THIS vault.
    let route = use_route::<Route>();
    let (selected, wiki_selected) = match (&route, &*scope.read()) {
        (Route::VaultRoute { path, .. }, s) if !s.is_wiki() => (path.clone(), String::new()),
        (Route::WikiDocRoute { wiki, path, .. }, s) if s.wiki.as_deref() == Some(wiki) => {
            (path.clone(), String::new())
        }
        (Route::WikiPageRoute { path }, s) if !s.is_wiki() => (String::new(), path.clone()),
        _ => (String::new(), String::new()),
    };
    // Auto-open the shelf when a shelf page is the current route
    // (deep link / graph click), so the selection is visible.
    if !wiki_selected.is_empty() && !*wiki_open.peek() {
        wiki_open.set(true);
    }

    let scope_now = scope.read().clone();
    let heading = wiki_title
        .read()
        .clone()
        .flatten()
        .or_else(|| scope_now.wiki.clone())
        .unwrap_or_else(|| "Vault".to_owned());

    rsx! {
        div { class: "flex h-full min-h-0 flex-col",
            if let Some(wiki) = scope_now.wiki.clone() {
                div { class: "flex flex-col gap-1 px-3 pt-3",
                    Link {
                        to: Route::WikiRoute {},
                        class: "text-[0.7rem] text-muted-foreground hover:text-foreground",
                        "← Wikis"
                    }
                    div { class: "flex items-center justify-between gap-2 pb-1",
                        Link {
                            to: Route::WikiHomeRoute { org: scope_now.org.clone(), wiki: wiki.clone() },
                            class: "flex min-w-0 items-center gap-1.5 text-[0.7rem] font-semibold uppercase tracking-[0.18em] text-muted-foreground hover:text-foreground",
                            span { class: "flex h-3.5 w-3.5 items-center justify-center", BookOpen { size: 13 } }
                            span { class: "truncate", "{heading}" }
                        }
                        {mode_toggle(mode(), set_mode)}
                    }
                }
            } else {
                div { class: "flex items-center justify-between px-3 pb-1 pt-3",
                    span { class: "text-[0.7rem] font-semibold uppercase tracking-[0.18em] text-muted-foreground",
                        "{heading}"
                    }
                    {mode_toggle(mode(), set_mode)}
                }
            }
            div { class: "min-h-0 flex-1 overflow-y-auto pb-2",
                match &*files.read_unchecked() {
                    Some(Ok(pages)) if mode() == ExplorerMode::Tags => {
                        let (root, untagged) = build_tag_tree(pages, is_wiki);
                        let icons = tag_icon_map(pages);
                        rsx! {
                            nav { class: "flex flex-col gap-px px-1.5",
                                if root.children.is_empty() && untagged.is_empty() {
                                    div { class: "px-3 py-2 text-xs text-muted-foreground", "No pages yet." }
                                }
                                for (seg, node) in &root.children {
                                    {tag_node(seg, node, String::new(), 0, tag_expanded, selected.clone(), &icons, &scope_now)}
                                }
                                if !untagged.is_empty() {
                                    {loose_section("untagged", "Untagged", &untagged, collapsed, selected.clone(), &scope_now)}
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

/// The Folders | Tags switch.
fn mode_toggle(
    current: ExplorerMode,
    set_mode: impl Fn(ExplorerMode) + Clone + 'static,
) -> Element {
    rsx! {
        div { class: "flex shrink-0 items-center gap-0.5 rounded-md bg-muted/40 p-0.5",
            for (m, label) in [(ExplorerMode::Folders, "Folders"), (ExplorerMode::Tags, "Tags")] {
                {
                    let set_mode = set_mode.clone();
                    rsx! {
                        button {
                            key: "{label}",
                            r#type: "button",
                            "data-testid": "explorer-mode",
                            "data-mode": m.as_str(),
                            "aria-pressed": if current == m { "true" } else { "false" },
                            class: if current == m {
                                "rounded px-1.5 py-0.5 text-[0.65rem] font-medium bg-accent text-foreground"
                            } else {
                                "rounded px-1.5 py-0.5 text-[0.65rem] text-muted-foreground hover:text-foreground"
                            },
                            onclick: move |_| set_mode(m),
                            "{label}"
                        }
                    }
                }
            }
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
        "flex w-full items-center gap-1.5 rounded-md bg-accent px-1.5 py-1 text-left text-[13px] text-foreground"
    } else {
        "flex w-full items-center gap-1.5 rounded-md px-1.5 py-1 text-left text-[13px] text-muted-foreground hover:bg-accent/40 hover:text-foreground"
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
                    {explorer_node(nodes.clone(), child, depth + 1, expanded, selected.clone(), scope)}
                }
            }
        }
    }
}

/// One tag virtual folder row + its children (pages, then subtags).
/// `expanded` is inverted relative to the folder tree's `collapsed`
/// set: tag buckets start closed, opened paths are recorded.
#[allow(clippy::too_many_arguments)]
fn tag_node(
    seg: &str,
    node: &TagNode,
    prefix: String,
    depth: usize,
    mut expanded: Signal<HashSet<String>>,
    selected: String,
    icons: &std::collections::HashMap<String, String>,
    scope: &ExplorerScope,
) -> Element {
    let tag_path = if prefix.is_empty() {
        seg.to_string()
    } else {
        format!("{prefix}/{seg}")
    };
    let key = format!("tag:{tag_path}");
    // A bucket holding the open page starts open, so a deep link
    // lands visible.
    let holds_selected = !selected.is_empty() && node.pages.iter().any(|p| p.path == selected);
    let is_collapsed = !expanded.read().contains(&key) && !holds_selected;
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
                "data-testid": "explorer-tag",
                "data-tag": "{tag_path}",
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
                    {page_row(page, depth + 1, selected.clone(), scope)}
                }
                for (child_seg, child) in &node.children {
                    {tag_node(child_seg, child, tag_path.clone(), depth + 1, expanded, selected.clone(), icons, scope)}
                }
            }
        }
    }
}

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
        "flex w-full items-center gap-1.5 rounded-md bg-accent px-1.5 py-1 text-left text-[13px] text-foreground"
    } else {
        "flex w-full items-center gap-1.5 rounded-md px-1.5 py-1 text-left text-[13px] text-muted-foreground hover:bg-accent/40 hover:text-foreground"
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
    expanded: Signal<HashSet<String>>,
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
    mut expanded: Signal<HashSet<String>>,
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

/// A single shelf page row — clicking opens the default wiki's page
/// view. AI-generated pages (frontmatter `ai_generated: true`) carry a
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
            "data-testid": "wiki-shelf-page",
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
                    {page_row(page, 1, selected.clone(), scope)}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ExplorerMode, ExplorerScope, build_tag_tree, group_keys, initial_mode, mode_key};
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
    fn a_wiki_opens_in_tags_and_the_vault_in_folders() {
        assert_eq!(ExplorerMode::default_for(true), ExplorerMode::Tags);
        assert_eq!(ExplorerMode::default_for(false), ExplorerMode::Folders);
        // Off the web nothing is remembered, so the default is what
        // the first open gets.
        assert_eq!(
            initial_mode(&wiki_vault_id("music-theory"), true),
            ExplorerMode::Tags
        );
        assert_eq!(initial_mode(VAULT_ID, false), ExplorerMode::Folders);
    }

    #[test]
    fn the_toggle_is_remembered_per_vault_id() {
        assert_ne!(mode_key(VAULT_ID), mode_key(&wiki_vault_id("music-theory")));
        assert_ne!(
            mode_key(&wiki_vault_id("cooking")),
            mode_key(&wiki_vault_id("music-theory"))
        );
        assert_eq!(
            ExplorerMode::parse(ExplorerMode::Tags.as_str()),
            Some(ExplorerMode::Tags)
        );
        assert_eq!(
            ExplorerMode::parse(ExplorerMode::Folders.as_str()),
            Some(ExplorerMode::Folders)
        );
        assert_eq!(ExplorerMode::parse("nope"), None);
    }

    /// The seeded wikis carry `type: concept` and few tags. In a wiki
    /// the type is a topic and groups the page; in the vault it is the
    /// note's widget (`song`) and does not.
    #[test]
    fn a_wiki_page_groups_by_its_type_and_a_vault_note_does_not() {
        let concept = page("Concepts/Modes.md", "concept", &[]);
        assert_eq!(group_keys(&concept, true), vec!["concept".to_string()]);
        assert!(group_keys(&concept, false).is_empty());

        let tagged = page("Concepts/Ionian.md", "concept", &["modes"]);
        assert_eq!(
            group_keys(&tagged, true),
            vec!["modes".to_string(), "concept".to_string()]
        );

        // Structural types describe the tree, not the page.
        for structural in ["folder", "index", "tag"] {
            assert!(group_keys(&page("X.md", structural, &[]), true).is_empty());
        }
        // A tag that already names the type is not doubled.
        let both = page("Y.md", "concept", &["concept"]);
        assert_eq!(group_keys(&both, true), vec!["concept".to_string()]);
    }

    #[test]
    fn the_tag_tree_lands_a_page_under_every_group() {
        let pages = vec![
            page("Concepts/Modes.md", "concept", &["theory/scales"]),
            page("People/Bach.md", "person", &[]),
            page("Loose.md", "", &[]),
        ];
        let (root, untagged) = build_tag_tree(&pages, true);
        let theory = root.children.get("theory").expect("theory bucket");
        assert_eq!(
            theory.children.get("scales").map(|n| n.pages.len()),
            Some(1)
        );
        assert_eq!(root.children.get("concept").map(|n| n.pages.len()), Some(1));
        assert_eq!(root.children.get("person").map(|n| n.pages.len()), Some(1));
        assert_eq!(untagged.len(), 1);
        assert_eq!(untagged[0].path, "Loose.md");
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
