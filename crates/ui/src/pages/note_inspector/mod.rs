//! The note inspector — the right sidebar beside an open note:
//! **Properties** (its frontmatter, edited live), **Links** (backlinks,
//! outgoing links, referenced verses), **Graph** (the local graph
//! around it) and **Share** (its share links).
//!
//! One component, keyed by vault id. A wiki page is a vault note that
//! happens to live in a wiki (`wiki:<slug>` beside the org's own
//! `default`), so the vault page and the wiki page — and the wiki
//! home, centred on the wiki's index — mount this same inspector with
//! different ids, and every panel works over whichever vault it was
//! given: the graph RPCs, the share target and the properties editor
//! all take the vault id, not a page. Improving a panel here improves
//! both routes; there is no wiki-shaped copy.
//!
//! The inspector fetches for the tab that is showing and nothing
//! else, and the [`Local graph`](local_graph) tab's renderer sits
//! behind a wasm-split boundary, so the sidebar costs nothing until
//! somebody opens it.
//!
//! What it does *not* own: the tab state (the page holds it, so the
//! desktop aside and the mobile sheet show the same tab) and where a
//! row click goes (`on_open` — the vault opens a tab in its focused
//! pane, the wiki navigates to the page's route).

pub(crate) mod local_graph;

use std::collections::HashMap;

use architect_ui::prelude::*;
use dioxus::prelude::*;
use vault_proto::PageMeta;

use crate::pages::vault::{FileMeta, basename_of, fetch_backlinks, fetch_links};
// The lazy loader names its entry point unqualified.
use local_graph::local_graph_surface;

/// The inspector's tabs, in strip order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum InspectorTab {
    /// The focused note's frontmatter, edited live — first because it
    /// is the one used most while writing.
    #[default]
    Properties,
    /// Backlinks + outgoing links (+ referenced verses).
    Links,
    /// The local graph around the note.
    Graph,
    /// Share links for the note.
    Share,
}

impl InspectorTab {
    pub const ALL: [InspectorTab; 4] = [
        InspectorTab::Properties,
        InspectorTab::Links,
        InspectorTab::Graph,
        InspectorTab::Share,
    ];

    pub fn label(self) -> &'static str {
        match self {
            InspectorTab::Properties => "Properties",
            InspectorTab::Links => "Links",
            InspectorTab::Graph => "Graph",
            InspectorTab::Share => "Share",
        }
    }
}

/// The inspector: tab strip + the showing tab's body. See the module
/// docs for what the props mean; `on_hide` renders the strip's Hide
/// control when given (the desktop aside), `extra` goes in the strip
/// beside it (the vault's split button).
#[component]
pub fn NoteInspector(
    /// The org slug every RPC goes to.
    org: ReadSignal<String>,
    /// The vault the note lives in: [`crate::document_session::VAULT_ID`]
    /// or a wiki's `wiki:<slug>`.
    vault_id: String,
    /// The focused note (vault-relative path); `None` shows the
    /// nothing-open states.
    path: ReadSignal<Option<String>>,
    /// Bumped by the page after a save or a live change; the link and
    /// graph tabs re-fetch on it.
    refresh_key: ReadSignal<u64>,
    /// The vault's folder index — titles and shas for the rows.
    pages: ReadSignal<Vec<PageMeta>>,
    /// Where a row or node click goes.
    on_open: Callback<FileMeta>,
    /// Which tab shows; owned by the page.
    tab: Signal<InspectorTab>,
    #[props(default)] on_hide: Option<Callback<()>>,
    #[props(default)] extra: Option<Element>,
) -> Element {
    let showing = tab();
    rsx! {
        div { class: "flex items-center gap-1 border-b border-border/60 px-2 py-1.5",
            for t in InspectorTab::ALL {
                button {
                    key: "{t.label()}",
                    r#type: "button",
                    "data-testid": "inspector-tab-{t.label()}",
                    class: if showing == t {
                        "rounded px-2 py-1 text-xs font-medium text-foreground bg-accent"
                    } else {
                        "rounded px-2 py-1 text-xs text-muted-foreground hover:text-foreground"
                    },
                    onclick: move |_| tab.set(t),
                    "{t.label()}"
                }
            }
            div { class: "ml-auto flex items-center gap-1.5",
                {extra}
                if let Some(hide) = on_hide {
                    button {
                        r#type: "button",
                        class: "text-xs text-muted-foreground hover:text-foreground",
                        onclick: move |_| hide.call(()),
                        "Hide"
                    }
                }
            }
        }
        match showing {
            InspectorTab::Properties => rsx! { crate::pages::note_properties::NoteProperties {} },
            InspectorTab::Share => rsx! {
                crate::pages::share_panel::SharePanel {
                    slug: org(),
                    vault_id: vault_id.clone(),
                    path: path(),
                }
            },
            InspectorTab::Links => rsx! {
                LinksPanel { org, vault: vault_id.clone(), path, refresh_key, pages, on_open }
            },
            InspectorTab::Graph => match path() {
                Some(p) => {
                    let args = local_graph::LocalGraphArgs {
                        org: org(),
                        vault_id: vault_id.clone(),
                        path: p,
                        refresh: refresh_key(),
                        pages,
                        on_open,
                    };
                    // The layout engine + SVG renderer download the
                    // first time this tab opens (see `local_graph`).
                    task_plugin_ui::lazy_element_with!(
                        "local_graph",
                        local_graph_surface,
                        local_graph::LocalGraphArgs,
                        args
                    )
                }
                None => rsx! {
                    div { class: "px-3 py-4 text-sm text-muted-foreground", "Open a note to see its local graph." }
                },
            },
        }
    }
}

