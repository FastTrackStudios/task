//! `/mealplan` — the food dashboard.
//!
//! Pulls the three mealplan sub-domains for the selected org into
//! a single sectioned page:
//!
//! - **Recipes** — the cookbook (`<wiki>/Cookbook/*.cook`). Minimal
//!   create form (name → seeds a `>> title:` cooklang stub).
//! - **Pantry** — food-on-hand. Minimal create form (name + qty +
//!   unit).
//! - **Meal Plan** — planned meals on the calendar (date + slot +
//!   status).
//!
//! Editing, cooking (pantry deduction), and nutrition resolution
//! live in the CLI for now; this is the read + light-create slice,
//! mirroring the `/fitness` page. Recipes + Pantry state is the shared
//! optimistic store ([`crate::stores`]): one `AtomResult` per section,
//! typed `Id::Temp` rows for in-flight creates (no `__pending__` path
//! sentinels), rollback + tray notification on failure.

use architect::Id;
use dioxus::prelude::*;
use architect_ui::lucide_dioxus::ShoppingCart;
use architect_ui::prelude::*;
use uuid::Uuid;

use cookbook_proto::Recipe;
use mealplan_proto::Meal;
use pantry_proto::PantryItem;

use crate::orgs::{OrgMeta, OrgSelection};
use crate::stores;

const INPUT_CLS: &str = "rounded-lg border border-input bg-input/30 px-3 py-2 text-sm transition-colors \
     focus-visible:border-ring focus-visible:outline-none focus-visible:ring-[3px] \
     focus-visible:ring-ring/50 placeholder:text-muted-foreground";

#[component]
pub fn MealplanView() -> Element {
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();

    // The org we list / create into (first selected, or home).
    let slug = use_memo(move || {
        crate::orgs::selected_slugs(&selection.read(), &org_list.read())
            .into_iter()
            .next()
    });

    rsx! {
        div { class: "mx-auto flex max-w-3xl flex-col gap-8 p-4 sm:p-6 lg:p-10",
            div { class: "flex items-start gap-3",
                div { class: "flex flex-1 flex-col gap-1",
                    Heading { level: HeadingLevel::H1, "Mealplan" }
                    Text {
                        variant: TextVariant::Muted,
                        class: "text-sm",
                        "The cookbook, what's in the pantry, and the meals on the calendar.",
                    }
                }
                Link {
                    to: crate::routes::Route::ShoppingRoute {},
                    class: "inline-flex min-h-[36px] shrink-0 items-center gap-1.5 rounded-full border border-border px-3 py-1.5 text-sm font-medium text-foreground hover:bg-muted",
                    ShoppingCart { size: 14 }
                    "Shopping"
                }
            }

            super::mealplan_week::MealWeek { slug }
            RecipesSection { slug }
            PantrySection { slug }
            MealPlanSection { slug }
        }
    }
}

/// Shared section header — title + a count chip.
#[component]
fn SectionHeader(title: String, count: usize) -> Element {
    rsx! {
        div { class: "flex items-center justify-between gap-3",
            Heading { level: HeadingLevel::H2, "{title}" }
            Text { variant: TextVariant::Muted, class: "text-sm", "{count}" }
        }
    }
}

/// Inline error banner for a failed section load.
#[component]
fn LoadError(what: String, err: String) -> Element {
    rsx! {
        div { class: "rounded-lg border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive",
            "Couldn't load {what}: {err}"
        }
    }
}

/// Empty-state placeholder for a section with no rows.
#[component]
fn EmptyState(message: String) -> Element {
    rsx! {
        div { class: "rounded-lg border border-dashed border-border px-4 py-6 text-center",
            Text { variant: TextVariant::Muted, class: "text-sm", "{message}" }
        }
    }
}

// ── Recipes ─────────────────────────────────────────────────────────

