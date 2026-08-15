//! `/vault` — browse the org's vault as a **virtual-folder
//! tree** and edit files live in the rich `editor::Editor`.
//!
//! The vault is organized the Obsidian "folder-note" way: each
//! note's `folder: "[[Parent]]"` frontmatter is a wikilink to a
//! parent *folder note*. The sidebar builds an expandable tree
//! from that property (not from physical directories) via the
//! server's [`folder_index`] rpc, which parses frontmatter once
//! and returns lightweight [`PageMeta`]s. Folder notes are real
//! notes: clicking the **name opens the note**, the **chevron
//! toggles** its children. Notes can be **re-filed** from the
//! sidebar (a "move to folder" picker rewrites the `folder`
//! property through [`set_folder`]).
//!
//! **Document tabs + split panes.** The document area is a set of
//! **panes** (a single 2-way horizontal split, max [`MAX_PANES`]);
//! each pane carries its own **tab strip** of open notes and an
//! active tab. Clicking a note in the tree (or a backlink / graph
//! node, or a `?path=` deep link) **opens-or-focuses** a tab in the
//! *focused* pane. Each open note renders as a
//! [`NoteView`](crate::pages::note_view::NoteView) — the per-note
//! `DocumentSession` + collab + `type:`-dispatch, extracted so it can
//! be instantiated once per tab/pane. With a single open note the
//! page looks exactly as it did before tabs existed.
//!
//! The open/save/conflict lifecycle lives in
//! [`DocumentSession`](crate::document_session::DocumentSession):
//! typed conflicts, a debounced autosave, explicit save (Ctrl+S /
//! toolbar), force-save (the conflict banner's *Overwrite*), and
//! reload — each `NoteView` renders from its typed state instead of a
//! hand-rolled signal cluster.
//!
//! Wikilinks + embeds resolve through the client-side
//! [`ClientVaultIndex`](crate::vault_lookup::ClientVaultIndex)
//! (folder-index metadata + lazy `get_file` content LRU), passed
//! to the editor as a stateful `DecorationSource`; `[[` and `#`
//! autocomplete ride the editor's trigger `CompletionSource`
//! (basenames + aliases from the folder index, tags from the
//! `VaultGraph` RPC). A right-side **backlinks panel** lists
//! pages linking to the *focused* note via the same RPC and refreshes
//! after every save.
//!
//! The server registers exactly one vault per org under the id
//! `"default"`.
//!
//! ## Module layout
//!
//! This was one 1,567-line file. The page component is still the
//! biggest piece — it owns the signal graph, so splitting it further
//! means moving state, not just text — but everything that did NOT
//! need the signals now lives beside it:
//!
//! - [`rpc`] — the `VaultSync` round-trips (folder index, links,
//!   backlinks, move-to-folder, create). Transport, not UI.
//! - [`tree`] — the virtual-folder node model ([`TreeNode`],
//!   [`build_tree`]) and its recursive renderer.
//! - [`tabs`] — document tabs and split panes ([`Pane`],
//!   [`MAX_PANES`], the tab strip).
//! - [`graph`] — pure builders for the Links tab's local graph and
//!   verse references.
//!
//! [`folder_index`]: vault_proto::VaultSync::folder_index
//! [`set_folder`]: vault_proto::VaultSync::set_folder
//! [`PageMeta`]: vault_proto::PageMeta

mod graph;
mod rpc;
mod tabs;
mod tree;

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use dioxus::prelude::*;
use architect_ui::lucide_dioxus::Folder;
use architect_ui::prelude::*;
use vault_proto::TagCount;
use view_knowledge_graph::KnowledgeGraphView;

use crate::pages::note_view::NoteView;
use crate::shell::mobile::{BottomSheet, MobileActionBar};

use graph::{build_local_graph, osis_to_ref};
use tabs::{MAX_PANES, OpenTab, Pane, render_tab_bar};
// `build_tree` / `basename_of` / `TreeNode` were free items on this
// module before the split; other pages still import them by those paths.
use tree::render_node;
pub(crate) use tree::{TreeNode, basename_of, build_tree};
// Re-exported: these were free functions on this module before the
// split, and other pages (search, note_view, the mobile shell) import
// them by their old paths.
use crate::vault_lookup;
pub(crate) use rpc::{create_new_file, fetch_folder_index};
use rpc::{fetch_backlinks, fetch_links, move_to_folder};

#[cfg(target_arch = "wasm32")]
use crate::document_session::VAULT_ID;

/// Minimal payload to open a file: its path + last-known sha.
#[derive(Clone, PartialEq)]
pub(crate) struct FileMeta {
    pub(crate) path: String,
    pub(crate) sha256: String,
}

/// The right sidebar's active tab. Properties (the focused note's
/// frontmatter, edited live) and Links (backlinks + outgoing links +
/// local graph).
#[derive(Clone, Copy, PartialEq, Eq)]
enum RightTab {
    Properties,
    Links,
    Share,
}

