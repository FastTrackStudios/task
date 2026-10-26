//! App docs in the UI: the documents client apps keep in each org's
//! app-docs catalog, shown by the face each keeps
//! (`app_docs_proto::DocFace`).
//!
//! Generic over any app. A kind is named by its docs' faces
//! ([`DocCard::kind_label`], the app's `kind_label` prop) and never by
//! Task; properties render from their own labels and types.
//!
//! - [`ProjectAppDocs`]: a project page's section — the docs attached
//!   to the project, grouped by kind.
//! - [`AppDocsPage`] (`/docs`): every doc in the selected orgs,
//!   filterable by kind.
//! - [`AppDocPage`] (`/docs/:id`): one doc's face and its comments.

use std::collections::BTreeMap;

use app_docs_proto::{DocCard, FacePropType, KIND_LABEL};
use architect_ui::prelude::*;
use dioxus::prelude::*;
use task_ui_core::orgs::{OrgSelection, selected_slugs};
use uuid::Uuid;

use crate::orgs::OrgMeta;

/// How many props a row shows beside its title.
const ROW_PROPS: usize = 3;

/// The selected orgs' slugs.
fn use_slugs() -> Memo<Vec<String>> {
    let selection = use_context::<Signal<OrgSelection>>();
    let orgs = use_context::<Signal<Vec<OrgMeta>>>();
    use_memo(move || selected_slugs(&selection.read(), &orgs.read()))
}

/// Cards grouped by what their kind is called, groups in name order,
/// cards in title order.
fn by_kind(cards: &[(String, DocCard)]) -> BTreeMap<String, Vec<(String, DocCard)>> {
    let mut groups: BTreeMap<String, Vec<(String, DocCard)>> = BTreeMap::new();
    for (slug, card) in cards {
        groups
            .entry(card.kind_label())
            .or_default()
            .push((slug.clone(), card.clone()));
    }
    for rows in groups.values_mut() {
        rows.sort_by_key(|(_, c)| c.title.to_lowercase());
    }
    groups
}

/// The props a row shows: the first few, the kind's label left out.
fn row_props(card: &DocCard) -> Vec<(String, String)> {
    card.face
        .iter()
        .flat_map(|f| f.props.iter())
        .filter(|p| p.key != KIND_LABEL)
        .take(ROW_PROPS)
        .map(|p| (p.label.clone(), p.value.display()))
        .collect()
}

/// One doc: its title (to its page), subtitle, a few props, comments.
#[component]
fn DocRow(card: DocCard) -> Element {
    let nav = use_navigator();
    let href = format!("/docs/{}", card.doc_id);
    let subtitle = card
        .face
        .as_ref()
        .and_then(|f| f.subtitle.clone())
        .unwrap_or_default();
    let colour = card
        .face
        .as_ref()
        .and_then(|f| f.colour.clone())
        .unwrap_or_default();
    let props = row_props(&card);
    rsx! {
        div {
            class: "flex items-center gap-3 px-4 py-2.5 cursor-pointer hover:bg-accent/20 transition-colors",
            onclick: move |_| {
                nav.push(href.as_str());
            },
            span {
                class: "size-2 shrink-0 rounded-full bg-muted",
                style: if colour.is_empty() { String::new() } else { format!("background-color: {colour}") },
            }
            div { class: "flex min-w-0 flex-1 flex-col",
                span { class: "min-w-0 truncate text-sm text-foreground", "{card.title}" }
                if !subtitle.is_empty() {
                    span { class: "min-w-0 truncate text-xs text-muted-foreground", "{subtitle}" }
                }
            }
            for (i, (label, value)) in props.into_iter().enumerate() {
                span {
                    key: "{i}",
                    class: "shrink-0 rounded-full bg-muted/60 px-2 py-0.5 text-[10px] text-muted-foreground",
                    title: "{label}",
                    "{label} · {value}"
                }
            }
            if card.comments > 0 {
                span {
                    class: "shrink-0 text-xs tabular-nums text-muted-foreground",
                    title: "Comments",
                    "💬 {card.comments}"
                }
            }
        }
    }
}