#[component]
fn RecipesSection(slug: Memo<Option<String>>) -> Element {
    let mut name = use_signal(String::new);

    // The shared store: one AtomResult for the list, optimistic create.
    // The store keys in-flight drafts by a typed `Id::Temp` — no magic
    // `__pending__` path sentinel.
    let result = stores::use_recipe_list();
    let muts = stores::use_recipe_mutations();

    let mut create = move || {
        let n = name.read().trim().to_string();
        if n.is_empty() {
            return;
        }
        let Some(s) = slug() else { return };
        name.set(String::new());
        muts.create(s, stores::draft_recipe(n));
    };

    // Web import: the server fetches + extracts + synthesizes a `.cook`
    // draft, which we then save through the normal optimistic create.
    let mut import_url = use_signal(String::new);
    let mut importing = use_signal(|| false);
    let notices = architect::use_notifications();
    let mut do_import = move || {
        let u = import_url.read().trim().to_string();
        if u.is_empty() || importing() {
            return;
        }
        let Some(s) = slug() else { return };
        importing.set(true);
        spawn(async move {
            match crate::feeds::import_recipe(&s, u).await {
                Ok(draft) => {
                    let name = draft.name.clone();
                    import_url.set(String::new());
                    muts.create(s, draft);
                    notices.info(format!("Imported “{name}” — review, edit, or cook it."));
                }
                Err(e) => {
                    notices.error(format!("Import failed: {e}"));
                }
            }
            importing.set(false);
        });
    };

    let rows: Vec<(Id<String>, Recipe)> = result.value().cloned().unwrap_or_default();
    let load_err = result.error().cloned();

    rsx! {
        section { class: "flex flex-col gap-3",
            SectionHeader { title: "Recipes".to_string(), count: rows.len() }

            div { class: "flex flex-col gap-2 rounded-xl border border-border bg-card/40 p-3 sm:flex-row sm:items-center",
                input {
                    class: "{INPUT_CLS} flex-1",
                    placeholder: "Recipe name (e.g. Truffle Pasta)…",
                    value: "{name}",
                    oninput: move |e| name.set(e.value()),
                    onkeydown: move |e| {
                        if e.key() == Key::Enter {
                            create();
                        }
                    },
                }
                Button { variant: ButtonVariant::Primary, on_click: move |_| create(), "Add" }
            }

            // Import from a recipe URL.
            div { class: "flex flex-col gap-2 rounded-xl border border-dashed border-border bg-card/20 p-3 sm:flex-row sm:items-center",
                input {
                    class: "{INPUT_CLS} flex-1",
                    r#type: "url",
                    placeholder: "Paste a recipe URL to import…",
                    value: "{import_url}",
                    disabled: importing(),
                    oninput: move |e| import_url.set(e.value()),
                    onkeydown: move |e| {
                        if e.key() == Key::Enter {
                            do_import();
                        }
                    },
                }
                Button {
                    variant: ButtonVariant::Secondary,
                    disabled: importing(),
                    on_click: move |_| do_import(),
                    if importing() { "Importing…" } else { "Import" }
                }
            }

            if let Some(err) = load_err {
                LoadError { what: "recipes".to_string(), err }
            }

            if rows.is_empty() {
                EmptyState { message: "No recipes yet — add one above.".to_string() }
            } else {
                div { class: "flex flex-col gap-2",
                    for (id, r) in rows {
                        RecipeRow { key: "{id}", pending: id.is_temp(), recipe: r }
                    }
                }
            }
        }
    }
}

/// One recipe: name + course badge + ingredient count. `pending` dims
/// an optimistic row whose write-through is in flight; failures roll
/// back + notify.
#[component]
fn RecipeRow(recipe: Recipe, pending: bool) -> Element {
    let nav = use_navigator();
    let name = recipe.name.clone();
    let course = recipe.course.clone();
    let ingredients = recipe.ingredients.len();
    let steps = recipe.cook_steps.len().max(recipe.steps.len());
    let path = recipe.path.clone();

    let state_cls = if pending {
        "border-border bg-card/40 opacity-60"
    } else {
        "border-border bg-card/40"
    };

    rsx! {
        div { class: "flex items-center gap-3 rounded-lg border px-3 py-2 {state_cls}",
            div { class: "flex min-w-0 flex-1 flex-col gap-0.5",
                Text { class: "break-words text-sm font-medium", "{name}" }
                span { class: "text-[11px] text-muted-foreground", "{ingredients} ingredients · {steps} steps" }
            }
            if let Some(c) = course {
                span { class: "shrink-0 rounded bg-muted px-1.5 py-px text-[11px] text-muted-foreground", "{c}" }
            }
            if !pending {
                div { class: "flex shrink-0 items-center gap-1.5",
                    // Author the cooklang source.
                    Button {
                        variant: ButtonVariant::Ghost,
                        size: ButtonSize::Small,
                        on_click: {
                            let path = path.clone();
                            move |_| { nav.push(crate::routes::Route::RecipeEditRoute { path: path.clone() }); }
                        },
                        "Edit"
                    }
                    // Cook-along: deep-links into the full-screen step +
                    // timer view (only once there are parsed steps).
                    if steps > 0 {
                        Button {
                            variant: ButtonVariant::Secondary,
                            size: ButtonSize::Small,
                            on_click: move |_| {
                                nav.push(crate::routes::Route::RecipeReadRoute { path: path.clone() });
                            },
                            "Cook"
                        }
                    }
                }
            }
        }
    }
}

// ── Pantry ──────────────────────────────────────────────────────────

