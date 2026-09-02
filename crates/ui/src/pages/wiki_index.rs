//! `/wiki` — the org's wikis, as a list you open.
//!
//! An org holds a set of wikis (`wiki.many.set`), and this is the door
//! to the set: one card per wiki with what it is for, who may see it, and
//! how much is in it; a form to add one; and the way to the graph and to
//! subscriptions. Opening a card goes to the wiki's home
//! (`WikiHomeRoute`), which is where its pages live.
//!
//! The list is fetched again whenever the session changes: on a cold load
//! the first fetch predates the restored token and comes back refused.

use architect_ui::prelude::*;
use dioxus::prelude::*;

use crate::orgs::{OrgMeta, OrgSelection, selected_slugs};
use crate::routes::Route;

/// One org's wikis, for the list — the org rides along so a card can
/// name it and link into it.
#[derive(Clone, PartialEq)]
struct OrgWikis {
    slug: String,
    name: String,
    wikis: Result<Vec<wiki_proto::WikiSummary>, String>,
}

#[component]
pub fn WikiIndexView() -> Element {
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();
    let account = use_context::<Signal<Option<crate::auth::ActiveAccount>>>();

    // Every org the switcher covers: one under a single org, all of
    // mine under "All". The list is the union, grouped by org.
    let orgs = use_memo(move || {
        let list = org_list.read();
        selected_slugs(&selection.read(), &list)
            .into_iter()
            .map(|slug| {
                let name = list
                    .iter()
                    .find(|o| o.slug == slug)
                    .map(|o| o.name.clone())
                    .unwrap_or_else(|| slug.clone());
                (slug, name)
            })
            .collect::<Vec<_>>()
    });

    let mut wikis = use_resource(move || async move {
        let _session = account.read().as_ref().map(|a| a.user_id);
        let targets = orgs();
        if targets.is_empty() {
            return Err("no organization selected".to_owned());
        }
        let mut out = Vec::with_capacity(targets.len());
        for (slug, name) in targets {
            let wikis = crate::feeds::fetch_wikis(&slug).await;
            out.push(OrgWikis { slug, name, wikis });
        }
        Ok::<_, String>(out)
    });

    // The "New wiki" form: shown on demand, cleared on success.
    let mut composing = use_signal(|| false);
    let mut new_title = use_signal(String::new);
    let mut new_purpose = use_signal(String::new);
    let mut new_visibility = use_signal(|| "private".to_owned());
    // Which org gets the new wiki: the only one, or a pick under "All"
    // (defaulting to the first — home).
    let mut new_org = use_signal(String::new);
    let mut create_error = use_signal(|| Option::<String>::None);
    let mut creating = use_signal(|| false);
    let nav = use_navigator();

    let on_create = move |e: Event<FormData>| {
        e.prevent_default();
        let title = new_title.read().trim().to_owned();
        if title.is_empty() || creating() {
            return;
        }
        let picked = new_org.read().clone();
        let slug = if picked.is_empty() {
            orgs().first().map(|(s, _)| s.clone()).unwrap_or_default()
        } else {
            picked
        };
        if slug.is_empty() {
            create_error.set(Some("no organization selected".to_owned()));
            return;
        }
        // Private by default: promotion is what makes private writing
        // public, and it must be a choice (`wiki.promote.vault`).
        let new = wiki_proto::NewWiki {
            title,
            slug: String::new(),
            purpose: new_purpose.read().trim().to_owned(),
            visibility: wiki_proto::Visibility::parse(&new_visibility.read()).unwrap_or_default(),
            source: None,
        };
        creating.set(true);
        spawn(async move {
            match crate::feeds::create_wiki(&slug, new).await {
                Ok(summary) => {
                    new_title.set(String::new());
                    new_purpose.set(String::new());
                    composing.set(false);
                    create_error.set(None);
                    wikis.restart();
                    nav.push(Route::WikiHomeRoute {
                        org: slug.clone(),
                        wiki: summary.slug,
                    });
                }
                Err(err) => create_error.set(Some(err)),
            }
            creating.set(false);
        });
    };

    rsx! {
        div { class: "mx-auto flex h-full w-full max-w-5xl flex-col gap-5 overflow-y-auto p-4 sm:p-6 lg:p-8",
            header { class: "flex flex-col gap-1",
                span { class: "text-[0.7rem] font-semibold uppercase tracking-[0.18em] text-muted-foreground",
                    "Workspace"
                }
                div { class: "flex flex-wrap items-baseline justify-between gap-3",
                    Heading { level: HeadingLevel::H1, class: "tracking-tight", "Wikis" }
                    div { class: "flex items-center gap-2 text-xs",
                        button {
                            class: "rounded-md border border-border/70 px-2.5 py-1 text-xs text-foreground hover:bg-accent",
                            onclick: move |_| {
                                composing.toggle();
                                create_error.set(None);
                            },
                            if composing() { "Cancel" } else { "New wiki" }
                        }
                        Link {
                            to: Route::GraphRoute {},
                            class: "text-muted-foreground underline decoration-border underline-offset-2 hover:text-foreground",
                            "Graph →"
                        }
                        Link {
                            to: Route::WikiSourcesRoute {},
                            class: "text-muted-foreground underline decoration-border underline-offset-2 hover:text-foreground",
                            "Archived sources →"
                        }
                    }
                }
                Text { variant: TextVariant::Muted,
                    "What this org knows, one wiki per subject. A wiki is a vault that can be published: others can subscribe to it, link into it, and ask to change it."
                }
            }

            if composing() {
                form {
                    class: "flex flex-wrap items-center gap-2 rounded-lg border border-border/70 bg-card/40 p-2",
                    onsubmit: on_create,
                    input {
                        class: "min-w-0 flex-1 rounded-lg border border-border/70 bg-background px-2 py-1 text-sm",
                        placeholder: "Title — Music Theory",
                        value: "{new_title}",
                        autofocus: true,
                        oninput: move |e| new_title.set(e.value()),
                    }
                    input {
                        class: "min-w-0 flex-[2] rounded-lg border border-border/70 bg-background px-2 py-1 text-sm",
                        placeholder: "What it is for, in a sentence",
                        value: "{new_purpose}",
                        oninput: move |e| new_purpose.set(e.value()),
                    }
                    if orgs().len() > 1 {
                        select {
                            class: "rounded-lg border border-border/70 bg-background px-2 py-1 text-sm",
                            value: "{new_org}",
                            onchange: move |e| new_org.set(e.value()),
                            for (slug , name) in orgs() {
                                option { key: "{slug}", value: "{slug}", "{name}" }
                            }
                        }
                    }
                    select {
                        class: "rounded-lg border border-border/70 bg-background px-2 py-1 text-sm",
                        value: "{new_visibility}",
                        onchange: move |e| new_visibility.set(e.value()),
                        option { value: "private", "Private" }
                        option { value: "unlisted", "Unlisted" }
                        option { value: "public", "Public" }
                    }
                    button {
                        r#type: "submit",
                        class: "rounded-lg border border-border/70 px-3 py-1 text-sm hover:bg-accent",
                        disabled: creating(),
                        if creating() { "Creating…" } else { "Create" }
                    }
                    if let Some(err) = create_error() {
                        span { class: "basis-full text-xs text-destructive", "{err}" }
                    }
                }
            }

            match &*wikis.read() {
                Some(Ok(groups)) if groups.iter().all(|g| matches!(&g.wikis, Ok(l) if l.is_empty())) => rsx! {
                    div { class: "rounded-xl border border-dashed border-border/70 px-6 py-12 text-center text-sm text-muted-foreground",
                        "No wikis yet. Create the first one above."
                    }
                },
                Some(Ok(groups)) => rsx! {
                    for g in groups.iter() {
                        section { key: "{g.slug}", class: "flex flex-col gap-2",
                            // Under "All" every group is named; under one
                            // org the heading would only repeat the switcher.
                            if groups.len() > 1 {
                                div { class: "flex items-baseline gap-2 pt-1",
                                    Heading { level: HeadingLevel::H2, class: "text-base tracking-tight", "{g.name}" }
                                    span { class: "font-mono text-xs text-muted-foreground", "{g.slug}" }
                                }
                            }
                            match &g.wikis {
                                Ok(list) if list.is_empty() => rsx! {
                                    div { class: "rounded-xl border border-dashed border-border/70 px-4 py-4 text-sm text-muted-foreground",
                                        "No wikis in this org yet."
                                    }
                                },
                                Ok(list) => rsx! {
                                    div { class: "grid gap-3 sm:grid-cols-2",
                                        for w in list.iter() {
                                            {wiki_card(&g.slug, w)}
                                        }
                                    }
                                },
                                Err(e) => rsx! {
                                    div { class: "rounded-xl border border-destructive/40 bg-destructive/10 px-4 py-3 text-sm",
                                        "Couldn't list this org's wikis: {e}"
                                        button {
                                            class: "ml-2 underline",
                                            onclick: move |_| wikis.restart(),
                                            "Retry"
                                        }
                                    }
                                },
                            }
                        }
                    }
                },
                Some(Err(e)) => rsx! {
                    div { class: "rounded-xl border border-destructive/40 bg-destructive/10 px-4 py-3 text-sm",
                        "Couldn't list wikis: {e}"
                        button {
                            class: "ml-2 underline",
                            onclick: move |_| wikis.restart(),
                            "Retry"
                        }
                    }
                },
                None => rsx! {
                    div { class: "flex items-center justify-center rounded-xl border border-border/70 bg-card/30 py-16",
                        Text { variant: TextVariant::Muted, "Loading wikis…" }
                    }
                },
            }

            section { class: "flex flex-col gap-2 pt-2",
                Heading { level: HeadingLevel::H2, class: "text-base tracking-tight", "Subscriptions" }
                Text { variant: TextVariant::Muted,
                    "Sources this org holds. A subscribed wiki's pages resolve inside your own writing."
                }
                crate::pages::wiki_subscriptions::SubscriptionsPanel {}
            }
        }
    }
}

