//! `/wiki/w/:org/:wiki/page?:path` — one page of one wiki, in the
//! vault editor.
//!
//! A wiki page is a vault note that happens to live in a wiki: the
//! server serves every wiki root as the vault `wiki:<slug>` beside the
//! org's own `default`, so this page mounts the same
//! [`NoteView`](crate::pages::note_view::NoteView) the vault page
//! does — the `DocumentSession` (open / autosave / conflict), per-file
//! CRDT collab with presence, wikilinks resolving through the wiki's
//! own folder index, `[[`/`#` completion, the `type:` dispatch — over
//! that vault id. There is no second editor: the textarea this file
//! used to carry, and the `read_page`/`write_page` pair behind it, are
//! gone; the wiki `Pages` service still serves the CLI and MCP, and
//! writes the same files.
//!
//! The same goes for the right sidebar: the
//! [`NoteInspector`](crate::pages::note_inspector) the vault page
//! mounts — Properties, Links, the local graph, Share — mounted here
//! over the wiki's vault id, so every panel reads the wiki's graph
//! and a share link minted here targets the wiki page. Nothing in it
//! is wiki-shaped; only where a row click goes is this page's call
//! (the page's route, through [`crate::routes::note_route`]).
//!
//! What stays wiki-shaped: the way back to the wiki and the provenance
//! strip (`type:`, `ai_generated`). Links inside the page route back
//! here (`WikiDocRoute`), never to the vault.

use architect_ui::prelude::*;
use dioxus::prelude::*;
use vault_proto::{PageMeta, TagCount};

use crate::document_session::wiki_vault_id;
use crate::pages::note_inspector::{InspectorTab, NoteInspector};
use crate::pages::note_view::NoteView;
use crate::pages::vault::{FileMeta, basename_of, fetch_folder_index};
use crate::routes::Route;
use crate::shell::mobile::{BottomSheet, MobileActionBar};
use crate::vault_lookup;