#[component]
fn PantrySection(slug: Memo<Option<String>>) -> Element {
    let mut name = use_signal(String::new);
    let mut qty = use_signal(String::new);
    let mut unit = use_signal(|| "g".to_string());

    // The shared store: one AtomResult for the list, optimistic create.
    let result = stores::use_pantry_list();
    let muts = stores::use_pantry_mutations();

    let mut create = move || {
        let n = name.read().trim().to_string();
        if n.is_empty() {
            return;
        }
        let Some(s) = slug() else { return };
        let q = qty.read().trim().parse::<f64>().ok();
        let u = unit.read().trim().to_string();
        name.set(String::new());
        qty.set(String::new());
        muts.create(s, stores::draft_pantry_item(n, q, u));
    };

    let rows: Vec<(Id<Uuid>, PantryItem)> = result.value().cloned().unwrap_or_default();
    let load_err = result.error().cloned();

    rsx! {
        section { class: "flex flex-col gap-3",
            SectionHeader { title: "Pantry".to_string(), count: rows.len() }

            div { class: "flex flex-col gap-2 rounded-xl border border-border bg-card/40 p-3 sm:flex-row sm:items-center",
                input {
                    class: "{INPUT_CLS} flex-1",
                    placeholder: "Item name (e.g. Flour)…",
                    value: "{name}",
                    oninput: move |e| name.set(e.value()),
                    onkeydown: move |e| {
                        if e.key() == Key::Enter {
                            create();
                        }
                    },
                }
                input {
                    class: "{INPUT_CLS} w-24",
                    placeholder: "qty",
                    value: "{qty}",
                    oninput: move |e| qty.set(e.value()),
                    onkeydown: move |e| {
                        if e.key() == Key::Enter {
                            create();
                        }
                    },
                }
                input {
                    class: "{INPUT_CLS} w-20",
                    placeholder: "unit",
                    value: "{unit}",
                    oninput: move |e| unit.set(e.value()),
                    onkeydown: move |e| {
                        if e.key() == Key::Enter {
                            create();
                        }
                    },
                }
                Button { variant: ButtonVariant::Primary, on_click: move |_| create(), "Add" }
            }

            if let Some(err) = load_err {
                LoadError { what: "pantry items".to_string(), err }
            }

            if rows.is_empty() {
                EmptyState { message: "No pantry items yet — add one above.".to_string() }
            } else {
                div { class: "flex flex-col gap-2",
                    for (id, it) in rows {
                        PantryRow { key: "{id}", pending: id.is_temp(), item: it }
                    }
                }
            }
        }
    }
}

/// One pantry item: name + on-hand qty + food category badge.
/// `pending` dims an optimistic row whose write-through is in flight;
/// failures roll back + notify.
#[component]
fn PantryRow(item: PantryItem, pending: bool) -> Element {
    let name = item.name.clone();
    let unit = item.unit.clone();
    let on_hand = item.stock_total();
    let category = item.food_category.clone();

    let state_cls = if pending {
        "border-border bg-card/40 opacity-60"
    } else {
        "border-border bg-card/40"
    };

    rsx! {
        div { class: "flex items-center gap-3 rounded-lg border px-3 py-2 {state_cls}",
            div { class: "flex min-w-0 flex-1 flex-col gap-0.5",
                Text { class: "break-words text-sm font-medium", "{name}" }
                if let Some(q) = on_hand {
                    span { class: "text-[11px] text-muted-foreground", "{q} {unit}" }
                } else {
                    span { class: "text-[11px] text-muted-foreground", "no qty recorded" }
                }
            }
            if !category.is_empty() {
                span { class: "shrink-0 rounded bg-muted px-1.5 py-px text-[11px] text-muted-foreground", "{category}" }
            }
        }
    }
}

// ── Meal Plan ───────────────────────────────────────────────────────

#[component]
fn MealPlanSection(slug: Memo<Option<String>>) -> Element {
    let meals = use_resource(move || async move {
        match slug() {
            Some(s) => crate::feeds::fetch_meal_plans(&s).await,
            None => Ok(Vec::new()),
        }
    });

    let (rows, load_err): (Vec<Meal>, Option<String>) = match &*meals.read() {
        Some(Ok(all)) => (all.clone(), None),
        Some(Err(e)) => (Vec::new(), Some(e.clone())),
        None => (Vec::new(), None),
    };

    rsx! {
        section { class: "flex flex-col gap-3",
            SectionHeader { title: "Meal Plan".to_string(), count: rows.len() }

            if let Some(err) = load_err {
                LoadError { what: "meal plan".to_string(), err }
            }

            if rows.is_empty() {
                EmptyState { message: "No planned meals yet.".to_string() }
            } else {
                div { class: "flex flex-col gap-2",
                    for m in rows {
                        MealRow { key: "{m.id}", meal: m }
                    }
                }
            }
        }
    }
}

/// One planned meal: name + date + slot + status badge.
#[component]
fn MealRow(meal: Meal) -> Element {
    let name = meal.name.clone();
    let date = meal.scheduled_for;
    let slot = meal.slot.clone();
    let status = meal.status.clone();

    rsx! {
        div { class: "flex items-center gap-3 rounded-lg border border-border bg-card/40 px-3 py-2",
            div { class: "flex min-w-0 flex-1 flex-col gap-0.5",
                Text { class: "break-words text-sm font-medium", "{name}" }
                span { class: "text-[11px] text-muted-foreground", "{date} · {slot}" }
            }
            span { class: "shrink-0 rounded bg-muted px-1.5 py-px text-[11px] text-muted-foreground", "{status}" }
        }
    }
}
