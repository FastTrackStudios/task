//! Study paths: an ordered way into a wiki.
//!
//! A wiki with thirty pages on a hard subject needs a place to start and
//! an order to read in. A *study path* is a page of `type: path` whose
//! body lists the steps — each a list item that opens with a link:
//!
//! ```markdown
//! 1. [[Psalm 82]] — the trial in heaven the rest explains
//! 2. [[Elohim]] — the word that makes it strange
//! ```
//!
//! Every page on a path gets a footer — which path, which step, the one
//! before and after — and the path page itself shows how far you are.
//! "Read" is per browser: opening a page marks it.

use std::collections::BTreeSet;

use dioxus::prelude::*;
use vault_proto::PageMeta;

use crate::routes::Route;

/// One step of a path: the page it opens and the note beside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    /// The link's target, as written (`Psalm 82`).
    pub target: String,
    /// The text after the link (`the trial in heaven …`), dash stripped.
    pub note: String,
}

/// The steps of a path page's markdown: list items (numbered or not)
/// that open with a `[[link]]`. Anything else on the page is prose.
#[must_use]
pub fn steps_of(markdown: &str) -> Vec<Step> {
    markdown
        .lines()
        .filter_map(|line| {
            let item = line.trim_start();
            let item = item
                .strip_prefix("- ")
                .or_else(|| item.strip_prefix("* "))
                .or_else(|| {
                    let digits = item.find(|c: char| !c.is_ascii_digit())?;
                    (digits > 0).then(|| item[digits..].strip_prefix(". "))?
                })?
                .trim_start();
            let rest = item.strip_prefix("[[")?;
            let (inner, after) = rest.split_once("]]")?;
            let target = inner.split(['|', '#']).next()?.trim().to_owned();
            let note = after
                .trim()
                .trim_start_matches(['—', '–', '-', ':'])
                .trim()
                .to_owned();
            (!target.is_empty()).then_some(Step { target, note })
        })
        .collect()
}

/// A path with its steps resolved to pages.
#[derive(Clone, Debug, PartialEq)]
struct Path {
    path: String,
    title: String,
    /// `(step, page path)`; `None` for a step whose page is missing.
    steps: Vec<(Step, Option<String>)>,
}

fn resolve(pages: &[PageMeta], target: &str) -> Option<String> {
    let t = target.to_lowercase();
    pages
        .iter()
        .filter(|p| !p.path.starts_with("raw/"))
        .find(|p| p.basename.to_lowercase() == t || p.aliases.iter().any(|a| a.to_lowercase() == t))
        .map(|p| p.path.clone())
}