#[component]
pub fn WikiPageView(org: String, wiki: String, path: ReadSignal<String>) -> Element {
    // The page path is a signal (the route re-renders this component
    // with a new one when a wikilink is followed): the inspector
    // follows it as a memo, the rest of the page reads it once.
    let page_path = use_memo(move || Some(path()));
    let path = path();
    // The org is the route's (a wiki belongs to one org; under "All" the
    // list spans several), so every read and write here goes to it.
    // Reactive on the prop: the route re-renders this component in
    // place when a wikilink is followed, so a value captured once at
    // mount would go stale (and an empty one breaks every route built
    // from it).
    let home = {
        let org = org.clone();
        use_memo(use_reactive!(|org| org))
    };
    let vault_id = wiki_vault_id(&wiki);
    let vault_sig = use_signal(|| vault_id.clone());
    let nav = use_navigator();

    // ── The wiki's folder index ───────────────────────────────
    // Wikilink candidates, cross-file lookup, the `type:` dispatch,
    // and — the reason the editor waits for it — the page's sha, the
    // base of its first conditional write.
    let mut files = use_resource(move || {
        let slug = home();
        let vault = vault_sig();
        async move { fetch_folder_index(slug, vault).await }
    });
    let pages_memo = use_memo(move || match &*files.read_unchecked() {
        Some(Ok(pages)) => pages.clone(),
        _ => Vec::new(),
    });

    // ── Live changes ──────────────────────────────────────────
    // The `VaultSync` stream carries every vault id; keep this wiki's.
    // A save here, another client's, or a write through the wiki
    // pipeline re-pulls the index (a rename, a new page) and refreshes
    // the inspector. The open note itself is live through collab.
    let vault_tick = use_signal(|| 0u64);
    let focus_tick = use_signal(|| 0u64);
    let refresh_key = use_memo(move || *focus_tick.read() + *vault_tick.read());
    architect::use_stream(
        move |tx| {
            let slug = home();
            async move {
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
            if change.vault_id != *vault_sig.peek() {
                return;
            }
            let path = match &change.event {
                vault_proto::VaultEvent::Put { path, .. }
                | vault_proto::VaultEvent::Delete { path } => path.as_str(),
                vault_proto::VaultEvent::Resync => {
                    files.restart();
                    vault_tick += 1;
                    return;
                }
            };
            if std::path::Path::new(path)
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("md"))
            {
                files.restart();
                vault_tick += 1;
            }
        },
    );

    // ── What NoteView needs from its page ─────────────────────
    // The focused note's live doc for the Properties sidebar, and the
    // scope that owns editor buffers (see `DocOwnerScope`).
    use_context_provider(|| Signal::new(None::<crate::pages::note_properties::FocusedDoc>));
    use_context_provider(|| {
        crate::document_session::DocOwnerScope(dioxus::core::current_scope_id())
    });
    let focused = use_signal(|| 0usize);
    let mut tag_rows = use_signal(Vec::<TagCount>::new);
    use_effect(move || {
        let slug = home();
        let vault = vault_sig();
        let _refresh = refresh_key();
        spawn(async move {
            if let Ok(tags) = vault_lookup::tag_candidates(slug, vault).await {
                tag_rows.set(tags);
            }
        });
    });
    // A wikilink, a `.base` row, a backlink, a graph node: another
    // page of this wiki — its route, by the vault id.
    let on_open = use_callback(move |meta: FileMeta| {
        nav.push(crate::routes::note_route(
            &home(),
            &vault_sig.peek(),
            meta.path,
        ));
    });
    let on_renamed = use_callback(move |()| files.restart());

    // ── Who may change it ─────────────────────────────────────
    // A wiki governed by its Editors opens read-only for everyone
    // else, with the way in (an Edit Request) above the page.
    let access = crate::pages::wiki_access::use_wiki_access(home, wiki.clone());
    let proposing = use_signal(|| false);
    let write_mode = use_memo(move || {
        crate::pages::wiki_access::write_mode(access.read().as_ref(), proposing())
    });
    let session_out = use_signal(|| None::<crate::document_session::DocumentSession>);
    let can_create = access.read().as_ref().is_some_and(|a| a.can_edit);
    let mut create_error = use_signal(|| None::<String>);

    // ── Move / Delete ─────────────────────────────────────────
    let mut page_action = use_signal(|| None::<PageAction>);
    let mut action_error = use_signal(|| None::<String>);
    let mut move_folder = use_signal(String::new);
    let mut move_new_folder = use_signal(String::new);
    let mut move_name = use_signal(String::new);
    let mut menu_open = use_signal(|| false);
    let wiki_title = access
        .read()
        .as_ref()
        .map_or_else(|| wiki.clone(), |a| a.title.clone());
    let folders = use_memo(move || {
        let mut out: Vec<String> = pages_memo
            .read()
            .iter()
            .filter_map(|p| p.path.rsplit_once('/').map(|(dir, _)| dir.to_owned()))
            .collect();
        out.sort();
        out.dedup();
        out
    });

    // ── The inspector ─────────────────────────────────────────
    // Open state is the shell's (the top-bar toggle), the tab is
    // this page's — the desktop aside and the mobile sheet share it.
    let shell_right = use_context::<Signal<crate::chrome::RightPanelOpen>>();
    let panel_open = shell_right.read().0;
    let right_tab = use_signal(InspectorTab::default);

    // ── Provenance strip ──────────────────────────────────────
    // `ai_generated` / `generated_by` are the wiki's own frontmatter
    // reading — the wiki `Pages` list carries them, the vault index
    // does not.
    let prov_wiki = wiki.clone();
    let provenance = use_resource(move || {
        let slug = home();
        let wiki = prov_wiki.clone();
        let _refresh = refresh_key();
        async move {
            crate::feeds::fetch_wiki_pages_of(&slug, &wiki)
                .await
                .unwrap_or_default()
        }
    });

    // The status line (the focused NoteView writes it; the mobile
    // action bar's Save reads it). Cleared on leave.
    let status_info = use_context::<crate::chrome::StatusBarInfo>().0;
    use_drop(move || {
        let mut info = status_info;
        info.set(None);
    });

    let meta: Option<PageMeta> = pages_memo.read().iter().find(|p| p.path == path).cloned();
    let page_type = meta
        .as_ref()
        .map(|m| m.page_type.clone())
        .unwrap_or_default();
    let (ai_generated, generated_by) = provenance
        .read()
        .as_ref()
        .and_then(|list| list.iter().find(|p| p.path == path))
        .map(|p| (p.ai_generated, p.generated_by.clone()))
        .unwrap_or_default();
    let has_page = meta.is_some();

    let body = match (&*files.read_unchecked(), meta) {
        (Some(Ok(_)), Some(meta)) => rsx! {
            NoteView {
                key: "{meta.path}",
                path: meta.path.clone(),
                sha: meta.sha256.clone(),
                home,
                vault_id: vault_id.clone(),
                pane_index: 0,
                focused,
                pages: pages_memo,
                tag_rows,
                focus_tick,
                on_open,
                on_renamed,
                write_mode: Some(write_mode.into()),
                session_out,
            }
        },
        (Some(Ok(_)), None) => {
            // Not a page of this wiki (yet). A link to a page nobody has
            // written is how a wiki grows; offer to start it.
            let create_path = path.clone();
            let create_title = basename_of(&path).to_owned();
            rsx! {
                div { class: "flex flex-col items-start gap-3 rounded-xl border border-border/70 bg-card/30 p-6",
                    Heading { level: HeadingLevel::H3, "{create_title}" }
                    Text { variant: TextVariant::Muted, "This page doesn't exist yet." }
                    if !can_create {
                        Text { variant: TextVariant::Muted,
                            "Only this wiki's Editors can start new pages."
                        }
                    } else {
                    Button {
                        variant: ButtonVariant::Primary,
                        size: ButtonSize::Small,
                        on_click: move |_| {
                            let slug = home();
                            let vault = vault_sig();
                            let p = create_path.clone();
                            let title = create_title.clone();
                            spawn(async move {
                                match create_page(slug, vault, p, page_seed(&title)).await {
                                    Ok(_) => files.restart(),
                                    Err(e) => create_error.set(Some(e)),
                                }
                            });
                        },
                        "Create page"
                    }
                    if let Some(e) = create_error() {
                        span { class: "text-sm text-destructive", "{e}" }
                    }
                    }
                }
            }
        }
        (Some(Err(e)), _) => rsx! {
            crate::states::ErrorState {
                title: "Couldn't load this wiki",
                message: e.clone(),
            }
        },
        (None, _) => rsx! {
            div { class: "flex items-center justify-center rounded-xl border border-border/70 bg-card/30 py-16",
                Text { variant: TextVariant::Muted, "Loading page…" }
            }
        },
    };

    rsx! {
        div { class: "flex h-full min-h-0 w-full",
            // `wiki-reading`: a wiki page is set for reading — see
            // `crate::reading`.
            div { class: "wiki-reading flex h-full min-h-0 min-w-0 flex-1 flex-col overflow-y-auto",
                div { class: "note-column mx-auto flex w-full max-w-3xl flex-col gap-2 px-6 pt-5",
                    // One quiet line: where this page belongs, what it is,
                    // who wrote it — and its actions behind ⋯. The path is
                    // in the tab and the sidebar already.
                    div { class: "relative flex flex-wrap items-center gap-x-2 gap-y-1 text-xs text-muted-foreground",
                        Link {
                            to: Route::WikiHomeRoute { org: org.clone(), wiki: wiki.clone() },
                            class: "hover:text-foreground",
                            "← {wiki_title}"
                        }
                        if !page_type.is_empty() {
                            span { class: "text-muted-foreground/60", "·" }
                            span { class: "capitalize", "{page_type}" }
                        }
                        if ai_generated {
                            span { class: "text-muted-foreground/60", "·" }
                            span {
                                title: if generated_by.is_empty() { "Machine-produced content".to_string() } else { format!("Machine-produced by {generated_by}") },
                                "✨ AI draft"
                            }
                        }
                        if has_page && can_create {
                            button {
                                r#type: "button",
                                class: "page-menu-button ml-auto rounded px-1.5 text-base leading-none hover:bg-accent hover:text-foreground",
                                title: "Page actions",
                                onclick: move |_| {
                                    let open = *menu_open.peek();
                                    menu_open.set(!open);
                                },
                                "⋯"
                            }
                        }
                        if has_page && can_create && menu_open() {
                            // A tap anywhere else closes the menu — on a
                            // phone there is no pointer to move away.
                            div {
                                class: "fixed inset-0 z-10",
                                onclick: move |_| menu_open.set(false),
                            }
                            span { class: "page-menu absolute right-0 top-6 z-20 flex min-w-40 flex-col rounded-lg border border-border bg-popover p-1 text-sm text-foreground shadow-lg",
                                button {
                                    r#type: "button",
                                    class: "rounded px-2 py-1 text-left hover:bg-accent",
                                    onclick: {
                                        let here = path
                                            .rsplit_once('/')
                                            .map(|(dir, _)| dir.to_owned())
                                            .unwrap_or_default();
                                        let name = basename_of(&path).to_owned();
                                        move |_| {
                                            action_error.set(None);
                                            // Start from where the page is and
                                            // what it is called, not from the
                                            // last move's answers.
                                            move_folder.set(here.clone());
                                            move_new_folder.set(String::new());
                                            move_name.set(name.clone());
                                            menu_open.set(false);
                                            page_action.set(Some(PageAction::Move));
                                        }
                                    },
                                    "Move / rename…"
                                }
                                button {
                                    r#type: "button",
                                    class: "rounded px-2 py-1 text-left text-destructive hover:bg-destructive/10",
                                    onclick: move |_| {
                                        action_error.set(None);
                                        menu_open.set(false);
                                        page_action.set(Some(PageAction::Delete));
                                    },
                                    "Delete…"
                                }
                            }
                        }
                    }
                    if let Some(act) = page_action() {
                        {
                            let from = path.clone();
                            let (org_c, wiki_c) = (org.clone(), wiki.clone());
                            let run = move |_| {
                                let slug = home();
                                let vault = vault_sig();
                                let from = from.clone();
                                let draft = session_out.peek().as_ref().and_then(|s| s.draft());
                                let org_nav = org_c.clone();
                                let wiki_nav = wiki_c.clone();
                                match act {
                                    PageAction::Move => {
                                        let typed = move_new_folder.peek().trim().to_owned();
                                        let folder = if typed.is_empty() { move_folder.peek().clone() } else { typed };
                                        let typed_name = move_name.peek().trim().to_owned();
                                        let name = if typed_name.is_empty() {
                                            basename_of(&from).to_owned()
                                        } else {
                                            typed_name
                                        };
                                        let Some(to) = new_page_path(&folder, &name) else { return };
                                        if to == from {
                                            page_action.set(None);
                                            return;
                                        }
                                        let buffer = draft.map(|d| d.text.into_bytes());
                                        spawn(async move {
                                            match crate::pages::page_actions::move_page(slug, vault, from, to.clone(), buffer).await {
                                                Ok(_) => {
                                                    page_action.set(None);
                                                    nav.push(Route::WikiDocRoute { org: org_nav, wiki: wiki_nav, path: to });
                                                }
                                                Err(e) => action_error.set(Some(e)),
                                            }
                                        });
                                    }
                                    PageAction::Delete => {
                                        let sha = draft.map(|d| d.base_sha256).filter(|s| !s.is_empty());
                                        spawn(async move {
                                            match crate::pages::page_actions::delete_page(slug, vault, from, sha).await {
                                                Ok(()) => {
                                                    page_action.set(None);
                                                    nav.push(Route::WikiHomeRoute { org: org_nav, wiki: wiki_nav });
                                                }
                                                Err(e) => action_error.set(Some(e)),
                                            }
                                        });
                                    }
                                }
                            };
                            rsx! {
                                div { class: "flex flex-wrap items-center gap-2 rounded-lg border border-border/70 bg-card/40 px-3 py-2 text-sm",
                                    "data-testid": "page-action",
                                    match act {
                                        PageAction::Move => rsx! {
                                            input {
                                                class: "min-w-0 basis-full rounded-md border border-border/70 bg-background px-2 py-1 sm:basis-auto sm:flex-1",
                                                placeholder: "Page name",
                                                title: "The page's name — renaming updates the links that point to it",
                                                value: "{move_name}",
                                                oninput: move |e| move_name.set(e.value()),
                                            }
                                            span { class: "text-muted-foreground", "in" }
                                            select {
                                                class: "rounded-md border border-border/70 bg-background px-2 py-1",
                                                value: "{move_folder}",
                                                onchange: move |e| move_folder.set(e.value()),
                                                option { value: "", "(top level)" }
                                                for f in folders.read().iter() {
                                                    option { key: "{f}", value: "{f}", "{f}" }
                                                }
                                            }
                                            input {
                                                class: "min-w-0 flex-1 rounded-md border border-border/70 bg-background px-2 py-1",
                                                placeholder: "or a new folder",
                                                value: "{move_new_folder}",
                                                oninput: move |e| move_new_folder.set(e.value()),
                                            }
                                            Button { variant: ButtonVariant::Primary, size: ButtonSize::Small, on_click: run, "Save" }
                                        },
                                        PageAction::Delete => rsx! {
                                            span { class: "flex-1",
                                                "Delete this page? Links to it will show as missing."
                                            }
                                            Button { variant: ButtonVariant::Destructive, size: ButtonSize::Small, on_click: run, "Delete" }
                                        },
                                    }
                                    Button {
                                        variant: ButtonVariant::Ghost,
                                        size: ButtonSize::Small,
                                        on_click: move |_| page_action.set(None),
                                        "Cancel"
                                    }
                                    if let Some(e) = action_error() {
                                        span { class: "basis-full text-destructive", "{e}" }
                                    }
                                }
                            }
                        }
                    }
                }
                if let Some(a) = access.read().clone().filter(|a| has_page && !a.can_edit) {
                    div { class: "px-4 sm:px-6 lg:px-8",
                        crate::pages::wiki_access::ProposeBar {
                            org: home,
                            wiki: wiki.clone(),
                            access: a,
                            proposing,
                            session: session_out,
                        }
                    }
                }
                div { class: "flex min-h-0 flex-1 flex-col pb-12", {body} }
                document::Link { rel: "stylesheet", href: editor::EDITOR_STYLE }
                document::Style { {crate::reading::reading_style()} }
                document::Style { {crate::collab::COLLAB_STYLE} }
            }
            // ── Right sidebar (md+): the same inspector as the vault ──
            if has_page && panel_open {
                aside {
                    class: "hidden w-72 shrink-0 flex-col overflow-y-auto border-l border-border bg-muted/30 md:flex",
                    "data-testid": "note-inspector",
                    NoteInspector {
                        org: home,
                        vault_id: vault_id.clone(),
                        path: page_path,
                        refresh_key,
                        pages: pages_memo,
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
        // ── Mobile chrome: Save + the inspector as a sheet ────
        MobileActionBar {
            button {
                r#type: "button",
                class: "flex min-h-11 flex-1 items-center justify-center gap-2 rounded-lg bg-primary px-3 py-2 text-sm font-medium text-primary-foreground active:bg-primary/85 disabled:opacity-50",
                disabled: !has_page,
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
                disabled: !has_page,
                onclick: move |_| {
                    let mut o = shell_right;
                    let cur = o.peek().0;
                    o.set(crate::chrome::RightPanelOpen(!cur));
                },
                "Backlinks"
            }
        }
        BottomSheet {
            open: has_page && panel_open,
            on_close: move |_| {
                let mut o = shell_right;
                o.set(crate::chrome::RightPanelOpen(false));
            },
            title: right_tab().label().to_string(),
            NoteInspector {
                org: home,
                vault_id: vault_id.clone(),
                path: page_path,
                refresh_key,
                pages: pages_memo,
                on_open,
                tab: right_tab,
            }
        }
    }
}

/// What the page strip is asking about.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PageAction {
    Move,
    Delete,
}

/// Start a page that a link named but nobody wrote: a create-only
/// write over the wiki's vault id, so a race with another author is
/// a visible failure rather than a silent overwrite.
///
/// Shared with the wiki home's "Add page". The error is a sentence for
/// the person who asked: a page already at that path, or the server's
/// reason for refusing (a wiki governed by its Editors).
pub(crate) async fn create_page(
    slug: String,
    vault_id: String,
    path: String,
    seed: String,
) -> Result<String, String> {
    let client = crate::vox_clients::vault_client(&slug).await?;
    match client
        .put_file(
            vault_id,
            path.clone(),
            seed.into_bytes(),
            vault_proto::IfMatch::CreateOnly,
        )
        .await
    {
        Ok(ack) => Ok(ack.sha256),
        Err(vox::VoxError::User(e)) => Err(match *e {
            vault_proto::VaultSyncError::Conflict { .. } => {
                format!("There's already a page at {path}.")
            }
            vault_proto::VaultSyncError::Refused(reason) => reason,
            vault_proto::VaultSyncError::BadPath => format!("“{path}” isn't a usable page name."),
            other => format!("Couldn't create it: {other}"),
        }),
        Err(e) => Err(format!("Couldn't create it: {e:?}")),
    }
}

/// A seeded page for `title`: frontmatter naming it, and its heading.
#[must_use]
pub(crate) fn page_seed(title: &str) -> String {
    let quoted = title.replace('"', "\\\"");
    format!("---\ntitle: \"{quoted}\"\n---\n\n# {title}\n\n")
}

/// Where a new page titled `title` goes in `folder` (wiki-relative, ""
/// for the root). `None` when nothing usable is left of the title.
#[must_use]
pub(crate) fn new_page_path(folder: &str, title: &str) -> Option<String> {
    let name: String = title
        .trim()
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            c => c,
        })
        .collect();
    let name = name.trim_matches(['.', ' ', '-']);
    if name.is_empty() {
        return None;
    }
    let folder = folder.trim_matches('/');
    Some(if folder.is_empty() {
        format!("{name}.md")
    } else {
        format!("{folder}/{name}.md")
    })
}

#[cfg(test)]
mod tests {
    use super::new_page_path;

    #[test]
    fn a_title_becomes_a_file_in_its_folder() {
        assert_eq!(new_page_path("", "Dorian"), Some("Dorian.md".into()));
        assert_eq!(
            new_page_path("Concepts", "Dorian"),
            Some("Concepts/Dorian.md".into())
        );
        assert_eq!(
            new_page_path("/Concepts/", "Is it A/B?"),
            Some("Concepts/Is it A-B.md".into())
        );
        assert_eq!(new_page_path("Concepts", "  ...  "), None);
    }
}