/// One wiki, as a card: title, purpose, visibility, size.
fn wiki_card(org: &str, w: &wiki_proto::WikiSummary) -> Element {
    let title = if w.title.is_empty() {
        w.slug.clone()
    } else {
        w.title.clone()
    };
    let (vis_label, vis_class) = match w.visibility {
        wiki_proto::Visibility::Public => (
            "public",
            "border-emerald-500/40 text-emerald-600 dark:text-emerald-400",
        ),
        wiki_proto::Visibility::Unlisted => (
            "unlisted",
            "border-amber-500/40 text-amber-600 dark:text-amber-400",
        ),
        wiki_proto::Visibility::Private => ("private", "border-border/70 text-muted-foreground"),
    };
    let purpose = if w.purpose.is_empty() {
        "No purpose written yet.".to_owned()
    } else {
        w.purpose.clone()
    };
    let slug = w.slug.clone();
    let org = org.to_owned();
    rsx! {
        Link {
            key: "{w.slug}",
            to: Route::WikiHomeRoute { org, wiki: slug },
            class: "group flex flex-col gap-2 rounded-xl border border-border/70 bg-card/40 p-4 text-left transition hover:border-border hover:bg-card/70",
            div { class: "flex items-start justify-between gap-3",
                span { class: "text-base font-semibold text-foreground group-hover:underline", "{title}" }
                div { class: "flex shrink-0 items-center gap-1.5 text-[0.65rem]",
                    if w.default {
                        span { class: "rounded-full border border-border/70 px-2 py-0.5 text-muted-foreground", "default" }
                    }
                    if w.repo_sourced {
                        span { class: "rounded-full border border-sky-500/40 px-2 py-0.5 text-sky-600 dark:text-sky-400", "from a repo" }
                    }
                    span { class: "rounded-full border px-2 py-0.5 {vis_class}", "{vis_label}" }
                }
            }
            p { class: "line-clamp-3 text-sm text-muted-foreground", "{purpose}" }
            div { class: "mt-auto flex items-center justify-between text-xs text-muted-foreground",
                span { class: "font-mono", "{w.slug}" }
                span { "{w.pages} pages" }
            }
        }
    }
}
