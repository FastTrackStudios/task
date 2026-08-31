//! `/mealplan/shopping` — the two-pass shopping run.
//!
//! The flow is the one you actually walk. **Kitchen** first: every row
//! you still need, tapped off as you find it on the shelf. Then
//! **Store**: only what didn't turn up, tapped off into the basket —
//! and that tap restocks the pantry, because a purchase is the only
//! half that creates stock.
//!
//! Nothing here is a one-off. A run is a vault page that persists
//! between sessions, so you can check the cupboard tonight and shop
//! tomorrow without losing a tick. And a run can be kept as a
//! **template** — the staples, a recipe's ingredients — then started
//! fresh whenever you cook it again, leaving the template clean for
//! next time.
//!
//! Mobile-first, same shape as [`super::cook_mode`]: full-screen sheet,
//! fat tap targets, a phase rail that ticks off as you go.

use architect_ui::lucide_dioxus::{
    Check, ChevronLeft, CirclePlus, ListChecks, Plus, RotateCcw, ShoppingCart, Store, X,
};
use architect_ui::prelude::*;
use dioxus::prelude::*;
use mealplan_proto::{EntryStatus, ShoppingEntry, ShoppingList};

use task_ui_core::orgs::{OrgMeta, OrgSelection};

/// Which pass the cook is on.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pass {
    /// Round the kitchen — everything still needed, tap what you have.
    Kitchen,
    /// At the shop — only what the kitchen didn't cover.
    Store,
}

impl Pass {
    fn label(self) -> &'static str {
        match self {
            Pass::Kitchen => "In the kitchen",
            Pass::Store => "At the store",
        }
    }
}

