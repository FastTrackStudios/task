//! `/wiki/page?:path` — read + edit one curated wiki page.
//!
//! The knowledge-graph view deep-links here on node click. Reads
//! and sha-guarded writes go through the org's `wiki_proto`
//! `Pages` service (the curated `<org>/wiki/Knowledge/` tree),
//! so the page honors the org switcher like every other wiki
//! surface. View mode renders the body through the shared
//! [`task_ui::Markdown`] block renderer (frontmatter lifted into
//! a header strip); edit mode is a plain-markdown textarea over
//! the raw file — the rich vault editor stays vault-only until
//! the wiki grows its own document-session transport.

use architect_ui::prelude::*;
use dioxus::prelude::*;
use wiki_proto::pages::WikiPageDoc;

async fn pages_client(slug: &str) -> Result<wiki_proto::service::pages::PagesClient, String> {
    crate::vox_clients::establish_for::<wiki_proto::service::pages::PagesClient>(slug).await
}

async fn fetch_page(slug: &str, wiki: &str, path: &str) -> Result<WikiPageDoc, String> {
    let c = pages_client(slug).await?;
    c.read_page(wiki.to_owned(), path.to_owned())
        .await
        .map_err(|e| format!("read_page: {e:?}"))
}

async fn save_page(
    slug: &str,
    wiki: &str,
    doc_path: &str,
    markdown: &str,
    base_sha256: &str,
) -> Result<WikiPageDoc, String> {
    let c = pages_client(slug).await?;
    c.write_page(
        wiki.to_owned(),
        doc_path.to_owned(),
        markdown.to_owned(),
        base_sha256.to_owned(),
    )
    .await
    .map_err(|e| format!("write_page: {e:?}"))
}

/// Frontmatter key/values + the body that follows. Only flat
/// `key: value` lines are lifted; anything else stays raw.
fn split_frontmatter(raw: &str) -> (Vec<(String, String)>, &str) {
    let Some(rest) = raw.strip_prefix("---\n") else {
        return (Vec::new(), raw);
    };
    let Some(end) = rest.find("\n---") else {
        return (Vec::new(), raw);
    };
    let mut kv = Vec::new();
    for line in rest[..end].lines() {
        if let Some((k, v)) = line.split_once(':') {
            let v = v.trim().trim_matches('"').trim_matches('\'');
            if !v.is_empty() {
                kv.push((k.trim().to_string(), v.to_string()));
            }
        }
    }
    (kv, rest[end..].trim_start_matches("\n---").trim_start())
}

fn fm_get<'a>(kv: &'a [(String, String)], key: &str) -> Option<&'a str> {
    kv.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