#[component]
pub fn VaultView(
    #[props(default)] initial_path: ReadSignal<String>,
    #[props(default)] initial_org: ReadSignal<String>,
) -> Element {
    // The vault follows the org switcher: browse the selected org's
    // vault (or the home org when viewing All). A non-empty `initial_org`
    // from the route — a cross-org search hit — overrides, so the note
    // opens in its own org WITHOUT changing the switcher. Re-runs when the
    // selection, route org, or org discovery changes.
    let org_list = use_context::<Signal<Vec<crate::orgs::OrgMeta>>>();
    let selection = use_context::<Signal<crate::orgs::OrgSelection>>();
    let active = use_memo(move || {
        let route_org = initial_org();
        if route_org.is_empty() {
            crate::orgs::active_slug(&selection.read(), &org_list.read())
        } else {
            route_org
        }
    });
    let mut files = use_resource(move || {
        let slug = active();
        async move { fetch_folder_index(slug).await }
    });

    let mut new_name = use_signal(String::new);
    // Failures from tree operations (move / create) outlive their
    // buttons via the app-wide notification queue.
    let notify = architect::try_use_notifications();

    // Tree UI state. `collapsed` holds folders the user has closed.
    // `move_target` is a note being re-filed; `create_parent` is the
    // folder a new note will be filed under.
    let collapsed = use_signal(HashSet::<String>::new);
    let mut move_target = use_signal(|| None::<String>);
    let mut create_parent = use_signal(|| None::<String>);
    // Mobile-only: the file tree lives in a bottom sheet once a note
    // is open (inline full-width while nothing is selected).
    let mut files_open = use_signal(|| false);

    // ── Tabs + split state ────────────────────────────────────
    // `panes` is 1–2 panes, each an ordered tab set + active index;
    // `focused` is the pane that tree/deep-link opens route into and
    // that owns the status line + backlinks. `focus_tick` is bumped by
    // the focused NoteView after each save so the backlinks/links/graph
    // panel refreshes (it used to read `session.save_count`).
    let mut panes = use_signal(|| {
        vec![Pane {
            tabs: Vec::new(),
            active: 0,
        }]
    });
    let mut focused = use_signal(|| 0usize);
    let focus_tick = use_signal(|| 0u64);
    // Bumped by the live-change stream below (someone *else's* write).
    // Separate from `focus_tick`, which NoteView drives off our own
    // save count — sharing one signal would let the two clobber each
    // other. Panels that want "refresh on any commit" read the sum.
    let vault_tick = use_signal(|| 0u64);
    let refresh_key = use_memo(move || *focus_tick.read() + *vault_tick.read());

    // ── Live vault changes ────────────────────────────────────
    // The `VaultSync` `#[subscribe]` stream: every committed write
    // (ours, another client's, or an external edit the server's
    // filesystem watcher picked up) arrives as a `VaultChange`.
    //
    // The folder tree is a *derived* view — `PageMeta.title` /
    // `.folder` come from parsed frontmatter the event doesn't
    // carry — so a change can't be folded into it; it re-pulls
    // `folder_index` instead. The event is the trigger, the rpc
    // stays the source of truth. Same for the backlinks / links /
    // verses panels, which ride `refresh_key`.
    //
    // The subscribe future reads `active`, so switching orgs
    // re-runs the hook and re-subscribes against the new org's
    // stream. Every *re*-subscribe also restarts the tree, which is
    // the recovery path for events published while we were detached
    // (the hub is a sliding mailbox — nothing is replayed).
    let subscribed_once = use_signal(|| false);
    architect::use_stream(
        move |tx| {
            // Signals are `Copy`; the hook takes `Fn`, so take
            // fresh mutable handles per call rather than capturing
            // by mutable reference.
            let (mut files, mut vault_tick, mut subscribed_once) =
                (files, vault_tick, subscribed_once);
            let slug = active();
            if *subscribed_once.peek() {
                files.restart();
                vault_tick += 1;
            }
            subscribed_once.set(true);
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
            let (mut files, mut vault_tick) = (files, vault_tick);
            // The stream is unfiltered — one backend can serve
            // several vault ids. Keep the one this page browses.
            if change.vault_id != crate::document_session::VAULT_ID {
                return;
            }
            let path = match &change.event {
                vault_proto::VaultEvent::Put { path, .. }
                | vault_proto::VaultEvent::Delete { path } => path.as_str(),
                // Explicit server "you missed something" hint.
                vault_proto::VaultEvent::Resync => {
                    files.restart();
                    vault_tick += 1;
                    return;
                }
            };
            let ext = std::path::Path::new(path)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or_default();
            // The tree holds notes + base views only; attachments and
            // sidecars churn without changing it.
            if ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("base") {
                files.restart();
            }
            // A note's body edit can add or drop wikilinks + tags, so
            // the link panels of whatever is focused may be stale.
            if ext.eq_ignore_ascii_case("md") {
                vault_tick += 1;
            }
        },
    );

    // The focused note's live editor doc, published by its `NoteView`
    // and consumed by the right-sidebar Properties tab.
    use_context_provider(|| Signal::new(None::<crate::pages::note_properties::FocusedDoc>));
    // …and the scope that owns those buffers. It must be THIS scope:
    // the context signal above lives here, so anything reachable
    // through it has to outlive the panes that publish into it. See
    // `DocOwnerScope` for the cross-scope read this prevents.
    use_context_provider(|| {
        crate::document_session::DocOwnerScope(dioxus::core::current_scope_id())
    });
    // Which right-sidebar tab is showing. Properties first — it's the
    // one used most while writing.
    let mut right_tab = use_signal(|| RightTab::Properties);
    let nav = use_navigator();

    // Focused pane's active-tab path — drives the tree highlight, the
    // backlinks panel, and the "any note open?" layout switches.
    let selected = use_memo(move || {
        let p = panes.read();
        let f = (*focused.read()).min(p.len().saturating_sub(1));
        p.get(f)
            .and_then(|pane| pane.tabs.get(pane.active))
            .map(|t| t.path.clone())
    });

    // Open-or-focus a note in the focused pane (tree rows, backlinks,
    // wikilinks, graph nodes, `.base` rows, freshly-created notes).
    let on_open = use_callback(move |meta: FileMeta| {
        files_open.set(false);
        let mut p = panes.write();
        let f = (*focused.peek()).min(p.len().saturating_sub(1));
        let Some(pane) = p.get_mut(f) else { return };
        if let Some(i) = pane.tabs.iter().position(|t| t.path == meta.path) {
            pane.active = i;
        } else {
            pane.tabs.push(OpenTab {
                path: meta.path,
                sha: meta.sha256,
            });
            pane.active = pane.tabs.len() - 1;
        }
    });

    // ── Tab / pane controls ───────────────────────────────────
    let focus_tab = use_callback(move |(pi, idx): (usize, usize)| {
        focused.set(pi);
        let mut p = panes.write();
        if let Some(pane) = p.get_mut(pi) {
            if idx < pane.tabs.len() {
                pane.active = idx;
            }
        }
    });
    let close_tab = use_callback(move |(pi, idx): (usize, usize)| {
        let mut removed_pane = false;
        {
            let mut p = panes.write();
            let Some(pane) = p.get_mut(pi) else { return };
            if idx >= pane.tabs.len() {
                return;
            }
            pane.tabs.remove(idx);
            if pane.active >= pane.tabs.len() {
                pane.active = pane.tabs.len().saturating_sub(1);
            }
            // Drop an emptied pane when it isn't the last one.
            if pane.tabs.is_empty() && p.len() > 1 {
                p.remove(pi);
                removed_pane = true;
            }
        }
        if removed_pane {
            let len = panes.read().len();
            if *focused.peek() >= len {
                focused.set(len.saturating_sub(1));
            }
        }
    });
    let split = use_callback(move |()| {
        let new_idx = {
            let mut p = panes.write();
            if p.len() >= MAX_PANES {
                return;
            }
            p.push(Pane {
                tabs: Vec::new(),
                active: 0,
            });
            p.len() - 1
        };
        focused.set(new_idx);
    });
    let close_pane = use_callback(move |pi: usize| {
        let len = {
            let mut p = panes.write();
            if p.len() <= 1 || pi >= p.len() {
                return;
            }
            p.remove(pi);
            p.len()
        };
        if *focused.peek() >= len {
            focused.set(len.saturating_sub(1));
        }
    });
    let focus_pane = use_callback(move |pi: usize| focused.set(pi));

    // Refresh the folder index after a rename commits (tree row path
    // changed) — threaded into every mounted NoteView.
    let on_renamed = use_callback(move |()| files.restart());

    // Deep-link + shell-tree navigation: `/vault?path=<path>` opens a
    // tab in the focused pane once the folder index lands. Reactive on
    // the query param — every NEW `?path=` opens; `last_link` remembers
    // the one already honored so in-page selection isn't stomped.
    let mut last_link = use_signal(String::new);
    use_effect(move || {
        let want = initial_path();
        if want.is_empty() || *last_link.peek() == want {
            return;
        }
        if let Some(Ok(pages)) = &*files.read() {
            let hit = pages.iter().find(|p| p.path == want).or_else(|| {
                pages
                    .iter()
                    .find(|p| basename_of(&p.path) == basename_of(&want))
            });
            if let Some(p) = hit {
                last_link.set(want);
                on_open.call(FileMeta {
                    path: p.path.clone(),
                    sha256: p.sha256.clone(),
                });
            }
        }
    });

    // Pane → route sync: keep the top tab strip (the ONE tab UI now
    // that the inner note-tab bar is hidden in single-pane mode)
    // tracking the focused pane's active note. Shares `last_link` with
    // the route→pane effect above so the two directions can't ping-pong:
    // whichever side moves first stamps `last_link`, and the other sees
    // it already matches and stops.
    use_effect(move || {
        let Some(sel) = selected() else { return };
        if *last_link.peek() == sel {
            return;
        }
        last_link.set(sel.clone());
        nav.push(crate::routes::Route::VaultRoute {
            path: sel,
            org: active(),
        });
    });

    // Autocomplete tags — `#` completes vault tags pulled once per org
    // (re-pulled after each save via `focus_tick`, since saves mint
    // tags). Shared by every mounted NoteView's completion source.
    let mut tag_rows = use_signal(Vec::<TagCount>::new);
    use_effect(move || {
        let slug = active();
        let _refresh = refresh_key();
        spawn(async move {
            if let Ok(tags) = vault_lookup::tag_candidates(slug).await {
                tag_rows.set(tags);
            }
        });
    });

    // Re-file a note under `parent` (None = root) via set_folder,
    // then refresh the tree.
    let do_move = use_callback(
        move |(path, prev_sha, parent): (String, String, Option<String>)| {
            spawn(async move {
                match move_to_folder(active(), path, parent, prev_sha).await {
                    Ok(_new_sha) => {
                        move_target.set(None);
                        files.restart();
                    }
                    Err(e) => {
                        if let Some(n) = notify {
                            n.error(format!("Move failed: {e}"));
                        }
                    }
                }
            });
        },
    );

    // Create a new empty note. If a folder was chosen, file it there,
    // then open a tab — the open re-fetches, so the buffer reflects the
    // server-spliced `folder:` frontmatter.
    let create_file = move || {
        let mut name = new_name.peek().trim().to_owned();
        if name.is_empty() {
            return;
        }
        if !name.to_ascii_lowercase().ends_with(".md") {
            name.push_str(".md");
        }
        let parent = create_parent.peek().clone();
        spawn(async move {
            match create_new_file(active(), name.clone()).await {
                Ok(created_sha) => {
                    new_name.set(String::new());
                    create_parent.set(None);
                    let mut open_sha = created_sha.clone();
                    if let Some(parent) = parent {
                        match move_to_folder(active(), name.clone(), Some(parent), created_sha)
                            .await
                        {
                            Ok(new_sha) => open_sha = new_sha,
                            Err(e) => {
                                if let Some(n) = notify {
                                    n.error(format!("Created, but filing failed: {e}"));
                                }
                            }
                        }
                    }
                    on_open.call(FileMeta {
                        path: name,
                        sha256: open_sha,
                    });
                    files.restart();
                }
                Err(e) => {
                    if let Some(n) = notify {
                        n.error(format!("Create failed: {e}"));
                    }
                }
            }
        });
    };

    // Build the tree from the folder index.
    let tree = use_memo(move || match &*files.read_unchecked() {
        Some(Ok(pages)) => Some(Rc::new(build_tree(pages))),
        _ => None,
    });

    // Folder-index pages threaded into every NoteView (wikilink
    // candidates + cross-file lookup + the `type:` dispatch).
    let pages_memo = use_memo(move || match &*files.read_unchecked() {
        Some(Ok(pages)) => pages.clone(),
        _ => Vec::new(),
    });

    // path → (title, sha) for the backlinks panel rows.
    let page_lookup = use_memo(move || match &*files.read_unchecked() {
        Some(Ok(pages)) => pages
            .iter()
            .map(|p| (p.path.clone(), (p.title.clone(), p.sha256.clone())))
            .collect::<HashMap<String, (String, String)>>(),
        _ => HashMap::new(),
    });

    // Backlinks for the focused note, re-pulled when the selection
    // changes and after every committed save (`focus_tick`).
    let shell_right = use_context::<Signal<crate::chrome::RightPanelOpen>>();
    let backlinks_open = use_memo(move || shell_right.read().0);
    let backlinks = use_resource(move || {
        let slug = active();
        let path = selected();
        let _refresh = refresh_key();
        async move {
            match path {
                Some(p) => fetch_backlinks(slug, p).await,
                None => Ok(Vec::new()),
            }
        }
    });

    // Outgoing wikilinks of the focused note.
    let outlinks = use_resource(move || {
        let slug = active();
        let path = selected();
        let _refresh = refresh_key();
        async move {
            match path {
                Some(p) => fetch_links(slug, p).await,
                None => Ok(Vec::new()),
            }
        }
    });

    // Verses the focused note references (from synced note→verse
    // links), with their text — the inline scripture reader.
    let verses = use_resource(move || {
        let slug = active();
        let path = selected();
        let _refresh = refresh_key();
        async move {
            let Some(p) = path else { return Vec::new() };
            let links = crate::feeds::fetch_links_for(&slug, &format!("note:{p}"))
                .await
                .unwrap_or_default();
            let mut refs: Vec<String> = links
                .iter()
                .filter(|l| l.target.kind == links_proto::NodeKind::Verse)
                .map(|l| l.target.id.clone())
                .collect();
            refs.sort();
            refs.dedup();
            refs.truncate(16);
            let mut out = Vec::new();
            for osis in refs {
                let human = osis_to_ref(&osis);
                let text = crate::feeds::fetch_verse_text(&slug, "WEB", &human)
                    .await
                    .ok();
                out.push((osis, human, text));
            }
            out
        }
    });

    let sidebar_body = match &*files.read_unchecked() {
        Some(Ok(_)) => {
            let Some(t) = tree() else { unreachable!() };
            let (nodes, roots) = (Rc::new(t.0.clone()), t.1.clone());
            rsx! {
                nav { class: "flex flex-col gap-0.5 px-2 pb-4",
                    if roots.is_empty() {
                        div { class: "px-2 py-1 text-sm text-muted-foreground", "Empty vault. Create a note above." }
                    }
                    for &root in roots.iter() {
                        {render_node(nodes.clone(), root, 0, collapsed, selected, on_open, move_target, create_parent)}
                    }
                }
            }
        }
        Some(Err(e)) => rsx! {
            div { class: "px-2 py-2",
                crate::states::InlineError {
                    message: e.clone(),
                    label: "Vault".to_string(),
                }
            }
        },
        None => rsx! {
            div { class: "flex items-center gap-2 px-3 py-2 text-sm text-muted-foreground",
                Spinner { size: SpinnerSize::Small }
                "Loading vault…"
            }
        },
    };

    // Folder targets for the move picker.
    let folder_targets: Vec<(String, String)> = tree()
        .map(|t| {
            t.0.iter()
                .filter(|n| n.is_folder)
                .map(|n| (n.meta.basename.clone(), n.meta.title.clone()))
                .collect()
        })
        .unwrap_or_default();

    let has_file = selected.read().is_some();
    let current = selected.read().clone().unwrap_or_default();
    let verse_list = verses.read().clone();
    let moving = move_target.read().clone();
    let create_under = create_parent.read().clone();
    let panel_open = *backlinks_open.read();

    // ── Status line (focused NoteView writes it; the page reads it for
    //    the mobile action bar's Save affordance) ─────────────────────
    let status_info = use_context::<crate::chrome::StatusBarInfo>().0;
    // Clear the status segments when nothing is open, and on page leave.
    use_effect(move || {
        if selected().is_none() {
            let mut info = status_info;
            info.set(None);
        }
    });
    use_drop(move || {
        let mut info = status_info;
        info.set(None);
    });

    // ── Tree pane content ─────────────────────────────────
    let tree_content = rsx! {
        div { class: "flex flex-col gap-2 px-3 py-3",
            if let Some(parent) = create_under.clone() {
                Text { variant: TextVariant::Muted, class: "text-xs",
                    "New note will be filed under {parent}."
                }
            }
            div { class: "flex items-center gap-2",
                Input {
                    value: new_name,
                    placeholder: "New note…",
                    on_change: move |_| {},
                }
                Button {
                    variant: ButtonVariant::Secondary,
                    size: ButtonSize::Small,
                    on_click: move |_| create_file(),
                    "Create"
                }
            }
        }
        // ── Move-to-folder picker ─────────────────
        if let Some(path) = moving.clone() {
            div { class: "mx-2 mb-2 rounded border border-border bg-background p-2",
                div { class: "flex items-center justify-between gap-2 pb-1",
                    Text { variant: TextVariant::Muted, class: "text-xs truncate", "Move to…" }
                    button {
                        class: "text-xs text-muted-foreground hover:text-foreground",
                        onclick: move |_| move_target.set(None),
                        "Cancel"
                    }
                }
                div { class: "flex max-h-48 flex-col gap-0.5 overflow-y-auto",
                    {
                        let p = path.clone();
                        rsx! {
                            button {
                                class: "rounded px-2 py-1 text-left text-sm hover:bg-accent/50",
                                onclick: move |_| do_move.call((p.clone(), String::new(), None)),
                                "(Root)"
                            }
                        }
                    }
                    for (base, title) in folder_targets.iter().cloned() {
                        {
                            let p = path.clone();
                            let b = base.clone();
                            rsx! {
                                button {
                                    key: "{base}",
                                    class: "truncate rounded px-2 py-1 text-left text-sm hover:bg-accent/50",
                                    onclick: move |_| do_move.call((p.clone(), String::new(), Some(b.clone()))),
                                    "{title}"
                                }
                            }
                        }
                    }
                }
            }
        }
        {sidebar_body}
    };

    // ── Backlinks + verses content ────────────────────────
    let backlinks_body = rsx! {
        if let Some(vs) = verse_list.as_ref().filter(|v| !v.is_empty()) {
            div { class: "border-b border-border/60 px-3 py-3",
                Heading { level: HeadingLevel::H3, class: "mb-2", "Referenced verses" }
                div { class: "flex flex-col gap-2",
                    for (osis, human, text) in vs.clone() {
                        div { key: "{osis}", class: "rounded-md bg-background/60 p-2",
                            span { class: "text-xs font-semibold text-primary", "{human}" }
                            if let Some(t) = text {
                                p { class: "mt-0.5 text-xs leading-snug text-muted-foreground", "{t}" }
                            }
                        }
                    }
                }
            }
        }
        match &*backlinks.read_unchecked() {
            Some(Ok(list)) if list.is_empty() => rsx! {
                div { class: "px-3 py-2 text-sm text-muted-foreground",
                    "No backlinks yet. Link to this note with [[{basename_of(&current)}]]."
                }
            },
            Some(Ok(list)) => rsx! {
                nav { class: "flex flex-col gap-0.5 px-2 pb-4",
                    for path in list.iter().cloned() {
                        {
                            let (title, sha) = page_lookup
                                .read()
                                .get(&path)
                                .cloned()
                                .unwrap_or_else(|| (basename_of(&path).to_owned(), String::new()));
                            let target = FileMeta { path: path.clone(), sha256: sha };
                            rsx! {
                                button {
                                    key: "{path}",
                                    class: "group flex flex-col items-start gap-0.5 rounded px-2 py-1.5 text-left text-sm hover:bg-accent/50",
                                    onclick: move |_| on_open.call(target.clone()),
                                    span { class: "font-medium", "{title}" }
                                    span { class: "text-xs text-muted-foreground", "{path}" }
                                }
                            }
                        }
                    }
                }
            },
            Some(Err(e)) => rsx! {
                div { class: "px-2 py-2",
                    crate::states::InlineError {
                        message: e.clone(),
                        label: "Backlinks".to_string(),
                    }
                }
            },
            None => rsx! {
                div { class: "flex flex-col gap-2 px-3 py-2",
                    Skeleton { class: "h-4 w-3/4" }
                    Skeleton { class: "h-4 w-1/2" }
                    Skeleton { class: "h-4 w-2/3" }
                }
            },
        }
        div { class: "border-t border-border/60 px-3 pb-1 pt-3",
            Heading { level: HeadingLevel::H3, "Links" }
        }
        match &*outlinks.read_unchecked() {
            Some(Ok(links)) if links.is_empty() => rsx! {
                div { class: "px-3 py-2 text-sm text-muted-foreground", "No outgoing links." }
            },
            Some(Ok(links)) => rsx! {
                nav { class: "flex flex-col gap-0.5 px-2 pb-4",
                    for link in links.iter().cloned() {
                        {
                            let label = link.alias.clone().unwrap_or_else(|| link.linkpath.clone());
                            match link.resolved.clone() {
                                Some(target_path) => {
                                    let (_, sha) = page_lookup
                                        .read()
                                        .get(&target_path)
                                        .cloned()
                                        .unwrap_or_default();
                                    let target = FileMeta { path: target_path.clone(), sha256: sha };
                                    rsx! {
                                        button {
                                            key: "{link.linkpath}",
                                            class: "flex flex-col items-start gap-0.5 rounded px-2 py-1.5 text-left text-sm hover:bg-accent/50",
                                            onclick: move |_| on_open.call(target.clone()),
                                            span { class: "font-medium", "{label}" }
                                            span { class: "text-xs text-muted-foreground", "{target_path}" }
                                        }
                                    }
                                }
                                None => rsx! {
                                    div {
                                        key: "{link.linkpath}",
                                        class: "px-2 py-1.5 text-sm text-muted-foreground/70",
                                        title: "Unresolved link",
                                        "{label}"
                                    }
                                },
                            }
                        }
                    }
                }
            },
            Some(Err(e)) => rsx! {
                div { class: "px-2 py-2",
                    crate::states::InlineError {
                        message: e.clone(),
                        label: "Links".to_string(),
                    }
                }
            },
            None => rsx! {
                div { class: "flex flex-col gap-2 px-3 py-2",
                    Skeleton { class: "h-4 w-2/3" }
                    Skeleton { class: "h-4 w-1/2" }
                }
            },
        }
        if has_file {
            div { class: "mt-auto border-t border-border/60 px-3 pb-1 pt-3",
                Heading { level: HeadingLevel::H3, "Local graph" }
            }
            match (&*backlinks.read_unchecked(), &*outlinks.read_unchecked()) {
                (Some(Ok(bl)), Some(Ok(ol))) => {
                    let graph = build_local_graph(&current, bl, ol, &page_lookup.read());
                    let cur = current.clone();
                    rsx! {
                        div { class: "mx-2 mb-3 h-64 shrink-0 overflow-hidden rounded-lg border border-border/70",
                            KnowledgeGraphView {
                                graph,
                                node_scale: 0.3,
                                spacing: 1.5,
                                active: Some(current.clone()),
                                on_node_click: move |id: String| {
                                    if id == cur {
                                        return;
                                    }
                                    let (_, sha) = page_lookup.peek().get(&id).cloned().unwrap_or_default();
                                    on_open.call(FileMeta { path: id, sha256: sha });
                                },
                            }
                        }
                    }
                }
                (Some(Err(e)), _) | (_, Some(Err(e))) => rsx! {
                    div { class: "px-2 py-2",
                        crate::states::InlineError {
                            message: e.clone(),
                            label: "Graph".to_string(),
                        }
                    }
                },
                _ => rsx! {
                    div { class: "mx-2 mb-3",
                        Skeleton { class: "h-64 w-full rounded-lg" }
                    }
                },
            }
        }
    };

    // Snapshot of the panes for this render pass (the mount loop).
    let pane_list = panes.read().clone();
    let n_panes = pane_list.len();
    let focused_idx = (*focused.read()).min(n_panes.saturating_sub(1));

    rsx! {
        div { class: "flex h-full min-h-[80vh]",
            // ── Virtual-folder tree (mobile-only) ─────────
            aside {
                class: if has_file { "hidden" } else { "flex w-full flex-col overflow-y-auto pb-14 md:hidden" },
                {tree_content.clone()}
            }
            // ── Document area: panes + backlinks ──────────
            div {
                class: if has_file { "flex min-w-0 flex-1" } else { "hidden min-w-0 flex-1 md:flex" },
                // Panes container (1–2 panes side by side).
                div { class: "flex min-h-0 min-w-0 flex-1",
                    for (pi, pane) in pane_list.iter().cloned().enumerate() {
                        div {
                            key: "pane-{pi}",
                            class: if pi == 0 { "flex min-h-0 min-w-0 flex-1 flex-col" } else { "hidden min-h-0 min-w-0 flex-1 flex-col border-l border-border md:flex" },
                            onfocusin: move |_| focus_pane.call(pi),
                            // Inner note-tab bar only in SPLIT mode — a single
                            // router route can't represent two panes, so split
                            // keeps its own tab strip. In single-pane mode the
                            // top strip is the one tab UI (pane↔route synced).
                            if n_panes > 1 {
                                {render_tab_bar(pi, &pane, n_panes, focused_idx, focus_tab, close_tab, split, close_pane)}
                            }
                            if let Some(tab) = pane.tabs.get(pane.active).cloned() {
                                NoteView {
                                    key: "{pi}:{tab.path}",
                                    path: tab.path.clone(),
                                    sha: tab.sha.clone(),
                                    home: active,
                                    pane_index: pi,
                                    focused,
                                    pages: pages_memo,
                                    tag_rows,
                                    focus_tick,
                                    on_open,
                                    on_renamed,
                                }
                            } else {
                                div { class: "flex h-full items-center justify-center p-8",
                                    Text { variant: TextVariant::Muted,
                                        "Select a note from the tree to open it here."
                                    }
                                }
                            }
                        }
                    }
                }
                // ── Right sidebar (md+, focused note): Properties | Links ──
                if has_file && panel_open {
                    aside { class: "hidden w-72 shrink-0 flex-col overflow-y-auto border-l border-border bg-muted/30 md:flex",
                        // Tab header: Properties / Links + a Hide control.
                        div { class: "flex items-center gap-1 border-b border-border/60 px-2 py-1.5",
                            for (tab, label) in [
                                (RightTab::Properties, "Properties"),
                                (RightTab::Links, "Links"),
                                (RightTab::Share, "Share"),
                            ] {
                                button {
                                    key: "{label}",
                                    class: if right_tab() == tab {
                                        "rounded px-2 py-1 text-xs font-medium text-foreground bg-accent"
                                    } else {
                                        "rounded px-2 py-1 text-xs text-muted-foreground hover:text-foreground"
                                    },
                                    onclick: move |_| right_tab.set(tab),
                                    "{label}"
                                }
                            }
                            div { class: "ml-auto flex items-center gap-1.5",
                                if n_panes < MAX_PANES {
                                    button {
                                        class: "rounded px-1 text-sm text-muted-foreground hover:bg-accent hover:text-foreground",
                                        title: "Split right",
                                        onclick: move |_| split.call(()),
                                        "⇥"
                                    }
                                }
                                button {
                                    class: "text-xs text-muted-foreground hover:text-foreground",
                                    onclick: move |_| {
                                        let mut o = shell_right;
                                        o.set(crate::chrome::RightPanelOpen(false));
                                    },
                                    "Hide"
                                }
                            }
                        }
                        if right_tab() == RightTab::Properties {
                            crate::pages::note_properties::NoteProperties {}
                        } else if right_tab() == RightTab::Share {
                            crate::pages::share_panel::SharePanel { slug: active(), path: selected() }
                        } else {
                            {backlinks_body.clone()}
                        }
                    }
                }
            }
        }
        // ── Mobile chrome ─────────────────────────────────
        MobileActionBar {
            button {
                r#type: "button",
                class: "flex min-h-11 flex-1 items-center justify-center gap-2 rounded-lg border border-border px-3 py-2 text-sm font-medium text-foreground active:bg-accent",
                onclick: move |_| files_open.set(true),
                Folder { size: 16 }
                "Files"
            }
            button {
                r#type: "button",
                class: "flex min-h-11 flex-1 items-center justify-center gap-2 rounded-lg bg-primary px-3 py-2 text-sm font-medium text-primary-foreground active:bg-primary/85 disabled:opacity-50",
                disabled: !has_file,
                onclick: move |_| {
                    if let Some(cb) = status_info.peek().as_ref().and_then(|d| d.on_save) {
                        cb.call(());
                    }
                },
                if status_info.read().as_ref().is_some_and(|d| d.dirty) { "Save •" } else { "Save" }
            }
            button {
                r#type: "button",
                class: "flex min-h-11 flex-1 items-center justify-center gap-2 rounded-lg border border-border px-3 py-2 text-sm font-medium text-foreground active:bg-accent disabled:opacity-50",
                disabled: !has_file,
                onclick: move |_| {
                    let mut o = shell_right;
                    let cur = o.peek().0;
                    o.set(crate::chrome::RightPanelOpen(!cur));
                },
                "Backlinks"
            }
        }
        BottomSheet {
            open: files_open(),
            on_close: move |_| files_open.set(false),
            title: "Vault",
            {tree_content}
        }
        BottomSheet {
            open: has_file && panel_open,
            on_close: move |_| {
                let mut o = shell_right;
                o.set(crate::chrome::RightPanelOpen(false));
            },
            title: match right_tab() {
                RightTab::Properties => "Properties",
                RightTab::Links => "Links",
                RightTab::Share => "Share",
            },
            div { class: "flex items-center gap-1 border-b border-border/60 px-2 py-1.5",
                for (tab, label) in [
                                (RightTab::Properties, "Properties"),
                                (RightTab::Links, "Links"),
                                (RightTab::Share, "Share"),
                            ] {
                    button {
                        key: "{label}",
                        class: if right_tab() == tab {
                            "rounded px-2 py-1 text-xs font-medium text-foreground bg-accent"
                        } else {
                            "rounded px-2 py-1 text-xs text-muted-foreground hover:text-foreground"
                        },
                        onclick: move |_| right_tab.set(tab),
                        "{label}"
                    }
                }
            }
            if right_tab() == RightTab::Properties {
                crate::pages::note_properties::NoteProperties {}
            } else if right_tab() == RightTab::Share {
                crate::pages::share_panel::SharePanel { slug: active(), path: selected() }
            } else {
                {backlinks_body}
            }
        }
        document::Link { rel: "stylesheet", href: editor::EDITOR_STYLE }
        document::Style { {crate::collab::COLLAB_STYLE} }
    }
}

