//! Search the text of every vault and wiki — the left drawer's Search
//! (Obsidian's search pane). Commands, pages and names are the command
//! palette's (pull down); this finds what notes *say*.
//!
//! One query fans out over the selected orgs, and in each over the
//! org's vault and every wiki it holds, through `VaultSync::search_text`
//! (a wiki is the vault `wiki:<slug>`). Results are grouped by where
//! they live; a hit opens the note in its vault or its wiki. Typing is
//! debounced, and a newer query supersedes the one in flight.

use architect_ui::lucide_dioxus::{ArrowLeft, BookOpen, FileText, Search, X};
use dioxus::prelude::*;
use vault_proto::TextHit;

use crate::document_session::{VAULT_ID, wiki_vault_id};
use crate::orgs::OrgMeta;

/// Whether the search is open. Provided once by the app shell.
#[derive(Clone, Copy)]
pub struct NotesSearchOpen(pub Signal<bool>);

pub fn provide_notes_search() {
    use_context_provider(|| NotesSearchOpen(Signal::new(false)));
}

pub fn use_notes_search() -> Signal<bool> {
    use_context::<NotesSearchOpen>().0
}

/// Hits kept per vault.
const PER_VAULT: u32 = 40;

/// One place notes live: an org's vault, or one of its wikis.
#[derive(Clone, PartialEq)]
struct Source {
    org: String,
    vault_id: String,
    /// "Notes", or the wiki's title — with the org's name when several
    /// orgs are searched.
    label: String,
    wiki: bool,
}

/// The hits of one source.
#[derive(Clone, PartialEq)]
struct Found {
    source: Source,
    hits: Vec<TextHit>,
}

/// Every source of the selected orgs.
async fn sources(orgs: Vec<OrgMeta>) -> Vec<Source> {
    let many = orgs.len() > 1;
    let mut out = Vec::new();
    for org in orgs {
        let suffix = if many {
            format!(" · {}", org.name)
        } else {
            String::new()
        };
        out.push(Source {
            org: org.slug.clone(),
            vault_id: VAULT_ID.to_owned(),
            label: format!("Notes{suffix}"),
            wiki: false,
        });
        for w in crate::feeds::fetch_wikis(&org.slug)
            .await
            .unwrap_or_default()
        {
            out.push(Source {
                org: org.slug.clone(),
                vault_id: wiki_vault_id(&w.slug),
                label: format!("{}{suffix}", w.title),
                wiki: true,
            });
        }
    }
    out
}

async fn search_one(source: Source, query: String) -> Option<Found> {
    let client = crate::vox_clients::vault_client(&source.org).await.ok()?;
    let hits = client
        .search_text(source.vault_id.clone(), query, PER_VAULT)
        .await
        .ok()?;
    (!hits.is_empty()).then_some(Found { source, hits })
}

/// The search. Mount once, in the shell; shows nothing while closed.
#[component]
pub fn NotesSearch() -> Element {
    let mut open = use_notes_search();
    if !open() {
        return rsx! {};
    }
    rsx! { SearchSheet { on_close: move |()| open.set(false) } }
}

