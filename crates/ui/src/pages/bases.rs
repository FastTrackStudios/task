//! `/bases` — render the vault's Obsidian `.base` files as live tables.
//!
//! The query engine is native (it can't run on `wasm32`), so the views
//! are executed server-side via `VaultSync::base_views` and arrive as
//! flat [`vault_proto::BaseView`]s (column headers + grouped, projected
//! rows). This page just paints them: a base picker across the top, then
//! one table per view, with `groupBy` labels as section rows. Clicking a
//! row opens that note in `/vault`.

use architect_ui::prelude::*;
use dioxus::prelude::*;
use vault_proto::BaseView;

use crate::orgs::{OrgMeta, OrgSelection, selected_slugs};

/// Where a base row opens. Recipes are `.cook` files, not notes — the
/// vault viewer would show raw cooklang, so they open in the recipe
/// reader instead. Everything else is a note.
fn open_route(path: String) -> crate::routes::Route {
    if path.ends_with(".cook") {
        crate::routes::Route::RecipeReadRoute { path }
    } else {
        crate::routes::Route::VaultRoute {
            path,
            org: String::new(),
        }
    }
}

/// Display label for a base path (`Scripture/Songs.base` → `Songs`).
fn base_label(path: &str) -> String {
    path.rsplit('/')
        .next()
        .unwrap_or(path)
        .trim_end_matches(".base")
        .to_string()
}

#[component]
pub fn BasesView() -> Element {
    let nav = use_navigator();
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();
    let mut selected = use_signal(|| None::<String>);

    let bases = use_resource(move || async move {
        let slug = selected_slugs(&selection.read(), &org_list.read())
            .first()
            .cloned()
            .ok_or_else(|| "no organization selected".to_string())?;
        crate::feeds::fetch_bases(&slug).await
    });

    // The active base: explicit pick, else the first available.
    let views = use_resource(move || async move {
        let slug = selected_slugs(&selection.read(), &org_list.read())
            .first()
            .cloned()?;
        let active = match selected() {
            Some(s) => s,
            None => bases.read().as_ref()?.as_ref().ok()?.first().cloned()?,
        };
        let v = crate::feeds::fetch_base_views(&slug, &active).await.ok()?;
        Some((active, v))
    });

    // Base picker chips.
    let base_list: Vec<String> = match &*bases.read() {
        Some(Ok(list)) => list.clone(),
        _ => Vec::new(),
    };
    let active = views
        .read()
        .as_ref()
        .and_then(|o| o.as_ref().map(|(a, _)| a.clone()));
    let picker = rsx! {
        div { class: "flex flex-wrap gap-1.5",
            for path in base_list.iter().cloned() {
                {
                    let is_active = active.as_deref() == Some(path.as_str());
                    let label = base_label(&path);
                    let cls = if is_active {
                        "rounded-md border border-primary/40 bg-primary/10 px-2.5 py-1 text-xs font-medium text-foreground"
                    } else {
                        "rounded-md border border-border bg-background px-2.5 py-1 text-xs text-muted-foreground hover:bg-accent hover:text-foreground"
                    };
                    rsx! {
                        button {
                            key: "{path}",
                            r#type: "button",
                            class: "{cls}",
                            onclick: move |_| selected.set(Some(path.clone())),
                            "{label}"
                        }
                    }
                }
            }
        }
    };

    let body = match &*views.read() {
        Some(Some((_, views))) if !views.is_empty() => {
            rsx! {
                div { class: "flex flex-col gap-6",
                    for view in views.clone() {
                        BaseViewRender { key: "{view.name}", view, on_open: move |path: String| {
                            nav.push(open_route(path));
                        } }
                    }
                }
            }
        }
        Some(Some(_)) => rsx! {
            div { class: "rounded-xl border border-dashed border-border/70 bg-card/40 px-4 py-10 text-center",
                Text { variant: TextVariant::Muted, "This base has no views, or nothing matched its filter." }
            }
        },
        Some(None) if base_list.is_empty() => rsx! {
            div { class: "rounded-xl border border-dashed border-border/70 bg-card/40 px-4 py-10 text-center",
                Text { variant: TextVariant::Muted, "No .base files in this vault yet." }
            }
        },
        _ => rsx! {
            div { class: "flex items-center justify-center rounded-xl border border-border/70 bg-card/30 py-10",
                Text { variant: TextVariant::Muted, "Loading bases…" }
            }
        },
    };

    rsx! {
        div { class: "mx-auto flex h-full w-full max-w-5xl flex-col gap-4 p-4 sm:p-6 lg:p-8",
            header { class: "flex flex-col gap-2",
                span { class: "text-[0.7rem] font-semibold uppercase tracking-[0.18em] text-muted-foreground",
                    "Vault"
                }
                Heading { level: HeadingLevel::H1, class: "tracking-tight", "Bases" }
                Text {
                    variant: TextVariant::Muted,
                    "Obsidian Bases — saved table views over your vault notes. Pick one to filter and group."
                }
                {picker}
            }
            {body}
        }
    }
}