/// The Links tab: referenced verses, backlinks, outgoing links — each
/// row opens through `on_open`; an unresolved link is listed but has
/// nothing to open.
#[component]
fn LinksPanel(
    org: ReadSignal<String>,
    vault: ReadSignal<String>,
    path: ReadSignal<Option<String>>,
    refresh_key: ReadSignal<u64>,
    pages: ReadSignal<Vec<PageMeta>>,
    on_open: Callback<FileMeta>,
) -> Element {
    // path → (title, sha) for the rows.
    let page_lookup = use_memo(move || {
        pages
            .read()
            .iter()
            .map(|p| (p.path.clone(), (p.title.clone(), p.sha256.clone())))
            .collect::<HashMap<String, (String, String)>>()
    });

    let backlinks = use_resource(move || {
        let slug = org();
        let vault = vault();
        let path = path();
        let _refresh = refresh_key();
        async move {
            match path {
                Some(p) => fetch_backlinks(slug, vault, p).await,
                None => Ok(Vec::new()),
            }
        }
    });
    let outlinks = use_resource(move || {
        let slug = org();
        let vault = vault();
        let path = path();
        let _refresh = refresh_key();
        async move {
            match path {
                Some(p) => fetch_links(slug, vault, p).await,
                None => Ok(Vec::new()),
            }
        }
    });
    // Verses the note references (from synced note→verse links), with
    // their text — the inline scripture reader.
    let verses = use_resource(move || {
        let slug = org();
        let path = path();
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

    let current = path().unwrap_or_default();
    let verse_list = verses.read().clone();

    rsx! {
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
        div { class: "px-3 pb-1 pt-3",
            Heading { level: HeadingLevel::H3, "Backlinks" }
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
                                    r#type: "button",
                                    "data-testid": "inspector-backlink",
                                    "data-path": "{path}",
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
                                            r#type: "button",
                                            "data-testid": "inspector-link",
                                            "data-path": "{target_path}",
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
    }
}

/// OSIS verse id → a human reference the scripture service parses
/// (`John.3.16` → `John 3:16`; a range keeps its start). Best-effort.
pub(crate) fn osis_to_ref(osis: &str) -> String {
    let first = osis.split('-').next().unwrap_or(osis);
    let mut it = first.rsplitn(3, '.');
    match (it.next(), it.next(), it.next()) {
        (Some(v), Some(c), Some(b)) => format!("{b} {c}:{v}"),
        _ => first.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::osis_to_ref;

    #[test]
    fn osis_becomes_a_human_reference() {
        assert_eq!(osis_to_ref("John.3.16"), "John 3:16");
        assert_eq!(osis_to_ref("John.3.16-John.3.17"), "John 3:16");
        assert_eq!(osis_to_ref("Gen"), "Gen");
    }
}