#[component]
pub fn ShoppingView() -> Element {
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();
    let notices = architect::use_notifications();
    let nav = use_navigator();

    let slug = use_memo(move || {
        task_ui_core::orgs::selected_slugs(&selection.read(), &org_list.read())
            .into_iter()
            .next()
    });

    // All lists for the org — runs and templates in one fetch, split
    // client-side by `is_template`.
    let mut lists = use_resource(move || async move {
        let s = slug()?;
        crate::fetch_shopping_lists(&s).await.ok()
    });

    let mut selected = use_signal(|| None::<uuid::Uuid>);
    let mut pass = use_signal(|| Pass::Kitchen);
    let mut busy = use_signal(|| false);

    let snapshot = lists.read().clone().flatten().unwrap_or_default();
    let runs: Vec<ShoppingList> = snapshot
        .iter()
        .filter(|l| !l.is_template)
        .cloned()
        .collect();
    let templates: Vec<ShoppingList> = snapshot.iter().filter(|l| l.is_template).cloned().collect();

    // The open run: the explicit pick, else the first with anything
    // outstanding, else the first run at all.
    let active: Option<ShoppingList> = selected()
        .and_then(|id| runs.iter().find(|l| l.id == id).cloned())
        .or_else(|| runs.iter().find(|l| l.outstanding() > 0).cloned())
        .or_else(|| runs.first().cloned());

    // Every mutation returns the whole updated list, so the handlers
    // just re-fetch rather than patching a local copy — one source of
    // truth, and the vault page stays authoritative.
    let mut run_action = move |fut: std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ShoppingList, String>>>,
    >| {
        if busy() {
            return;
        }
        busy.set(true);
        spawn(async move {
            match fut.await {
                Ok(_) => lists.restart(),
                Err(e) => {
                    notices.error(format!("Shopping update failed: {e}"));
                }
            }
            busy.set(false);
        });
    };

    rsx! {
        div { class: "fixed inset-0 z-40 flex flex-col bg-background text-foreground",

            // ── Header ───────────────────────────────────────────
            header { class: "flex items-center gap-3 border-b border-border px-3 py-2 pt-[calc(0.5rem+env(safe-area-inset-top,0px))]",
                button {
                    class: "flex size-10 shrink-0 items-center justify-center rounded-lg text-muted-foreground hover:bg-muted hover:text-foreground",
                    aria_label: "Back to mealplan",
                    onclick: move |_| { nav.push(task_plugin_ui::href(crate::APP_ID, "", "")); },
                    ChevronLeft { size: 20 }
                }
                div { class: "flex min-w-0 flex-1 flex-col",
                    Heading {
                        level: HeadingLevel::H2,
                        class: "truncate text-base font-semibold",
                        match &active {
                            Some(l) => l.name.clone(),
                            None => "Shopping".to_string(),
                        }
                    }
                    if let Some(l) = &active {
                        div { class: "text-xs text-muted-foreground",
                            "{l.outstanding()} of {l.entries.len()} still to get"
                        }
                    }
                }
                if let Some(l) = &active {
                    {
                        let id = l.id.to_string();
                        let name = format!("{} — template", l.name);
                        rsx! {
                            Button {
                                variant: ButtonVariant::Ghost,
                                size: ButtonSize::Small,
                                disabled: busy(),
                                on_click: move |_| {
                                    let (s, id, name) = (slug(), id.clone(), name.clone());
                                    run_action(Box::pin(async move {
                                        let s = s.ok_or_else(|| "no org selected".to_string())?;
                                        crate::save_as_template(&s, id, name).await.map_err(|e| e.to_string())
                                    }));
                                },
                                ListChecks { size: 14 }
                                "Keep as template"
                            }
                        }
                    }
                }
            }

            // ── Pass rail ────────────────────────────────────────
            nav {
                class: "flex gap-1.5 border-b border-border px-3 py-2",
                aria_label: "Shopping passes",
                for p in [Pass::Kitchen, Pass::Store] {
                    {
                        let current = pass() == p;
                        let cls = if current {
                            "border-primary bg-primary/15 text-foreground"
                        } else {
                            "border-border text-muted-foreground hover:bg-muted"
                        };
                        rsx! {
                            button {
                                key: "{p.label()}",
                                class: "inline-flex min-h-[36px] flex-1 items-center justify-center gap-1.5 rounded-full border px-3 py-1.5 text-sm font-medium transition-colors {cls}",
                                aria_current: if current { "step" },
                                onclick: move |_| pass.set(p),
                                if p == Pass::Kitchen { Check { size: 14 } } else { Store { size: 14 } }
                                "{p.label()}"
                            }
                        }
                    }
                }
            }

            // ── Body ─────────────────────────────────────────────
            div { class: "flex-1 overflow-y-auto px-3 pb-6 pt-3",
                div { class: "mx-auto flex w-full max-w-2xl flex-col gap-5",
                    match &active {
                        None => rsx! {
                            task_ui_core::states::EmptyState {
                                title: "No shopping list yet",
                                hint: "Start one from a template below, or add a recipe's missing ingredients from its cook page.",
                            }
                        },
                        Some(list) => {
                            let entries: Vec<ShoppingEntry> = list.entries.iter().cloned().collect();
                            let list_id = list.id.to_string();
                            // Kitchen shows everything still outstanding;
                            // the store shows the same set, because a row
                            // only leaves it by being bought.
                            let rows: Vec<ShoppingEntry> = entries
                                .iter()
                                .filter(|e| e.status == EntryStatus::Needed)
                                .cloned()
                                .collect();
                            let settled: Vec<ShoppingEntry> = entries
                                .iter()
                                .filter(|e| e.is_settled())
                                .cloned()
                                .collect();
                            rsx! {
                                if rows.is_empty() {
                                    div { class: "rounded-xl border border-success/40 bg-success/10 px-4 py-6 text-center",
                                        Text { class: "text-sm font-medium text-success", "Everything's accounted for." }
                                    }
                                } else {
                                    section { class: "flex flex-col gap-2",
                                        Heading {
                                            level: HeadingLevel::H3,
                                            class: "text-sm font-semibold uppercase tracking-wide text-muted-foreground",
                                            if pass() == Pass::Kitchen { "Check the shelves" } else { "Still to buy" }
                                        }
                                        div { class: "flex flex-col divide-y divide-border/50 overflow-hidden rounded-xl border border-border bg-card/40",
                                            for e in rows.iter() {
                                                {
                                                    let entry_id = e.id.to_string();
                                                    let lid = list_id.clone();
                                                    let qty = qty_label(e);
                                                    let name = e.name.clone();
                                                    let note = e.note.clone();
                                                    let kitchen = pass() == Pass::Kitchen;
                                                    rsx! {
                                                        button {
                                                            key: "{e.id}",
                                                            class: "flex min-h-[52px] items-center gap-3 px-3 py-2 text-left transition-colors hover:bg-muted/40",
                                                            disabled: busy(),
                                                            onclick: move |_| {
                                                                let (s, lid, entry_id) = (slug(), lid.clone(), entry_id.clone());
                                                                run_action(Box::pin(async move {
                                                                    let s = s.ok_or_else(|| "no org selected".to_string())?;
                                                                    if kitchen {
                                                                        crate::mark_have(&s, lid, entry_id, true).await
                                                                    } else {
                                                                        crate::mark_purchased(&s, lid, entry_id).await
                                                                    }
                                                                    .map_err(|e| e.to_string())
                                                                }));
                                                            },
                                                            span { class: "flex size-6 shrink-0 items-center justify-center rounded-md border border-border" }
                                                            span { class: "flex min-w-0 flex-1 flex-col",
                                                                span { class: "text-sm text-foreground",
                                                                    if !qty.is_empty() {
                                                                        span { class: "font-medium", "{qty} " }
                                                                    }
                                                                    "{name}"
                                                                }
                                                                if let Some(n) = note {
                                                                    span { class: "text-xs text-muted-foreground", "{n}" }
                                                                }
                                                            }
                                                            span { class: "shrink-0 text-xs text-muted-foreground",
                                                                if kitchen { "I have it" } else { "In the basket" }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }

                                // Settled rows, so a mistake is undoable.
                                if !settled.is_empty() {
                                    details { class: "rounded-xl border border-border bg-card/30 px-3 py-2",
                                        summary { class: "cursor-pointer text-xs text-muted-foreground",
                                            "Done — {settled.len()} row(s)"
                                        }
                                        ul { class: "mt-2 flex flex-col gap-1",
                                            for e in settled.iter() {
                                                {
                                                    let entry_id = e.id.to_string();
                                                    let lid = list_id.clone();
                                                    let name = e.name.clone();
                                                    let had = e.status == EntryStatus::Have;
                                                    rsx! {
                                                        li { key: "{e.id}", class: "flex items-center justify-between gap-3 text-sm",
                                                            span { class: "flex items-center gap-2 text-muted-foreground line-through",
                                                                Check { size: 13 }
                                                                "{name}"
                                                            }
                                                            span { class: "flex items-center gap-2",
                                                                span { class: "text-xs text-muted-foreground",
                                                                    if had { "had it" } else { "bought" }
                                                                }
                                                                // Only an "I have it" tick is safely
                                                                // undoable here — undoing a purchase
                                                                // would have to un-restock the pantry.
                                                                if had {
                                                                    button {
                                                                        class: "flex size-6 items-center justify-center rounded-full text-muted-foreground hover:bg-muted hover:text-foreground",
                                                                        aria_label: "Put back on the list",
                                                                        disabled: busy(),
                                                                        onclick: move |_| {
                                                                            let (s, lid, entry_id) = (slug(), lid.clone(), entry_id.clone());
                                                                            run_action(Box::pin(async move {
                                                                                let s = s.ok_or_else(|| "no org selected".to_string())?;
                                                                                crate::mark_have(&s, lid, entry_id, false).await.map_err(|e| e.to_string())
                                                                            }));
                                                                        },
                                                                        X { size: 13 }
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
                    }

                    // ── Templates ────────────────────────────────
                    section { class: "flex flex-col gap-2",
                        Heading {
                            level: HeadingLevel::H3,
                            class: "text-sm font-semibold uppercase tracking-wide text-muted-foreground",
                            "Start from a template"
                        }
                        if templates.is_empty() {
                            Text {
                                variant: TextVariant::Muted,
                                class: "text-xs",
                                "No templates yet — build a list, then \"Keep as template\" to reuse it."
                            }
                        } else {
                            div { class: "flex flex-col gap-2",
                                for t in templates.iter() {
                                    {
                                        let tid = t.id.to_string();
                                        let name = t.name.clone();
                                        let run_name = t.name.clone();
                                        let count = t.entries.len();
                                        rsx! {
                                            div { key: "{t.id}", class: "flex items-center gap-3 rounded-xl border border-border bg-card/40 px-3 py-2",
                                                div { class: "flex min-w-0 flex-1 flex-col",
                                                    span { class: "truncate text-sm text-foreground", "{name}" }
                                                    span { class: "text-xs text-muted-foreground", "{count} item(s)" }
                                                }
                                                Button {
                                                    size: ButtonSize::Small,
                                                    disabled: busy(),
                                                    on_click: move |_| {
                                                        let (s, tid, run_name) = (slug(), tid.clone(), run_name.clone());
                                                        run_action(Box::pin(async move {
                                                            let s = s.ok_or_else(|| "no org selected".to_string())?;
                                                            let started = crate::start_from_template(&s, tid, run_name)
                                                                .await
                                                                .map_err(|e| e.to_string())?;
                                                            selected.set(Some(started.id));
                                                            pass.set(Pass::Kitchen);
                                                            Ok(started)
                                                        }));
                                                    },
                                                    Plus { size: 14 }
                                                    "Start"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // ── Other runs ───────────────────────────────
                    if runs.len() > 1 {
                        section { class: "flex flex-col gap-2",
                            Heading {
                                level: HeadingLevel::H3,
                                class: "text-sm font-semibold uppercase tracking-wide text-muted-foreground",
                                "Other lists"
                            }
                            div { class: "flex flex-wrap gap-2",
                                for l in runs.iter() {
                                    {
                                        let id = l.id;
                                        let name = l.name.clone();
                                        let open = l.outstanding();
                                        let current = active.as_ref().is_some_and(|a| a.id == id);
                                        let cls = if current {
                                            "border-primary bg-primary/15 text-foreground"
                                        } else {
                                            "border-border text-muted-foreground hover:bg-muted"
                                        };
                                        rsx! {
                                            button {
                                                key: "{id}",
                                                class: "inline-flex min-h-[36px] items-center gap-1.5 rounded-full border px-3 py-1.5 text-sm {cls}",
                                                onclick: move |_| selected.set(Some(id)),
                                                "{name}"
                                                span { class: "text-xs text-muted-foreground", "({open})" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // ── Footer ───────────────────────────────────────────
            if let Some(l) = &active {
                {
                    let id = l.id.to_string();
                    let reset_id = id.clone();
                    rsx! {
                        div { class: "border-t border-border bg-card/40 px-3 py-2 pb-[calc(0.5rem+env(safe-area-inset-bottom,0px))]",
                            div { class: "mx-auto flex w-full max-w-2xl items-center gap-2",
                                Button {
                                    variant: ButtonVariant::Ghost,
                                    size: ButtonSize::Small,
                                    disabled: busy(),
                                    on_click: move |_| {
                                        let (s, id) = (slug(), reset_id.clone());
                                        run_action(Box::pin(async move {
                                            let s = s.ok_or_else(|| "no org selected".to_string())?;
                                            crate::reset_shopping_list(&s, id).await.map_err(|e| e.to_string())
                                        }));
                                    },
                                    RotateCcw { size: 14 }
                                    "Start over"
                                }
                                div { class: "flex-1" }
                                Button {
                                    variant: ButtonVariant::Secondary,
                                    size: ButtonSize::Small,
                                    disabled: busy(),
                                    on_click: move |_| {
                                        let (s, id) = (slug(), id.clone());
                                        run_action(Box::pin(async move {
                                            let s = s.ok_or_else(|| "no org selected".to_string())?;
                                            crate::add_low_stock(&s, id).await.map_err(|e| e.to_string())
                                        }));
                                    },
                                    CirclePlus { size: 14 }
                                    "Add low stock"
                                }
                                if pass() == Pass::Kitchen {
                                    Button {
                                        size: ButtonSize::Small,
                                        on_click: move |_| pass.set(Pass::Store),
                                        ShoppingCart { size: 14 }
                                        "To the store"
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

/// Add everything `recipe_path` needs but the pantry can't cover to the
/// org's working shopping list, creating that list on first use.
///
/// "The working list" is the first non-template run — the one
/// [`ShoppingView`] opens by default. Cook mode calls this so a failed
/// pantry check turns into a shopping run in one tap, rather than
/// making you copy the shortages by hand.
pub async fn add_recipe_shortages(
    slug: &str,
    recipe_path: String,
    servings: u32,
) -> Result<ShoppingList, String> {
    let target = working_list(slug).await?;
    crate::add_missing_for_recipe(slug, target.id.to_string(), recipe_path, servings)
        .await
        .map_err(|e| e.to_string())
}

/// Put *everything* `recipe_path` calls for onto the working list — the
/// gather checklist. Unlike [`add_recipe_shortages`] this doesn't
/// consult the pantry, so the kitchen pass is a real look at a real
/// shelf and whatever doesn't turn up becomes the shopping.
pub async fn add_recipe_gather_list(
    slug: &str,
    recipe_path: String,
    servings: u32,
) -> Result<ShoppingList, String> {
    let target = working_list(slug).await?;
    crate::add_recipe_ingredients(slug, target.id.to_string(), recipe_path, servings)
        .await
        .map_err(|e| e.to_string())
}

/// The org's working run — the first non-template list, which is what
/// [`ShoppingView`] opens by default. Created on first use.
async fn working_list(slug: &str) -> Result<ShoppingList, String> {
    let existing = crate::fetch_shopping_lists(slug)
        .await
        .map_err(|e| e.to_string())?;
    match existing.into_iter().find(|l| !l.is_template) {
        Some(l) => Ok(l),
        None => crate::create_shopping_list(slug, new_list_draft("Shopping"))
            .await
            .map_err(|e| e.to_string()),
    }
}

/// An empty list draft. The backend assigns the vault `path` and
/// stamps the dates, so those stay blank here.
#[must_use]
pub fn new_list_draft(name: &str) -> ShoppingList {
    ShoppingList {
        path: String::new(),
        id: uuid::Uuid::nil(),
        name: name.to_string(),
        store_location_id: None,
        entries: mealplan_proto::ShoppingEntries::default(),
        is_template: false,
        from_template: None,
        date_created: None,
        date_modified: None,
        details: String::new(),
    }
}

/// The quantity prefix for a row — empty when the entry carries none,
/// so a free-text "bread" reads as just "bread".
fn qty_label(e: &ShoppingEntry) -> String {
    match (e.qty, e.unit.as_str()) {
        (None, "") => String::new(),
        (None, u) => u.to_string(),
        (Some(q), "") => fmt_num(q),
        (Some(q), u) => format!("{} {u}", fmt_num(q)),
    }
}

fn fmt_num(v: f64) -> String {
    if v.fract().abs() < 1e-9 {
        format!("{}", v.round() as i64)
    } else {
        let s = format!("{v:.2}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}