/// The wiki's paths, fetched and resolved. Re-fetched when the page list
/// changes.
fn use_paths(org: Memo<String>, vault_id: String, pages: Memo<Vec<PageMeta>>) -> Memo<Vec<Path>> {
    let fetched = use_resource(move || {
        let slug = org();
        let vault = vault_id.clone();
        let path_pages: Vec<PageMeta> = pages()
            .into_iter()
            .filter(|p| p.page_type == "path")
            .collect();
        async move {
            let mut out = Vec::new();
            for p in path_pages {
                if let Ok(md) =
                    crate::document_session::fetch_file(slug.clone(), vault.clone(), p.path.clone())
                        .await
                {
                    out.push((p, md));
                }
            }
            out
        }
    });
    use_memo(move || {
        let pages = pages();
        fetched
            .read()
            .as_ref()
            .map(|list| {
                list.iter()
                    .map(|(meta, md)| Path {
                        path: meta.path.clone(),
                        title: meta.title.clone(),
                        steps: steps_of(md)
                            .into_iter()
                            .map(|s| {
                                let at = resolve(&pages, &s.target);
                                (s, at)
                            })
                            .collect(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    })
}

// ── Read marks (per browser) ────────────────────────────────────────

fn read_key(org: &str, wiki: &str) -> String {
    format!("task.wiki-read.{org}.{wiki}")
}

#[cfg(target_arch = "wasm32")]
fn load_read(key: &str) -> BTreeSet<String> {
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| s.get_item(key).ok().flatten())
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default()
}

#[cfg(target_arch = "wasm32")]
fn save_read(key: &str, set: &BTreeSet<String>) {
    if let (Some(s), Ok(json)) = (
        web_sys::window().and_then(|w| w.local_storage().ok().flatten()),
        serde_json::to_string(set),
    ) {
        let _ = s.set_item(key, &json);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn load_read(_key: &str) -> BTreeSet<String> {
    BTreeSet::new()
}

#[cfg(not(target_arch = "wasm32"))]
fn save_read(_key: &str, _set: &BTreeSet<String>) {}

/// The pages this browser has opened in the wiki, with the current one
/// marked as it is opened.
fn use_read_marks(org: &str, wiki: &str, current: &str) -> Signal<BTreeSet<String>> {
    let key = read_key(org, wiki);
    let mut read = use_signal(|| load_read(&key));
    let current = current.to_owned();
    use_effect(use_reactive!(|current| {
        let mut set = read.peek().clone();
        if set.insert(current.clone()) {
            save_read(&key, &set);
            read.set(set);
        }
    }));
    read
}

// ── Views ───────────────────────────────────────────────────────────

/// Under a page: each path it is on, with the step before and after.
/// On a path page: the progress through it, and where to continue.
#[component]
pub fn StudyPathBar(
    org: Memo<String>,
    wiki: String,
    vault_id: String,
    pages: Memo<Vec<PageMeta>>,
    current: String,
) -> Element {
    let paths = use_paths(org, vault_id, pages);
    let read = use_read_marks(&org.peek(), &wiki, &current);
    let route = |path: &str| Route::WikiDocRoute {
        org: org(),
        wiki: wiki.clone(),
        path: path.to_owned(),
    };
    let title_of = |path: &str| {
        pages
            .read()
            .iter()
            .find(|p| p.path == path)
            .map_or_else(|| path.to_owned(), |p| p.title.clone())
    };

    let paths = paths();
    let read = read();
    // The path page itself: progress and "continue".
    if let Some(p) = paths.iter().find(|p| p.path == current) {
        let total = p.steps.iter().filter(|(_, at)| at.is_some()).count();
        let done = p
            .steps
            .iter()
            .filter(|(_, at)| at.as_ref().is_some_and(|a| read.contains(a)))
            .count();
        let next = p
            .steps
            .iter()
            .find_map(|(_, at)| at.as_ref().filter(|a| !read.contains(*a)).cloned());
        let pct = (done * 100).checked_div(total).unwrap_or(0);
        return rsx! {
            div { class: "study-path-progress mb-4 rounded-lg border border-border bg-card/50 px-4 py-3",
                "data-testid": "study-path-progress",
                div { class: "flex items-center gap-3 text-sm",
                    span { class: "font-medium", "{done} of {total} read" }
                    div { class: "h-1.5 flex-1 overflow-hidden rounded-full bg-muted",
                        div { class: "h-full rounded-full bg-primary", style: "width: {pct}%" }
                    }
                    if let Some(n) = next {
                        Link { to: route(&n), class: "shrink-0 rounded-md bg-primary px-2.5 py-1 text-xs font-medium text-primary-foreground hover:bg-primary/85",
                            if done == 0 { "Start →" } else { "Continue: {title_of(&n)} →" }
                        }
                    } else {
                        span { class: "shrink-0 text-xs text-muted-foreground", "Done ✓" }
                    }
                }
            }
        };
    }
    // A page on one or more paths: a footer per path.
    let on: Vec<(Path, usize)> = paths
        .into_iter()
        .filter_map(|p| {
            let i = p
                .steps
                .iter()
                .position(|(_, at)| at.as_deref() == Some(current.as_str()))?;
            Some((p, i))
        })
        .collect();
    if on.is_empty() {
        return rsx! {};
    }
    rsx! {
        div { class: "study-path-footer mt-10 flex flex-col gap-2", "data-testid": "study-path-footer",
            for (p , i) in on {
                {
                    let n = p.steps.len();
                    let prev = p.steps[..i].iter().rev().find_map(|(_, at)| at.clone());
                    let next = p.steps[i + 1..].iter().find_map(|(_, at)| at.clone());
                    let dots: Vec<(bool, bool)> = p
                        .steps
                        .iter()
                        .enumerate()
                        .map(|(k, (_, at))| (k == i, at.as_ref().is_some_and(|a| read.contains(a))))
                        .collect();
                    rsx! {
                        div { key: "{p.path}", class: "rounded-lg border border-border bg-card/50 px-4 py-3 text-sm",
                            div { class: "flex items-center gap-2 text-xs text-muted-foreground",
                                span { class: "font-semibold uppercase tracking-wider", "Study path" }
                                Link { to: route(&p.path), class: "font-medium text-foreground hover:underline", "{p.title}" }
                                span { class: "ml-auto tabular-nums", "Step {i + 1} of {n}" }
                            }
                            div { class: "my-2 flex gap-1",
                                for (k , (here , done)) in dots.into_iter().enumerate() {
                                    span {
                                        key: "{k}",
                                        class: if here { "h-1.5 flex-1 rounded-full bg-primary" } else if done { "h-1.5 flex-1 rounded-full bg-primary/40" } else { "h-1.5 flex-1 rounded-full bg-muted" },
                                    }
                                }
                            }
                            div { class: "flex items-center justify-between gap-3",
                                if let Some(pv) = prev {
                                    Link { to: route(&pv), class: "min-w-0 truncate text-muted-foreground hover:text-foreground", "← {title_of(&pv)}" }
                                } else {
                                    span {}
                                }
                                if let Some(nx) = next {
                                    Link { to: route(&nx), class: "min-w-0 truncate font-medium text-primary hover:underline", "{title_of(&nx)} →" }
                                } else {
                                    Link { to: route(&p.path), class: "text-muted-foreground hover:text-foreground", "End of path ✓" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::steps_of;

    #[test]
    fn a_path_is_its_linked_list_items() {
        let md = "# Start here\n\nIntro prose with [[a link]].\n\n1. [[Psalm 82]] — the trial\n2. [[Elohim|the word]]: what it means\n- [[Divine Council#Tiers]]\n* not a step\n10. [[Last]]\n";
        let steps = steps_of(md);
        let targets: Vec<&str> = steps.iter().map(|s| s.target.as_str()).collect();
        assert_eq!(targets, ["Psalm 82", "Elohim", "Divine Council", "Last"]);
        assert_eq!(steps[0].note, "the trial");
        assert_eq!(steps[1].note, "what it means");
    }
}
