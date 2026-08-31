//! Cook mode — a full-screen, step-by-step cook-along for one recipe.
//!
//! Built straight off the structured [`cookbook_proto::Recipe`] the
//! parser now produces: ingredient/cookware/timer names are inlined in
//! each step's text, and every `~{…}` timer is a [`RecipeTimer`] with a
//! second count. So a step that reads "steep the tea for 4 minutes"
//! carries a one-tap **Start 4:00** countdown — no math, no leaving the
//! page. Multiple timers run at once; each fires a tray notice (and a
//! short beep on web) when it lands.
//!
//! Mobile-first: a fixed full-screen sheet, fat tap targets, the
//! running timers pinned under the header so they stay visible while
//! you scroll the steps.

use std::collections::HashSet;
use task_ui_core::format::duration_hms;

use architect_ui::lucide_dioxus::{
    Check, CircleCheck, Clock, Flame, Play, Receipt, ShoppingCart, TriangleAlert, Users,
    UtensilsCrossed, X,
};
use architect_ui::prelude::*;
use cookbook_proto::{Recipe, RecipeTimer};
use dioxus::prelude::*;
use mealplan_proto::{CookReceipt, Fulfillment, SkipReason};

use task_ui_core::orgs::{OrgMeta, OrgSelection};

/// A countdown started from a step's timer. `remaining` ticks down once
/// a second; at zero it's `done` and stays pinned until dismissed.
#[derive(Clone, PartialEq)]
struct RunningTimer {
    id: u64,
    label: String,
    total: u32,
    remaining: u32,
}

/// One stage of the walkthrough. The cook works through these in order:
/// gather everything, then one phase per cooklang `= Section` — so a
/// recipe written with `= Prep` / `= Cook` walks as Gather → Prep →
/// Cook. A recipe with no headings collapses to a single "Steps" phase,
/// which reads exactly like the old flat list.
#[derive(Clone, PartialEq)]
enum Phase {
    /// The ingredient checklist.
    Gather,
    /// A named run of steps — `label` plus indices into `cook_steps`.
    Steps { label: String, steps: Vec<usize> },
}

impl Phase {
    fn label(&self) -> &str {
        match self {
            Phase::Gather => "Gather",
            Phase::Steps { label, .. } => label,
        }
    }
}

/// Split a recipe into walkthrough phases: the gather list (when the
/// recipe has ingredients) followed by one phase per consecutive run of
/// steps sharing a section name.
fn build_phases(recipe: &Recipe) -> Vec<Phase> {
    let mut out = Vec::new();
    if !recipe.ingredients.is_empty() {
        out.push(Phase::Gather);
    }

    let mut current: Option<(Option<String>, Vec<usize>)> = None;
    for (i, step) in recipe.cook_steps.iter().enumerate() {
        match &mut current {
            Some((name, idxs)) if *name == step.section => idxs.push(i),
            _ => {
                if let Some((name, steps)) = current.take() {
                    out.push(Phase::Steps {
                        label: name.unwrap_or_else(|| "Steps".to_string()),
                        steps,
                    });
                }
                current = Some((step.section.clone(), vec![i]));
            }
        }
    }
    if let Some((name, steps)) = current {
        out.push(Phase::Steps {
            label: name.unwrap_or_else(|| "Steps".to_string()),
            steps,
        });
    }
    out
}