/// Dispatch a view to the renderer matching its Obsidian `view_type`
/// (table / cards / list); board, calendar and unknown types fall back to
/// a table.
#[component]
fn BaseViewRender(view: BaseView, on_open: EventHandler<String>) -> Element {
    let rows: usize = view.groups.iter().map(|g| g.rows.len()).sum();
    let body = match view.view_type.as_str() {
        "cards" | "gallery" => rsx! { BaseCardsBody { view: view.clone(), on_open } },
        "list" => rsx! { BaseListBody { view: view.clone(), on_open } },
        _ => rsx! { BaseTableBody { view: view.clone(), on_open } },
    };
    rsx! {
        section { class: "flex flex-col gap-2",
            div { class: "flex items-baseline gap-2",
                Heading { level: HeadingLevel::H3, "{view.name}" }
                span { class: "rounded bg-muted/50 px-1.5 py-0.5 text-[10px] uppercase tracking-wide text-muted-foreground",
                    "{view.view_type}"
                }
                span { class: "text-xs text-muted-foreground", "{rows} rows" }
            }
            {body}
        }
    }
}

#[component]
fn BaseTableBody(view: BaseView, on_open: EventHandler<String>) -> Element {
    let col_count = view.columns.len();
    rsx! {
            div { class: "overflow-hidden rounded-xl border border-border/70",
                table { class: "w-full border-collapse text-sm",
                    thead {
                        tr { class: "border-b border-border/70 bg-muted/40 text-left",
                            for col in view.columns.iter() {
                                th { key: "{col}", class: "px-3 py-2 font-medium text-muted-foreground", "{col}" }
                            }
                        }
                    }
                    tbody {
                        for group in view.groups.iter() {
                            if !group.label.is_empty() {
                                tr { class: "bg-muted/20",
                                    td {
                                        colspan: "{col_count}",
                                        class: "px-3 py-1.5 text-xs font-semibold uppercase tracking-wide text-muted-foreground",
                                        "{group.label}"
                                    }
                                }
                            }
                            for row in group.rows.iter() {
                                {
                                    let path = row.path.clone();
                                    rsx! {
                                        tr {
                                            key: "{row.path}",
                                            class: "cursor-pointer border-b border-border/40 last:border-0 hover:bg-accent/50",
                                            onclick: move |_| on_open.call(path.clone()),
                                            for (i, cell) in row.cells.iter().enumerate() {
                                                td {
                                                    key: "{i}",
                                                    class: if i == 0 { "px-3 py-2 font-medium text-foreground" } else { "px-3 py-2 text-muted-foreground" },
                                                    "{cell}"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
    }
}

#[component]
fn BaseCardsBody(view: BaseView, on_open: EventHandler<String>) -> Element {
    rsx! {
        div { class: "flex flex-col gap-3",
            for group in view.groups.iter() {
                if !group.label.is_empty() {
                    div { class: "text-xs font-semibold uppercase tracking-wide text-muted-foreground",
                        "{group.label}"
                    }
                }
                div { class: "grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3",
                    for row in group.rows.iter() {
                        {
                            let path = row.path.clone();
                            let cols = view.columns.clone();
                            rsx! {
                                button {
                                    key: "{row.path}",
                                    r#type: "button",
                                    class: "flex flex-col gap-1.5 rounded-lg border border-border/70 bg-card/40 p-3 text-left hover:bg-accent/40",
                                    onclick: move |_| on_open.call(path.clone()),
                                    div { class: "font-medium text-foreground", "{row.title}" }
                                    for (i, cell) in row.cells.iter().enumerate() {
                                        if i > 0 && !cell.is_empty() {
                                            div { key: "{i}", class: "text-xs text-muted-foreground",
                                                span { class: "text-muted-foreground/60",
                                                    "{cols.get(i).cloned().unwrap_or_default()}: "
                                                }
                                                "{cell}"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn BaseListBody(view: BaseView, on_open: EventHandler<String>) -> Element {
    rsx! {
        div { class: "flex flex-col gap-3",
            for group in view.groups.iter() {
                if !group.label.is_empty() {
                    div { class: "text-xs font-semibold uppercase tracking-wide text-muted-foreground",
                        "{group.label}"
                    }
                }
                ul { class: "flex flex-col gap-1",
                    for row in group.rows.iter() {
                        {
                            let path = row.path.clone();
                            let secondary: String = row
                                .cells
                                .iter()
                                .skip(1)
                                .filter(|c| !c.is_empty())
                                .cloned()
                                .collect::<Vec<_>>()
                                .join(" · ");
                            rsx! {
                                li { key: "{row.path}", class: "flex items-baseline gap-2",
                                    span { class: "text-muted-foreground/50", "•" }
                                    button {
                                        r#type: "button",
                                        class: "text-left text-sm font-medium text-foreground hover:underline",
                                        onclick: move |_| on_open.call(path.clone()),
                                        "{row.title}"
                                    }
                                    if !secondary.is_empty() {
                                        span { class: "text-xs text-muted-foreground", "{secondary}" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// One base rendered as its view tables — shared by the `/bases` page and
/// embedded in the vault content pane when a `.base` file is open
/// (Obsidian-style). `on_open` fires with a note's vault path on row click.
#[component]
pub fn BaseDoc(base_path: String, on_open: EventHandler<String>) -> Element {
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();
    let bp = base_path.clone();
    let views = use_resource(move || {
        let bp = bp.clone();
        async move {
            let slug = selected_slugs(&selection.read(), &org_list.read())
                .first()
                .cloned()?;
            crate::feeds::fetch_base_views(&slug, &bp).await.ok()
        }
    });
    let snapshot = views.read().clone();
    match snapshot {
        Some(Some(vs)) if !vs.is_empty() => {
            // App-view dispatch (plans/vault-views.md): a view kind
            // registered in `crate::app_views` renders the full custom
            // page in place of a generic table — the base entry is the
            // page's vault identity. First registered kind wins (an app
            // view owns the whole pane); generic views render stacked.
            if let Some(el) = vs
                .iter()
                .find_map(|v| crate::app_views::render(&v.view_type))
            {
                rsx! {
                    div { class: "flex min-h-0 flex-1 flex-col", {el} }
                }
            } else {
                rsx! {
                    div { class: "flex flex-col gap-6 p-4 sm:p-6",
                        for view in vs {
                            BaseViewRender { key: "{view.name}", view, on_open }
                        }
                    }
                }
            }
        }
        Some(Some(_)) => rsx! {
            div { class: "p-6",
                Text { variant: TextVariant::Muted, "This base matched no notes." }
            }
        },
        _ => rsx! {
            div { class: "p-6",
                Text { variant: TextVariant::Muted, "Loading base…" }
            }
        },
    }
}