// ── shared frontmatter readers ───────────────────────────────

// The frontmatter readers (`frontmatter_value`, `front_block_maps`,
// `slugify`) and the `type: song` shape (`SongFront` + friends) live in
// `task-ui-core` so the extracted player crate can parse the same notes
// without depending on this shell. Re-exported at the old paths —
// `crate::pages::vault::SongFront` still resolves.
pub use task_ui_core::frontmatter::{
    FrontSection, FrontStem, SongFront, front_block_maps, frontmatter_value,
    setlist_song_links_from_body, setlist_songs_from, setlist_songs_from_body, slugify,
    song_front_from, song_slug_from,
};

/// Starter scaffold for a freshly-created note: an empty-but-present
/// frontmatter block so the Properties panel has something to show
/// (a `created` date + empty `tags`/`aliases` sequences). No `title`
/// key — the note's title IS its filename (see `note_header`).
// Called only from the wasm arm of `create_new_file` (the native
// client path isn't wired yet).
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn seed_note_bytes() -> Vec<u8> {
    let today = chrono::Local::now().date_naive();
    format!("---\ncreated: {today}\ntags: []\naliases: []\n---\n\n").into_bytes()
}

#[cfg(test)]
mod song_front_tests {
    use super::song_front_from;

    const NOTE: &str = "---\ntype: song\nartist: Elevation Worship\nkey: B\nbpm: 128\ntime_signature: \"4/4\"\nduration_sec: 372.5\nsections:\n  - name: Intro\n    start_sec: 0\n    end_sec: 15.2\n  - name: Verse 1\n    start_sec: 15.2\n    end_sec: 45\nstems:\n  - name: Click\n    group: Guide\n    default_muted: true\n    content_hash: aaa111\n  - name: \"Electric Guitar 1\"\n    group: Guitars\n    content_hash: bbb222\n---\n# Praise\nbody text\n";