/// Groups of rows under their kind's name.
#[component]
fn KindGroups(cards: Vec<(String, DocCard)>) -> Element {
    let groups = by_kind(&cards);
    rsx! {
        for (label, rows) in groups.into_iter() {
            div { key: "{label}", class: "flex flex-col gap-2",
                div { class: "flex items-baseline gap-2",
                    Heading { level: HeadingLevel::H3, class: "text-sm font-semibold", "{label}" }
                    span { class: "text-xs tabular-nums text-muted-foreground", "· {rows.len()}" }
                }
                div { class: "flex flex-col divide-y divide-border/60 rounded-xl border border-border bg-card/40",
                    for (slug, card) in rows.into_iter() {
                        DocRow { key: "{slug}:{card.doc_id}", card }
                    }
                }
            }
        }
    }
}

/// A project page's app docs: what the project holds through its
/// links, grouped by kind. Renders nothing when it holds none.
#[component]
pub fn ProjectAppDocs(slug: String, project_id: Uuid) -> Element {
    let key = use_memo(use_reactive!(|(slug, project_id)| (slug, project_id)));
    let held = use_resource(move || async move {
        let (slug, id) = key();
        let cards = crate::feeds::fetch_project_faces(&slug, id).await;
        cards.map(|cs| cs.into_iter().map(|c| (slug.clone(), c)).collect::<Vec<_>>())
    });
    let cards = match &*held.read() {
        Some(Ok(cards)) if !cards.is_empty() => cards.clone(),
        Some(Err(e)) => {
            tracing::warn!(error = %e, "project app docs fetch failed");
            return rsx! {};
        }
        _ => return rsx! {},
    };
    rsx! {
        div { class: "flex flex-col gap-3",
            div { class: "flex items-baseline gap-2",
                Heading { level: HeadingLevel::H2, class: "text-lg font-semibold", "Documents" }
                span { class: "text-xs tabular-nums text-muted-foreground", "· {cards.len()}" }
            }
            KindGroups { cards }
        }
    }
}

/// `/docs` — every app doc in the selected orgs, by kind, with a filter
/// to one kind ("all my setlists" is one click).
#[component]
pub fn AppDocsPage() -> Element {
    let slugs = use_slugs();
    let mut kind = use_signal(|| None::<String>);
    let listed = use_resource(move || async move {
        let slugs = slugs();
        crate::feeds::fetch_app_doc_faces(&slugs).await
    });
    let all = match &*listed.read() {
        Some(Ok(cards)) => cards.clone(),
        Some(Err(e)) => {
            return rsx! {
                crate::pages::missing::Missing {
                    title: "Couldn't load documents".to_owned(),
                    detail: e.clone(),
                }
            };
        }
        None => Vec::new(),
    };
    let labels: Vec<String> = by_kind(&all).into_keys().collect();
    let shown: Vec<(String, DocCard)> = match kind() {
        Some(k) => all.iter().filter(|(_, c)| c.kind_label() == k).cloned().collect(),
        None => all.clone(),
    };
    let chip = |on: bool| {
        if on {
            "rounded-full border border-primary bg-primary/10 px-3 py-1 text-xs text-foreground"
        } else {
            "rounded-full border border-border px-3 py-1 text-xs text-muted-foreground hover:text-foreground"
        }
    };
    rsx! {
        section { class: "mx-auto flex w-full max-w-4xl flex-col gap-4 p-6",
            div { class: "flex items-baseline gap-2",
                Heading { level: HeadingLevel::H1, class: "text-2xl font-semibold", "Documents" }
                span { class: "text-xs tabular-nums text-muted-foreground", "· {all.len()}" }
            }
            Text { variant: TextVariant::Muted,
                "What your apps keep here — each shown by the face it gives itself."
            }
            if !labels.is_empty() {
                div { class: "flex flex-wrap items-center gap-2",
                    button {
                        class: chip(kind().is_none()),
                        onclick: move |_| kind.set(None),
                        "All"
                    }
                    for label in labels.into_iter() {
                        button {
                            key: "{label}",
                            class: chip(kind().as_deref() == Some(label.as_str())),
                            onclick: {
                                let label = label.clone();
                                move |_| kind.set(Some(label.clone()))
                            },
                            "{label}"
                        }
                    }
                }
            }
            if shown.is_empty() && listed.read().is_some() {
                Text { variant: TextVariant::Muted, "No documents yet." }
            } else {
                KindGroups { cards: shown }
            }
        }
    }
}