#[component]
pub fn CookMode(recipe: Recipe, on_close: EventHandler<()>) -> Element {
    let nav_to_shopping = use_navigator();
    let mut timers = use_signal(Vec::<RunningTimer>::new);
    let mut next_id = use_signal(|| 0u64);
    let mut gathered = use_signal(HashSet::<usize>::new);
    let mut done_steps = use_signal(HashSet::<usize>::new);
    let notices = architect::use_notifications();

    // Servings scaler — multiplies ingredient quantities (timers are
    // left alone; cooking time isn't proportional to batch size).
    let base_servings = recipe.servings.unwrap_or(1).max(1);
    let mut target_servings = use_signal(move || base_servings);
    let factor = f64::from(target_servings()) / f64::from(base_servings);

    // "Cooked it" → deduct ingredients from the pantry, server-side.
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();
    let slug = use_memo(move || {
        task_ui_core::orgs::selected_slugs(&selection.read(), &org_list.read())
            .into_iter()
            .next()
    });
    let mut deducting = use_signal(|| false);
    // The last cook receipt — the per-ingredient deductions + the
    // ingredients we couldn't touch. Pinned under the header until
    // dismissed so the cook can see what was used and what wasn't.
    let mut receipt = use_signal(|| None::<CookReceipt>);
    // Held in a signal, not captured directly, so `mark_cooked` stays
    // `Copy` — the header and the footer both call it.
    let recipe_path = use_signal(|| recipe.path.clone());
    let mut mark_cooked = move || {
        if deducting() {
            return;
        }
        let Some(s) = slug() else { return };
        let path = recipe_path();
        let servings = target_servings();
        deducting.set(true);
        spawn(async move {
            match crate::cook_recipe(&s, path, servings).await {
                Ok(r) if r.deducted.is_empty() && r.skipped.is_empty() => {
                    notices.info("Nothing to deduct — no matching pantry stock.");
                }
                Ok(r) => {
                    notices.info(format!(
                        "Deducted {} ingredient(s) from the pantry.",
                        r.deducted.len()
                    ));
                    receipt.set(Some(r));
                }
                Err(e) => {
                    notices.error(format!("Pantry update failed: {e}"));
                }
            }
            deducting.set(false);
        });
    };

    // "Can I cook this?" → ask the server (which owns the recipe +
    // pantry + nested-recipe resolution) whether there's enough stock.
    // The result is the full Fulfillment — have/missing + substitution
    // suggestions — rendered as-is; no derivation client-side.
    let mut checking = use_signal(|| false);
    let mut precheck = use_signal(|| None::<Fulfillment>);
    let precheck_path = recipe.path.clone();
    let mut check_pantry = move || {
        if checking() {
            return;
        }
        let Some(s) = slug() else { return };
        let path = precheck_path.clone();
        let servings = target_servings();
        checking.set(true);
        spawn(async move {
            match crate::can_cook(&s, path, servings).await {
                Ok(f) => {
                    precheck.set(Some(f));
                }
                Err(e) => {
                    notices.error(format!("Pantry check failed: {e}"));
                }
            }
            checking.set(false);
        });
    };

    // "Add missing to shopping list" — the shortages the pantry check
    // just reported, appended to the working list (created on first
    // use). Navigates there so the run is ready to walk.
    let mut listing = use_signal(|| false);
    let mut add_to_list = move || {
        if listing() {
            return;
        }
        let Some(s) = slug() else { return };
        let path = recipe_path();
        let servings = target_servings();
        listing.set(true);
        spawn(async move {
            match crate::shopping::add_recipe_shortages(&s, path, servings).await {
                Ok(l) => {
                    notices.info(format!("Added to “{}”.", l.name));
                    nav_to_shopping.push(task_plugin_ui::href(crate::APP_ID, "shopping", ""));
                }
                Err(e) => {
                    notices.error(format!("Couldn't build the shopping list: {e}"));
                }
            }
            listing.set(false);
        });
    };

    // "Make a shopping list" — every ingredient, not just what the
    // pantry thinks is short, so the kitchen pass is a real look at a
    // real shelf.
    let mut gather_list = move || {
        if listing() {
            return;
        }
        let Some(s) = slug() else { return };
        let path = recipe_path();
        let servings = target_servings();
        listing.set(true);
        spawn(async move {
            match crate::shopping::add_recipe_gather_list(&s, path, servings).await {
                Ok(l) => {
                    notices.info(format!("Added to \u{201c}{}\u{201d}.", l.name));
                    nav_to_shopping.push(task_plugin_ui::href(crate::APP_ID, "shopping", ""));
                }
                Err(e) => {
                    notices.error(format!("Couldn't build the shopping list: {e}"));
                }
            }
            listing.set(false);
        });
    };

    // Keep the screen awake while cooking — phones lock mid-recipe
    // otherwise. Acquired on mount, released on unmount; re-acquired on
    // tab refocus (the lock drops when hidden). No-op off the web.
    use_hook(|| set_wake_lock(true));
    use_drop(|| set_wake_lock(false));

    // One ticker for every running timer. Decrement once a second;
    // announce + beep the ones that just hit zero (kept in the list so
    // the "Done" chip stays visible until dismissed).
    use_future(move || async move {
        loop {
            sleep_one_second().await;
            if timers.read().iter().all(|t| t.remaining == 0) {
                continue;
            }
            let mut just_finished: Vec<String> = Vec::new();
            timers.with_mut(|list| {
                for t in list.iter_mut() {
                    if t.remaining > 0 {
                        t.remaining -= 1;
                        if t.remaining == 0 {
                            just_finished.push(t.label.clone());
                        }
                    }
                }
            });
            for label in just_finished {
                notices.info(format!("⏰ {label} — time's up"));
                beep();
            }
        }
    });

    let mut start_timer = move |t: &RecipeTimer| {
        let secs = t.seconds.max(1);
        let label = t
            .name
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "Timer".to_string());
        let id = next_id();
        next_id += 1;
        timers.write().push(RunningTimer {
            id,
            label,
            total: secs,
            remaining: secs,
        });
    };

    let running = timers.read().clone();
    let cooked = receipt.read().clone();
    let prechecked = precheck.read().clone();
    let total_steps = recipe.cook_steps.len().max(recipe.steps.len());
    let done_count = done_steps.read().len();

    // Walkthrough state. `active` is clamped rather than trusted so a
    // recipe that reloads with fewer phases can't strand the cook on an
    // index that no longer exists.
    let mut active = use_signal(|| 0usize);
    let phases = build_phases(&recipe);
    let phase_count = phases.len();
    let active_idx = active().min(phase_count.saturating_sub(1));
    let gathered_now = gathered.read().clone();
    let done_now = done_steps.read().clone();
    let phase_done: Vec<bool> = phases
        .iter()
        .map(|p| match p {
            Phase::Gather => (0..recipe.ingredients.len()).all(|i| gathered_now.contains(&i)),
            Phase::Steps { steps, .. } => steps.iter().all(|i| done_now.contains(i)),
        })
        .collect();
    let current_phase = phases.get(active_idx).cloned();
    let next_label = phases.get(active_idx + 1).map(|p| p.label().to_string());

    rsx! {
        div { class: "fixed inset-0 z-50 flex flex-col bg-background text-foreground",
            // ── Header ───────────────────────────────────────────
            header { class: "flex items-center gap-3 border-b border-border px-3 py-2 pt-[calc(0.5rem+env(safe-area-inset-top,0px))]",
                button {
                    class: "flex size-10 shrink-0 items-center justify-center rounded-lg text-muted-foreground hover:bg-muted hover:text-foreground",
                    aria_label: "Close cook mode",
                    onclick: move |_| on_close.call(()),
                    X { size: 20 }
                }
                div { class: "flex min-w-0 flex-1 flex-col",
                    Heading { level: HeadingLevel::H2, class: "truncate text-base font-semibold", "{recipe.name}" }
                    // (meta line follows)
                    div { class: "flex flex-wrap items-center gap-x-3 gap-y-0.5 text-xs text-muted-foreground",
                        if let Some(s) = recipe.servings {
                            span { class: "inline-flex items-center gap-1", Users { size: 12 } "{s} servings" }
                        }
                        if let Some(p) = recipe.prep_minutes {
                            span { class: "inline-flex items-center gap-1", Clock { size: 12 } "{p}m prep" }
                        }
                        if let Some(c) = recipe.cook_minutes {
                            span { class: "inline-flex items-center gap-1", Flame { size: 12 } "{c}m cook" }
                        }
                        if total_steps > 0 {
                            span { "{done_count}/{total_steps} steps" }
                        }
                    }
                }
                // Can I cook this? → pantry-readiness check (no writes).
                Button {
                    variant: ButtonVariant::Ghost,
                    size: ButtonSize::Small,
                    disabled: checking(),
                    on_click: move |_| check_pantry(),
                    CircleCheck { size: 14 }
                    if checking() { "Checking…" } else { "Can I cook this?" }
                }
                // Cooked it → deduct ingredients from the pantry.
                Button {
                    variant: ButtonVariant::Secondary,
                    size: ButtonSize::Small,
                    disabled: deducting(),
                    on_click: move |_| mark_cooked(),
                    UtensilsCrossed { size: 14 }
                    if deducting() { "Deducting…" } else { "Cooked it" }
                }
            }

            // ── Running timers (pinned) ──────────────────────────
            if !running.is_empty() {
                div { class: "flex gap-2 overflow-x-auto border-b border-border bg-muted/30 px-3 py-2",
                    for t in running.iter() {
                        {
                            let id = t.id;
                            let done = t.remaining == 0;
                            let pill = if done {
                                "border-success/50 bg-success/15 text-success"
                            } else {
                                "border-primary/40 bg-primary/10 text-foreground"
                            };
                            rsx! {
                                div {
                                    key: "{id}",
                                    class: "flex shrink-0 items-center gap-2 rounded-full border px-3 py-1.5 text-sm {pill}",
                                    span { class: "font-medium", "{t.label}" }
                                    span { class: "font-mono tabular-nums", "{duration_hms(t.remaining)}" }
                                    button {
                                        class: "flex size-5 items-center justify-center rounded-full hover:bg-foreground/10",
                                        aria_label: "Dismiss timer",
                                        onclick: move |_| { timers.write().retain(|x| x.id != id); },
                                        X { size: 13 }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // ── Pantry check (pinned after "Can I cook this?") ───
            // Renders the server's Fulfillment verbatim — have / missing
            // partition + substitution suggestions, no client derivation.
            if let Some(f) = prechecked {
                div { class: "border-b border-border bg-card/60 px-3 py-2.5",
                    div { class: "mx-auto flex w-full max-w-2xl flex-col gap-2",
                        div { class: "flex items-center gap-2",
                            if f.can_cook {
                                span { class: "text-success", CircleCheck { size: 15 } }
                            } else {
                                span { class: "text-warning", TriangleAlert { size: 15 } }
                            }
                            Heading {
                                level: HeadingLevel::H3,
                                class: "flex-1 text-sm font-semibold",
                                if f.can_cook { "You've got everything" } else { "Missing some ingredients" }
                            }
                            button {
                                class: "flex size-6 items-center justify-center rounded-full text-muted-foreground hover:bg-muted hover:text-foreground",
                                aria_label: "Dismiss pantry check",
                                onclick: move |_| precheck.set(None),
                                X { size: 14 }
                            }
                        }
                        if !f.missing.is_empty() {
                            ul { class: "flex flex-col gap-1",
                                for s in f.missing.iter() {
                                    li { key: "{s.ingredient_idx}", class: "flex flex-col gap-0.5",
                                        div { class: "flex items-baseline justify-between gap-3 text-sm",
                                            span { class: "text-foreground", "{s.name}" }
                                            span { class: "shrink-0 text-xs text-muted-foreground",
                                                "{s.reason.label()}"
                                            }
                                        }
                                        for sub in s.suggestions.iter() {
                                            div { key: "{sub.name}", class: "pl-3 text-xs text-muted-foreground",
                                                "↳ try {sub.name}"
                                                if let Some(note) = &sub.note {
                                                    " — {note}"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            // Turn the shortages into a shopping run in
                            // one tap, rather than copying them by hand.
                            div { class: "flex justify-end",
                                Button {
                                    variant: ButtonVariant::Secondary,
                                    size: ButtonSize::Small,
                                    disabled: listing(),
                                    on_click: move |_| add_to_list(),
                                    ShoppingCart { size: 14 }
                                    if listing() { "Adding…" } else { "Add missing to shopping list" }
                                }
                            }
                        }
                        if !f.have.is_empty() {
                            details { class: "text-sm",
                                summary { class: "cursor-pointer text-xs text-muted-foreground",
                                    "Have {f.have.len()} ingredient(s)"
                                }
                                ul { class: "mt-1 flex flex-col gap-0.5",
                                    // Index-qualified key: an ingredient can
                                    // appear twice in a recipe.
                                    for (i, h) in f.have.iter().enumerate() {
                                        li { key: "{i}-{h.name}", class: "flex items-baseline justify-between gap-3",
                                            span { class: "text-foreground", "{h.name}" }
                                            if let Some(need) = h.need {
                                                span { class: "shrink-0 font-mono tabular-nums text-muted-foreground",
                                                    "{fmt_num(need)} {h.unit}"
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

            // ── Cook receipt (pinned after "Cooked it") ──────────
            if let Some(r) = cooked {
                div { class: "border-b border-border bg-card/60 px-3 py-2.5",
                    div { class: "mx-auto flex w-full max-w-2xl flex-col gap-2",
                        div { class: "flex items-center gap-2",
                            Receipt { size: 15 }
                            Heading { level: HeadingLevel::H3, class: "flex-1 text-sm font-semibold", "Pantry receipt" }
                            button {
                                class: "flex size-6 items-center justify-center rounded-full text-muted-foreground hover:bg-muted hover:text-foreground",
                                aria_label: "Dismiss receipt",
                                onclick: move |_| receipt.set(None),
                                X { size: 14 }
                            }
                        }
                        if r.deducted.is_empty() {
                            p { class: "text-xs text-muted-foreground", "Nothing was deducted from the pantry." }
                        } else {
                            ul { class: "flex flex-col gap-0.5",
                                for line in r.deducted.iter() {
                                    li { key: "{line.item_id}-{line.ingredient}", class: "flex items-baseline justify-between gap-3 text-sm",
                                        span { class: "text-foreground", "{line.ingredient}" }
                                        span { class: "shrink-0 font-mono tabular-nums text-muted-foreground",
                                            "−{fmt_num(line.qty)} {line.unit}"
                                        }
                                    }
                                }
                            }
                        }
                        if !r.skipped.is_empty() {
                            div { class: "flex flex-col gap-1 rounded-lg border border-warning/40 bg-warning/10 px-2.5 py-2",
                                div { class: "flex items-center gap-1.5 text-xs font-medium text-warning",
                                    TriangleAlert { size: 13 }
                                    "Not deducted — top up by hand"
                                }
                                ul { class: "flex flex-col gap-0.5",
                                    // Index-qualified key: an ingredient can
                                    // appear twice in a recipe.
                                    for (i, skip) in r.skipped.iter().enumerate() {
                                        li { key: "{i}-{skip.ingredient}", class: "flex items-baseline justify-between gap-3 text-sm",
                                            span { class: "text-foreground", "{skip.ingredient}" }
                                            span { class: "shrink-0 text-xs text-muted-foreground", "{skip_reason_label(skip.reason)}" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // ── Phase rail ───────────────────────────────────────
            // Gather → Prep → Cook, straight off the recipe's cooklang
            // sections. Tappable so the cook can jump back mid-cook;
            // a completed phase keeps its tick.
            if phase_count > 1 {
                nav {
                    class: "flex gap-1.5 overflow-x-auto border-b border-border px-3 py-2",
                    aria_label: "Cooking phases",
                    for (pi, ph) in phases.iter().enumerate() {
                        {
                            let label = ph.label().to_string();
                            let complete = phase_done[pi];
                            let current = pi == active_idx;
                            let cls = if current {
                                "border-primary bg-primary/15 text-foreground"
                            } else if complete {
                                "border-success/50 bg-success/10 text-success"
                            } else {
                                "border-border text-muted-foreground hover:bg-muted"
                            };
                            rsx! {
                                button {
                                    key: "{pi}",
                                    class: "inline-flex min-h-[36px] shrink-0 items-center gap-1.5 rounded-full border px-3 py-1.5 text-sm font-medium transition-colors {cls}",
                                    aria_current: if current { "step" },
                                    onclick: move |_| active.set(pi),
                                    if complete { Check { size: 13 } }
                                    "{label}"
                                }
                            }
                        }
                    }
                }
            }

            // ── Scrollable body ──────────────────────────────────
            div { class: "flex-1 overflow-y-auto px-3 pb-6 pt-3",
                div { class: "mx-auto flex w-full max-w-2xl flex-col gap-5",

                    // Ingredients — tap to check off as you gather.
                    if matches!(current_phase, Some(Phase::Gather)) {
                        section { class: "flex flex-col gap-2",
                            // Take the whole ingredient list shopping —
                            // check the shelves there, buy what's missing.
                            // Not the same as the pantry check's
                            // "add missing", which trusts recorded stock.
                            div { class: "flex justify-end",
                                Button {
                                    variant: ButtonVariant::Ghost,
                                    size: ButtonSize::Small,
                                    disabled: listing(),
                                    on_click: move |_| gather_list(),
                                    ShoppingCart { size: 14 }
                                    "Make a shopping list"
                                }
                            }
                            div { class: "flex items-center justify-between gap-3",
                                Heading { level: HeadingLevel::H3, class: "text-sm font-semibold uppercase tracking-wide text-muted-foreground", "Ingredients" }
                                // Servings scaler — only when the recipe declares a yield.
                                if recipe.servings.is_some() {
                                    div { class: "flex items-center gap-1.5 rounded-full border border-border bg-card/60 px-1 py-0.5",
                                        button {
                                            class: "flex size-7 items-center justify-center rounded-full text-muted-foreground hover:bg-muted disabled:opacity-40",
                                            disabled: target_servings() <= 1,
                                            aria_label: "Fewer servings",
                                            onclick: move |_| { let v = target_servings(); if v > 1 { target_servings.set(v - 1); } },
                                            "−"
                                        }
                                        span { class: "min-w-[4.5rem] text-center text-xs tabular-nums text-foreground",
                                            "{target_servings()} servings"
                                        }
                                        button {
                                            class: "flex size-7 items-center justify-center rounded-full text-muted-foreground hover:bg-muted",
                                            aria_label: "More servings",
                                            onclick: move |_| target_servings.set(target_servings() + 1),
                                            "+"
                                        }
                                    }
                                }
                            }
                            div { class: "flex flex-col divide-y divide-border/50 overflow-hidden rounded-xl border border-border bg-card/40",
                                for (i, ing) in recipe.ingredients.iter().enumerate() {
                                    {
                                        let checked = gathered.read().contains(&i);
                                        let qty = scaled_qty(ing, factor);
                                        let name = ing.name.clone();
                                        rsx! {
                                            button {
                                                key: "{i}",
                                                class: "flex min-h-[44px] items-center gap-3 px-3 py-2 text-left transition-colors hover:bg-muted/40",
                                                onclick: move |_| {
                                                    let mut g = gathered.write();
                                                    if !g.insert(i) { g.remove(&i); }
                                                },
                                                span {
                                                    class: if checked {
                                                        "flex size-5 shrink-0 items-center justify-center rounded-md border border-success bg-success text-success-foreground"
                                                    } else {
                                                        "flex size-5 shrink-0 items-center justify-center rounded-md border border-border"
                                                    },
                                                    if checked { Check { size: 13 } }
                                                }
                                                span {
                                                    class: if checked { "text-sm text-muted-foreground line-through" } else { "text-sm text-foreground" },
                                                    if !qty.is_empty() {
                                                        span { class: "font-medium", "{qty} " }
                                                    }
                                                    "{name}"
                                                    if ing.optional {
                                                        span { class: "text-xs text-muted-foreground", " (optional)" }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Steps — each with its inline text + one-tap timers.
                    // Only the active phase's steps render, so the cook
                    // sees prep on its own, then the cook itself.
                    if let Some(Phase::Steps { label, steps }) = &current_phase {
                    section { class: "flex flex-col gap-2",
                        Heading { level: HeadingLevel::H3, class: "text-sm font-semibold uppercase tracking-wide text-muted-foreground", "{label}" }
                        for i in steps.iter().copied() {
                            {
                                let step = &recipe.cook_steps[i];
                                let done = done_steps.read().contains(&i);
                                let card = if done {
                                    "border-border/60 bg-card/30 opacity-60"
                                } else {
                                    "border-border bg-card/60"
                                };
                                rsx! {
                                    div { key: "{i}", class: "flex gap-3 rounded-xl border p-3 {card}",
                                        // Step number doubles as the done toggle.
                                        button {
                                            class: if done {
                                                "flex size-7 shrink-0 items-center justify-center rounded-full bg-success text-success-foreground"
                                            } else {
                                                "flex size-7 shrink-0 items-center justify-center rounded-full border border-border text-sm font-semibold text-muted-foreground hover:border-primary hover:text-foreground"
                                            },
                                            aria_label: "Toggle step done",
                                            onclick: move |_| {
                                                let mut d = done_steps.write();
                                                if !d.insert(i) { d.remove(&i); }
                                            },
                                            if done { Check { size: 15 } } else { "{i + 1}" }
                                        }
                                        div { class: "flex min-w-0 flex-1 flex-col gap-2",
                                            p {
                                                class: if done { "text-sm leading-relaxed text-muted-foreground line-through" } else { "text-sm leading-relaxed text-foreground" },
                                                "{step.text}"
                                            }
                                            if !step.timers.is_empty() {
                                                div { class: "flex flex-wrap gap-2",
                                                    for (ti, timer) in step.timers.iter().enumerate() {
                                                        {
                                                            let t = timer.clone();
                                                            let secs = t.seconds.max(1);
                                                            rsx! {
                                                                button {
                                                                    key: "{ti}",
                                                                    class: "inline-flex min-h-[36px] items-center gap-1.5 rounded-full border border-primary/40 bg-primary/10 px-3 py-1 text-sm font-medium text-primary transition-colors hover:bg-primary/20",
                                                                    onclick: move |_| start_timer(&t),
                                                                    Play { size: 13 }
                                                                    if let Some(n) = &timer.name {
                                                                        if !n.is_empty() {
                                                                            span { "{n} · " }
                                                                        }
                                                                    }
                                                                    span { class: "font-mono tabular-nums", "{duration_hms(secs)}" }
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
                }
            }

            // ── Walkthrough footer ───────────────────────────────
            // The one big control: finish this phase, move to the next.
            // Hidden for single-phase recipes, which have nowhere to go.
            if phase_count > 1 {
                div { class: "border-t border-border bg-card/40 px-3 py-2 pb-[calc(0.5rem+env(safe-area-inset-bottom,0px))]",
                    div { class: "mx-auto flex w-full max-w-2xl items-center gap-2",
                        if active_idx > 0 {
                            Button {
                                variant: ButtonVariant::Secondary,
                                on_click: move |_| active.set(active_idx - 1),
                                "Back"
                            }
                        }
                        div { class: "flex-1" }
                        if let Some(next) = next_label {
                            Button {
                                on_click: move |_| active.set(active_idx + 1),
                                "Next: {next}"
                            }
                        } else {
                            Button {
                                variant: ButtonVariant::Secondary,
                                disabled: deducting(),
                                on_click: move |_| mark_cooked(),
                                UtensilsCrossed { size: 14 }
                                "Done — cooked it"
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The quantity prefix for an ingredient, scaled by `factor`.
///
/// Three things the parser tells us and this has to honour:
///
/// - **Pinned quantities don't move.** `@salt{=1%tsp}` seasons the pan,
///   not each head, so cooklang marks it `scalable: false` and doubling
///   the recipe must leave it alone.
/// - **A range stays a range.** `1-2 tbsp` doubled is `2-4 tbsp`; both
///   ends scale.
/// - **Text quantities are untouchable.** There is no twice-a-pinch, so
///   `qty: None` falls back to the written form.
///
/// At `factor == 1` the result reads exactly as written.
pub(crate) fn scaled_qty(ing: &cookbook_proto::Ingredient, factor: f64) -> String {
    let f = if ing.scalable { factor } else { 1.0 };
    let num = match (ing.qty, ing.qty_max) {
        (Some(lo), Some(hi)) => Some(format!("{}–{}", fmt_num(lo * f), fmt_num(hi * f))),
        (Some(q), None) => Some(fmt_num(q * f)),
        _ => None,
    };
    match num {
        Some(n) if ing.unit.is_empty() => n,
        Some(n) => format!("{n} {}", ing.unit),
        None => match (&ing.qty_display, ing.unit.as_str()) {
            (Some(q), "") => q.clone(),
            (Some(q), u) => format!("{q} {u}"),
            (None, "") => String::new(),
            (None, u) => u.to_string(),
        },
    }
}

/// Human label for why an ingredient wasn't deducted from the pantry.
fn skip_reason_label(reason: SkipReason) -> &'static str {
    match reason {
        SkipReason::NoQuantity => "no quantity given",
        SkipReason::NoPantryMatch => "not in pantry",
        SkipReason::InconvertibleUnit => "unit mismatch",
        SkipReason::OutOfStock => "out of stock",
    }
}

/// Trim a scaled quantity to a tidy form — whole numbers lose the
/// decimal, others keep up to two places without trailing zeros.
pub(crate) fn fmt_num(v: f64) -> String {
    if (v.fract()).abs() < 1e-9 {
        format!("{}", v.round() as i64)
    } else {
        let s = format!("{v:.2}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// Route target for `/mealplan/recipe?:path` — resolves one recipe from
/// the shared cookbook store (cache-first after a `/mealplan` visit)
/// and drops straight into [`CookMode`]. Back closes to `/mealplan`.
#[component]
pub fn RecipeCookView(path: String) -> Element {
    let nav = use_navigator();
    let recipes = crate::use_recipe_list();
    let store = crate::use_recipe_store();
    let target = path.clone();
    let found = recipes.value().and_then(|rows| {
        rows.iter()
            .find(|(_, r)| r.path == target)
            .map(|(_, r)| r.clone())
    });

    let to_mealplan = move |()| {
        nav.push(task_plugin_ui::href(crate::APP_ID, "", ""));
    };

    match found {
        Some(recipe) => rsx! {
            CookMode { recipe, on_close: to_mealplan }
        },
        None if recipes.is_waiting() => rsx! {
            div { class: "flex h-full items-center justify-center p-8",
                task_ui_core::states::LoadingState {}
            }
        },
        None => rsx! {
            div { class: "mx-auto flex max-w-md flex-col gap-3 p-8 text-center",
                if let Some(err) = recipes.error() {
                    task_ui_core::states::ErrorState {
                        title: "Couldn't load the recipe",
                        message: err.clone(),
                        on_retry: move |()| store.reload(),
                    }
                } else {
                    task_ui_core::states::EmptyState {
                        title: "Recipe not found",
                        hint: "It may have been moved or renamed.",
                    }
                }
                Button {
                    variant: ButtonVariant::Secondary,
                    on_click: move |_| to_mealplan(()),
                    "Back to mealplan"
                }
            }
        },
    }
}

/// A short two-tone chime when a timer lands. Web Audio only; a no-op
/// off the web (the tray notice still fires everywhere).
#[cfg(target_arch = "wasm32")]
fn beep() {
    let _ = dioxus::document::eval(
        "try{const c=new (window.AudioContext||window.webkitAudioContext)();\
         const o=c.createOscillator();const g=c.createGain();o.connect(g);g.connect(c.destination);\
         o.type='sine';o.frequency.value=880;g.gain.setValueAtTime(0.001,c.currentTime);\
         g.gain.exponentialRampToValueAtTime(0.3,c.currentTime+0.02);\
         g.gain.exponentialRampToValueAtTime(0.001,c.currentTime+0.6);\
         o.start();o.stop(c.currentTime+0.6);}catch(e){}",
    );
}

#[cfg(not(target_arch = "wasm32"))]
fn beep() {}

/// Hold (or drop) a screen wake lock for the cook session. Uses the Web
/// Wake Lock API; the lock self-drops when the tab is hidden, so a
/// one-time `visibilitychange` hook re-acquires it on refocus while the
/// session is still active.
#[cfg(target_arch = "wasm32")]
fn set_wake_lock(on: bool) {
    let js = if on {
        "window.__cookActive=true;\
         (async()=>{try{window.__cookWL=await navigator.wakeLock.request('screen');}catch(e){}})();\
         if(!window.__cookWLHooked){window.__cookWLHooked=true;\
           document.addEventListener('visibilitychange',async()=>{\
             if(document.visibilityState==='visible'&&window.__cookActive){\
               try{window.__cookWL=await navigator.wakeLock.request('screen');}catch(e){}}});}"
    } else {
        "window.__cookActive=false;\
         try{if(window.__cookWL){window.__cookWL.release();window.__cookWL=null;}}catch(e){}"
    };
    let _ = dioxus::document::eval(js);
}

#[cfg(not(target_arch = "wasm32"))]
fn set_wake_lock(_on: bool) {}

/// One-second tick for the cook-mode timers. `architect::sleep` is the
/// cross-platform sleep (gloo-timers on wasm, tokio on native), so the
/// countdown advances on the desktop/mobile builds too — not just the web.
async fn sleep_one_second() {
    architect::sleep(architect::Duration::from_secs(1)).await;
}
