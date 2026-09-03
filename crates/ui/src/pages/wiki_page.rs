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
//! What stays wiki-shaped: the way back to the wiki, the provenance
//! strip (`type:`, `ai_generated`), and the backlinks — the wiki's
//! graph, not the vault's — under the note. Links inside the page
//! route back here (`WikiDocRoute`), never to the vault
//! (`crate::routes::note_route`).

use architect_ui::prelude::*;
use dioxus::prelude::*;
use vault_proto::{PageMeta, TagCount};

use crate::document_session::wiki_vault_id;
use crate::pages::note_view::NoteView;
use crate::pages::vault::{FileMeta, basename_of, fetch_backlinks, fetch_folder_index};
use crate::routes::Route;
use crate::vault_lookup;

#[component]
pub fn WikiPageView(org: String, wiki: String, path: String) -> Element {
    // The org is the route's (a wiki belongs to one org; under "All" the
    // list spans several), so every read and write here goes to it.
    let org_sig = use_signal(|| org.clone());
    let home = use_memo(move || org_sig());
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
    // the backlinks. The open note itself is live through collab.
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
    // A wikilink, a `.base` row, a backlink: another page of this wiki.
    let on_open = use_callback(move |meta: FileMeta| {
        let Some(wiki) =
            crate::document_session::wiki_of_vault_id(&vault_sig.peek()).map(str::to_owned)
        else {
            return;
        };
        nav.push(Route::WikiDocRoute {
            org: home(),
            wiki,
            path: meta.path,
        });
    });
    let on_renamed = use_callback(move |()| files.restart());

    // ── Backlinks: the wiki's graph ───────────────────────────
    let bl_path = path.clone();
    let backlinks = use_resource(move || {
        let slug = home();
        let vault = vault_sig();
        let p = bl_path.clone();
        let _refresh = refresh_key();
        async move { fetch_backlinks(slug, vault, p).await }
    });

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

    // Clear the shell status line on leave (the focused NoteView
    // writes it; nothing else here does).
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
                    Button {
                        variant: ButtonVariant::Primary,
                        size: ButtonSize::Small,
                        on_click: move |_| {
                            let slug = home();
                            let vault = vault_sig();
                            let p = create_path.clone();
                            let title = create_title.clone();
                            spawn(async move {
                                let seed = format!("---\ntitle: \"{title}\"\n---\n\n# {title}\n");
                                if create_page(slug, vault, p, seed).await.is_ok() {
                                    files.restart();
                                }
                            });
                        },
                        "Create page"
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
        div { class: "flex h-full min-h-0 w-full flex-col overflow-y-auto",
            div { class: "mx-auto flex w-full max-w-3xl flex-col gap-2 px-4 pt-4 sm:px-6 lg:px-8",
                Link {
                    to: Route::WikiHomeRoute { org: org.clone(), wiki: wiki.clone() },
                    class: "text-xs text-muted-foreground hover:text-foreground",
                    "← {wiki}"
                }
                div { class: "flex flex-wrap items-center gap-2 text-xs text-muted-foreground",
                    if ai_generated {
                        span {
                            class: "rounded-full border border-primary/40 bg-primary/10 px-2 py-0.5 font-medium text-primary",
                            title: if generated_by.is_empty() { "Machine-produced content".to_string() } else { format!("Machine-produced by {generated_by}") },
                            if generated_by.is_empty() {
                                "✨ AI generated"
                            } else {
                                "✨ AI generated · {generated_by}"
                            }
                        }
                    }
                    if !page_type.is_empty() {
                        span { class: "rounded-full border border-border/70 bg-card/60 px-2 py-0.5 font-medium uppercase tracking-wide",
                            "{page_type}"
                        }
                    }
                    span { class: "font-mono", "{path}" }
                }
            }
            div { class: "flex min-h-0 flex-1 flex-col", {body} }
            // ── Backlinks (this wiki's graph) ─────────────────
            div { class: "mx-auto w-full max-w-3xl px-4 pb-12 sm:px-6 lg:px-8",
                match &*backlinks.read_unchecked() {
                    Some(Ok(list)) if list.is_empty() => rsx! {
                        div { class: "border-t border-border/60 pt-3 text-xs text-muted-foreground",
                            "No backlinks yet. Link here with [[{basename_of(&path)}]]."
                        }
                    },
                    Some(Ok(list)) => rsx! {
                        div { class: "border-t border-border/60 pt-3",
                            Heading { level: HeadingLevel::H3, class: "mb-1 text-sm", "Backlinks" }
                            nav { class: "flex flex-col gap-0.5",
                                for bl in list.iter().cloned() {
                                    {
                                        let title = pages_memo
                                            .read()
                                            .iter()
                                            .find(|p| p.path == bl)
                                            .map(|p| p.title.clone())
                                            .unwrap_or_else(|| basename_of(&bl).to_owned());
                                        let target = FileMeta { path: bl.clone(), sha256: String::new() };
                                        rsx! {
                                            button {
                                                key: "{bl}",
                                                "data-testid": "wiki-backlink",
                                                "data-path": "{bl}",
                                                r#type: "button",
                                                class: "flex flex-col items-start gap-0.5 rounded px-2 py-1.5 text-left text-sm hover:bg-accent/50",
                                                onclick: move |_| on_open.call(target.clone()),
                                                span { class: "font-medium", "{title}" }
                                                span { class: "text-xs text-muted-foreground", "{bl}" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    },
                    Some(Err(e)) => rsx! {
                        div { class: "border-t border-border/60 pt-3",
                            crate::states::InlineError { message: e.clone(), label: "Backlinks".to_string() }
                        }
                    },
                    None => rsx! {},
                }
            }
            document::Link { rel: "stylesheet", href: editor::EDITOR_STYLE }
            document::Style { {crate::collab::COLLAB_STYLE} }
        }
    }
}

/// Start a page that a link named but nobody wrote: a create-only
/// write over the wiki's vault id, so a race with another author is
/// a visible failure rather than a silent overwrite.
async fn create_page(
    slug: String,
    vault_id: String,
    path: String,
    seed: String,
) -> Result<String, String> {
    let client = crate::vox_clients::vault_client(&slug).await?;
    client
        .put_file(
            vault_id,
            path,
            seed.into_bytes(),
            vault_proto::IfMatch::CreateOnly,
        )
        .await
        .map(|ack| ack.sha256)
        .map_err(|e| format!("put_file: {e:?}"))
}