#[component]
fn SearchSheet(on_close: EventHandler<()>) -> Element {
    let nav = use_navigator();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();
    let selection = use_context::<Signal<crate::orgs::OrgSelection>>();
    let orgs = use_memo(move || {
        let slugs = crate::orgs::selected_slugs(&selection.read(), &org_list.read());
        org_list
            .read()
            .iter()
            .filter(|o| slugs.contains(&o.slug))
            .cloned()
            .collect::<Vec<_>>()
    });
    let places = use_resource(move || sources(orgs()));
    let mut query = use_signal(String::new);

    let results = use_resource(move || {
        let q = query().trim().to_owned();
        let places = places.read().clone();
        async move {
            if q.is_empty() {
                return None;
            }
            // Debounce: a keystroke within this window supersedes us.
            architect::sleep(architect::Duration::from_millis(220)).await;
            let places = places?;
            let mut found = Vec::new();
            for s in places {
                if let Some(f) = search_one(s, q.clone()).await {
                    found.push(f);
                }
            }
            Some(found)
        }
    });

    let words: Vec<String> = query().split_whitespace().map(str::to_lowercase).collect();
    let total: usize = results
        .read()
        .as_ref()
        .and_then(|r| r.as_ref())
        .map_or(0, |r| r.iter().map(|f| f.hits.len()).sum());

    rsx! {
        div {
            class: "fixed inset-x-0 top-[var(--vv-top,0px)] z-50 flex h-[var(--vv-h,100dvh)] flex-col bg-background pt-[env(safe-area-inset-top,0px)] md:inset-0 md:h-auto md:items-center md:bg-black/40 md:p-4 md:pt-[10vh]",
            "data-testid": "notes-search",
            onclick: move |_| on_close.call(()),
            div {
                class: "flex min-h-0 w-full flex-1 flex-col md:max-w-2xl md:flex-none md:overflow-hidden md:rounded-xl md:border md:border-border md:bg-background md:shadow-xl",
                onclick: move |e| e.stop_propagation(),
                // The field.
                div { class: "flex items-center gap-2 px-3 pb-2 pt-2 md:border-b md:border-border md:p-2",
                    button {
                        r#type: "button",
                        class: "flex size-10 shrink-0 items-center justify-center rounded-full text-foreground active:bg-foreground/10 md:hidden",
                        aria_label: "Close search",
                        onclick: move |_| on_close.call(()),
                        ArrowLeft { size: 20 }
                    }
                    div { class: "flex h-11 min-w-0 flex-1 items-center gap-2 rounded-full bg-foreground/[0.06] px-4 md:rounded-lg md:bg-transparent md:px-2",
                        span { class: "text-muted-foreground", Search { size: 16 } }
                        input {
                            class: "h-full min-w-0 flex-1 bg-transparent text-[16px] outline-none placeholder:text-muted-foreground md:text-sm",
                            r#type: "search",
                            placeholder: "Search notes and wikis",
                            autocapitalize: "off",
                            value: "{query}",
                            onmounted: move |e: Event<MountedData>| {
                                spawn(async move {
                                    let _ = e.data().set_focus(true).await;
                                });
                            },
                            oninput: move |e| query.set(e.value()),
                            onkeydown: move |e| {
                                if e.key() == Key::Escape {
                                    on_close.call(());
                                }
                            },
                        }
                        if !query.read().is_empty() {
                            button {
                                r#type: "button",
                                class: "flex size-7 shrink-0 items-center justify-center rounded-full text-muted-foreground active:bg-foreground/10",
                                aria_label: "Clear",
                                onclick: move |_| query.set(String::new()),
                                X { size: 16 }
                            }
                        }
                    }
                }
                // The results.
                div { class: "min-h-0 flex-1 overflow-y-auto px-3 pb-6 md:max-h-[60vh] md:flex-none md:px-2 md:pb-2",
                    match &*results.read() {
                        Some(Some(found)) if !found.is_empty() => rsx! {
                            div { class: "px-2 pb-2 pt-1 text-[13px] text-muted-foreground",
                                if total == 1 { "1 result" } else { "{total} results" }
                            }
                            for f in found.iter().cloned() {
                                div { key: "{f.source.org}/{f.source.vault_id}", class: "mb-4",
                                    div { class: "flex items-center gap-2 px-2 pb-1.5 text-[13px] font-medium text-muted-foreground",
                                        if f.source.wiki { BookOpen { size: 14 } } else { FileText { size: 14 } }
                                        span { class: "truncate", "{f.source.label}" }
                                    }
                                    div { class: "flex flex-col divide-y divide-foreground/[0.06] overflow-hidden rounded-[18px] bg-foreground/[0.05] md:rounded-lg",
                                        for hit in f.hits.iter().cloned() {
                                            {
                                                let s = f.source.clone();
                                                let path = hit.path.clone();
                                                rsx! {
                                                    button {
                                                        key: "{hit.path}",
                                                        r#type: "button",
                                                        class: "flex flex-col gap-0.5 px-4 py-3 text-left active:bg-foreground/[0.06] md:py-2 md:hover:bg-accent",
                                                        onclick: move |_| {
                                                            nav.push(crate::routes::note_route(&s.org, &s.vault_id, path.clone()));
                                                            on_close.call(());
                                                        },
                                                        span { class: "truncate text-[16px] font-medium text-foreground md:text-sm", "{hit.title}" }
                                                        if hit.line > 0 {
                                                            span { class: "line-clamp-2 text-[14px] text-muted-foreground md:text-xs",
                                                                {highlight(&hit.excerpt, &words)}
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        },
                        Some(Some(_)) => rsx! {
                            div { class: "px-2 py-8 text-center text-[15px] text-muted-foreground", "No notes say that." }
                        },
                        _ if query.read().trim().is_empty() => rsx! {
                            div { class: "px-2 py-8 text-center text-[15px] text-muted-foreground",
                                "Search what your notes and wikis say."
                            }
                        },
                        _ => rsx! {
                            div { class: "px-2 py-8 text-center text-[15px] text-muted-foreground", "Searching…" }
                        },
                    }
                }
            }
        }
    }
}

/// `text` with every occurrence of `words` marked.
fn highlight(text: &str, words: &[String]) -> Element {
    let spans = mark_spans(text, words);
    rsx! {
        for (i, (part, hit)) in spans.into_iter().enumerate() {
            if hit {
                mark { key: "{i}", class: "rounded-[3px] bg-primary/25 text-foreground", "{part}" }
            } else {
                span { key: "{i}", "{part}" }
            }
        }
    }
}

/// Split `text` into runs, each marked or not, by case-insensitive
/// occurrences of any of `words` (ASCII case folding, so byte offsets
/// stay valid).
fn mark_spans(text: &str, words: &[String]) -> Vec<(String, bool)> {
    let lc = text.to_ascii_lowercase();
    let mut marked = vec![false; text.len()];
    for w in words.iter().filter(|w| !w.is_empty()) {
        let w = w.to_ascii_lowercase();
        let mut from = 0;
        while let Some(i) = lc[from..].find(&w) {
            let at = from + i;
            for m in &mut marked[at..at + w.len()] {
                *m = true;
            }
            from = at + w.len();
        }
    }
    let mut out: Vec<(String, bool)> = Vec::new();
    for (i, c) in text.char_indices() {
        let hit = marked[i];
        match out.last_mut() {
            Some((s, h)) if *h == hit => s.push(c),
            _ => out.push((c.to_string(), hit)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::mark_spans;

    #[test]
    fn every_occurrence_of_every_word_is_marked() {
        let spans = mark_spans("Mix the Mix, then master", &["mix".into(), "master".into()]);
        let marked: Vec<_> = spans
            .iter()
            .filter(|(_, h)| *h)
            .map(|(s, _)| s.as_str())
            .collect();
        assert_eq!(marked, ["Mix", "Mix", "master"]);
        assert_eq!(
            spans.iter().map(|(s, _)| s.as_str()).collect::<String>(),
            "Mix the Mix, then master"
        );
    }
}