#[component]
pub fn WikiPageView(org: String, wiki: String, path: String) -> Element {
    // The org is the route's (a wiki belongs to one org; under "All" the
    // list spans several), so every read and write here goes to it.
    let org_sig = use_signal(|| org.clone());

    // Refetch on org switch or deep-link change.
    let fetch_path = path.clone();
    let fetch_wiki = wiki.clone();
    let mut page = use_resource(use_reactive!(|(fetch_wiki, fetch_path)| async move {
        let slug = org_sig();
        fetch_page(&slug, &fetch_wiki, &fetch_path).await
    }));
    let live_wiki = wiki.clone();

    // ── Live page changes ─────────────────────────────────────
    // The `Events` `#[subscribe]` stream — a page rewritten by the
    // wiki pipeline (ingest output, an applied review action) or by
    // another reader refreshes here instead of showing a stale body
    // until reload. An edit in progress wins: re-reading under the
    // author would throw away their draft, and the sha guard on save
    // already turns a concurrent write into a visible conflict.
    let mut editing = use_signal(|| false);
    let live_path = path.clone();
    architect::use_stream(
        move |tx| {
            let slug = org_sig();
            async move {
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
            let mut page = page;
            if change.wiki_id != live_wiki || *editing.peek() {
                return;
            }
            let touched = match &change.event {
                wiki_proto::WikiEvent::PageWritten { path, .. }
                | wiki_proto::WikiEvent::PageDeleted { path, .. } => path == &live_path,
                wiki_proto::WikiEvent::Resync => true,
                _ => false,
            };
            if touched {
                page.restart();
            }
        },
    );

    let mut draft = use_signal(String::new);
    let mut base_sha = use_signal(String::new);
    let mut save_error = use_signal(String::new);
    let mut saving = use_signal(|| false);

    let save_path = path.clone();
    let save_wiki = wiki.clone();
    let on_save = move |_| {
        let save_path = save_path.clone();
        let save_wiki = save_wiki.clone();
        spawn(async move {
            let slug = org_sig();
            saving.set(true);
            save_error.set(String::new());
            let res = save_page(
                &slug,
                &save_wiki,
                &save_path,
                &draft.peek(),
                &base_sha.peek(),
            )
            .await;
            saving.set(false);
            match res {
                Ok(_) => {
                    editing.set(false);
                    page.restart();
                }
                Err(e) => save_error.set(e),
            }
        });
    };

    let body = match &*page.read() {
        Some(Ok(doc)) => {
            let (fm, md_body) = split_frontmatter(&doc.markdown);
            let title = fm_get(&fm, "title")
                .map(str::to_string)
                .or_else(|| {
                    md_body
                        .lines()
                        .find_map(|l| l.strip_prefix("# "))
                        .map(|t| t.trim().to_string())
                })
                .unwrap_or_else(|| {
                    std::path::Path::new(&doc.path)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or(&doc.path)
                        .to_string()
                });
            let page_type = fm_get(&fm, "type").unwrap_or_default().to_string();
            // Provenance: `ai_generated: true` marks machine-produced
            // content (the wiki's whole point — non-user-authored
            // knowledge); `generated_by:` names the model/agent.
            let ai_generated =
                fm_get(&fm, "ai_generated").is_some_and(|v| v.eq_ignore_ascii_case("true"));
            let generated_by = fm_get(&fm, "generated_by").unwrap_or_default().to_string();
            let doc_markdown = doc.markdown.clone();
            let doc_sha = doc.sha256.clone();
            rsx! {
                header { class: "flex flex-col gap-2",
                    div { class: "flex items-start justify-between gap-3",
                        Heading { level: HeadingLevel::H2, class: "tracking-tight", "{title}" }
                        if editing() {
                            div { class: "flex shrink-0 items-center gap-2",
                                Button {
                                    variant: ButtonVariant::Ghost,
                                    size: ButtonSize::Small,
                                    on_click: move |_| {
                                        editing.set(false);
                                        save_error.set(String::new());
                                    },
                                    "Cancel"
                                }
                                Button {
                                    variant: ButtonVariant::Primary,
                                    size: ButtonSize::Small,
                                    disabled: saving(),
                                    on_click: on_save.clone(),
                                    if saving() { "Saving…" } else { "Save" }
                                }
                            }
                        } else {
                            Button {
                                variant: ButtonVariant::Outline,
                                size: ButtonSize::Small,
                                on_click: move |_| {
                                    draft.set(doc_markdown.clone());
                                    base_sha.set(doc_sha.clone());
                                    save_error.set(String::new());
                                    editing.set(true);
                                },
                                "Edit"
                            }
                        }
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
                        span { class: "font-mono", "{doc.path}" }
                        for (k , v) in fm.iter().filter(|(k, _)| {
                            k != "title" && k != "type" && k != "ai_generated" && k != "generated_by"
                        }) {
                            span { key: "{k}", "{k}: {v}" }
                        }
                    }
                }
                if !save_error.read().is_empty() {
                    div { class: "rounded-xl border border-destructive/40 bg-destructive/10 px-4 py-3 text-sm",
                        "Couldn't save: {save_error}"
                        Button {
                            variant: ButtonVariant::Ghost,
                            size: ButtonSize::Small,
                            class: "ml-2",
                            on_click: move |_| {
                                editing.set(false);
                                save_error.set(String::new());
                                page.restart();
                            },
                            "Reload"
                        }
                    }
                }
                if editing() {
                    textarea {
                        class: "min-h-[60vh] w-full flex-1 resize-y rounded-xl border border-border/70 bg-card/30 p-4 font-mono text-sm leading-relaxed text-foreground outline-none focus:border-primary/60",
                        spellcheck: false,
                        value: "{draft}",
                        oninput: move |e| draft.set(e.value()),
                    }
                } else {
                    article { class: "flex flex-col gap-1 pb-12",
                        task_ui::Markdown { source: md_body.to_string() }
                    }
                }
            }
        }
        Some(Err(e)) => rsx! {
            crate::states::ErrorState {
                title: "Couldn't load this page",
                message: e.clone(),
            }
        },
        None => rsx! {
            div { class: "flex items-center justify-center rounded-xl border border-border/70 bg-card/30 py-16",
                Text { variant: TextVariant::Muted, "Loading page…" }
            }
        },
    };

    rsx! {
        div { class: "mx-auto flex h-full w-full max-w-3xl flex-col gap-4 overflow-y-auto p-4 sm:p-6 lg:p-8",
            Link {
                to: crate::routes::Route::WikiHomeRoute { org: org.clone(), wiki: wiki.clone() },
                class: "text-xs text-muted-foreground hover:text-foreground",
                "← {wiki}"
            }
            {body}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatter_lifts_flat_keys_and_returns_body() {
        let raw = "---\ntitle: \"Spaced repetition\"\ntype: concept\nsources: [a.md]\n---\n\nBody text.\n";
        let (fm, body) = split_frontmatter(raw);
        assert_eq!(fm_get(&fm, "title"), Some("Spaced repetition"));
        assert_eq!(fm_get(&fm, "type"), Some("concept"));
        assert_eq!(body, "Body text.\n");
    }

    #[test]
    fn no_frontmatter_passes_through() {
        let (fm, body) = split_frontmatter("# Just a page\n");
        assert!(fm.is_empty());
        assert_eq!(body, "# Just a page\n");
    }
}