    #[test]
    fn parses_full_song_front() {
        let f = song_front_from(NOTE);
        assert_eq!(f.artist.as_deref(), Some("Elevation Worship"));
        assert_eq!(f.key.as_deref(), Some("B"));
        assert_eq!(f.bpm, Some(128.0));
        assert_eq!(f.time_signature.as_deref(), Some("4/4"));
        assert_eq!(f.duration_sec, Some(372.5));
        assert_eq!(f.sections.len(), 2);
        assert_eq!(f.sections[1].name, "Verse 1");
        assert_eq!(f.sections[1].start_sec, 15.2);
        assert_eq!(f.stems.len(), 2);
        assert_eq!(f.stems[0].name, "Click");
        assert_eq!(f.stems[0].group.as_deref(), Some("Guide"));
        assert!(f.stems[0].default_muted);
        assert_eq!(f.stems[0].content_hash, "aaa111");
        assert_eq!(f.stems[1].name, "Electric Guitar 1");
        assert!(!f.stems[1].default_muted);
    }

    #[test]
    fn missing_front_is_empty() {
        let f = song_front_from("# Just a note\n");
        assert!(f.stems.is_empty());
        assert!(f.sections.is_empty());
        assert_eq!(f.bpm, None);
    }

    #[test]
    fn stem_without_hash_is_dropped() {
        let text = "---\nstems:\n  - name: Click\n  - name: Bass\n    content_hash: ccc\n---\n";
        let f = song_front_from(text);
        assert_eq!(f.stems.len(), 1);
        assert_eq!(f.stems[0].name, "Bass");
    }
}

#[cfg(test)]
mod setlist_wikilink_tests {
    use super::{setlist_songs_from, setlist_songs_from_body};

    #[test]
    fn body_wikilinks_define_the_setlist_in_order() {
        let note = "---\ntype: setlist\nsongs:\n  - old-entry\n---\n# Set\n\n[[Praise]]\n- [[God, I'm Just Grateful]]\n1. [[Songs/Holy Forever|HF]]\nsome prose [[Not A Row]] inline\n[[Praise]]\n";
        assert_eq!(
            setlist_songs_from_body(note),
            vec!["praise", "god-im-just-grateful", "holy-forever"]
        );
        // Body wikilinks WIN over the frontmatter list…
        assert_eq!(
            setlist_songs_from(note).first().map(|s| s.as_str()),
            Some("praise")
        );
    }

    #[test]
    fn frontmatter_fallback_still_works() {
        let note = "---\ntype: setlist\nsongs:\n  - praise\n  - washed\n---\nNo links here.\n";
        assert_eq!(setlist_songs_from(note), vec!["praise", "washed"]);
    }
}