/// How a prop's value reads on the doc's page.
fn prop_value(ty: FacePropType, value: &app_docs_proto::FaceValue) -> String {
    match (ty, value) {
        (FacePropType::Duration, app_docs_proto::FaceValue::Integer(s)) => {
            format!("{}:{:02}", s / 60, s % 60)
        }
        _ => value.display(),
    }
}

/// `/docs/:id` — one app doc: its face, its links, and its comments.
#[component]
pub fn AppDocPage(id: String) -> Element {
    let slugs = use_slugs();
    let doc_id = Uuid::parse_str(&id).ok();
    let found = use_resource(use_reactive!(|(doc_id,)| async move {
        let doc_id = doc_id?;
        for slug in slugs() {
            if let Ok(card) = crate::feeds::fetch_app_doc_face(&slug, doc_id).await {
                return Some((slug, card));
            }
        }
        None
    }));
    let Some(Some((slug, card))) = found.read().clone() else {
        return rsx! {
            if found.read().is_some() {
                crate::pages::missing::Missing {
                    title: "No such document".to_owned(),
                    detail: "None of your selected organizations holds it, or it was deleted."
                        .to_owned(),
                }
            }
        };
    };
    let face = card.face.clone().unwrap_or_default();
    let kind = card.kind_label();
    rsx! {
        section { class: "mx-auto flex w-full max-w-3xl flex-col gap-5 p-6",
            div { class: "flex flex-col gap-1",
                span { class: "text-[10px] uppercase tracking-wide text-muted-foreground", "{kind}" }
                Heading { level: HeadingLevel::H1, class: "text-2xl font-semibold", "{card.title}" }
                if let Some(sub) = face.subtitle.clone() {
                    Text { variant: TextVariant::Muted, "{sub}" }
                }
            }
            if !face.props.is_empty() {
                div { class: "flex flex-col divide-y divide-border/60 rounded-xl border border-border bg-card/40",
                    for p in face.props.iter().filter(|p| p.key != KIND_LABEL) {
                        div { key: "{p.key}", class: "flex items-center gap-3 px-4 py-2.5",
                            span { class: "w-40 shrink-0 text-xs text-muted-foreground", "{p.label}" }
                            span { class: "min-w-0 flex-1 truncate text-sm text-foreground",
                                {prop_value(p.ty, &p.value)}
                            }
                        }
                    }
                }
            }
            if !face.links.is_empty() {
                div { class: "flex flex-col gap-2",
                    Heading { level: HeadingLevel::H3, class: "text-sm font-semibold", "Links" }
                    div { class: "flex flex-col divide-y divide-border/60 rounded-xl border border-border bg-card/40",
                        for l in face.links.iter() {
                            Link {
                                key: "{l.rel}:{l.doc}",
                                class: "flex items-center gap-3 px-4 py-2.5 hover:bg-accent/20",
                                to: format!("/docs/{}", l.doc),
                                span { class: "w-40 shrink-0 text-xs text-muted-foreground", "{l.rel}" }
                                span { class: "min-w-0 flex-1 truncate text-sm text-foreground",
                                    if l.label.is_empty() { "{l.doc}" } else { "{l.label}" }
                                }
                            }
                        }
                    }
                }
            }
            crate::activity::EntityActivity {
                slug,
                entity_type: "doc".to_owned(),
                entity_id: card.doc_id,
            }
        }
    }
}
