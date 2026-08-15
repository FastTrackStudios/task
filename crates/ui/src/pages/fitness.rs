//! `/fitness` — the training dashboard.
//!
//! Pulls the four fitness sub-domains for the selected org into a
//! single sectioned page:
//!
//! - **Body** — tracked metrics (weight / body-fat / measurements),
//!   each a time series. Minimal create form (name + kind + unit).
//! - **Exercises** — the movement catalog. Minimal create form
//!   (name + category).
//! - **Workouts** — logged sessions (date + volume).
//! - **Intake** — daily food logs (date + calorie/macro totals).
//!
//! Editing, entry logging, routines, and nutrition resolution live
//! in the CLI for now; this is the read + light-create slice,
//! mirroring the `/locations` page. Body + Exercises state is the
//! shared optimistic store ([`crate::stores`]): one `AtomResult` per
//! section, typed `Id::Temp` rows for in-flight creates, rollback +
//! tray notification on failure.

use architect::Id;
use architect_ui::prelude::*;
use dioxus::prelude::*;
use uuid::Uuid;

use fitness_proto::body::BodyMetric;
use fitness_proto::exercises::Exercise;
use fitness_proto::intake::IntakeLog;
use fitness_proto::workouts::WorkoutSession;

use crate::orgs::{OrgMeta, OrgSelection};
use crate::stores;

const INPUT_CLS: &str = "rounded-lg border border-input bg-input/30 px-3 py-2 text-sm transition-colors \
     focus-visible:border-ring focus-visible:outline-none focus-visible:ring-[3px] \
     focus-visible:ring-ring/50 placeholder:text-muted-foreground";

/// Canonical body-metric kinds offered in the form's picker.
/// `kind` is free-form on the model; these cover the common cases.
const BODY_KINDS: &[&str] = &[
    "weight",
    "body-fat",
    "lean-mass",
    "waist",
    "hips",
    "chest",
    "neck",
    "other",
];

/// Canonical exercise categories offered in the form's picker.
const EXERCISE_CATEGORIES: &[&str] = &[
    "chest",
    "back",
    "legs",
    "glutes",
    "shoulders",
    "arms",
    "core",
    "cardio",
    "mobility",
    "other",
];

#[component]
pub fn FitnessView() -> Element {
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
            div { class: "flex flex-col gap-1",
                Heading { level: HeadingLevel::H1, "Fitness" }
                Text {
                    variant: TextVariant::Muted,
                    class: "text-sm",
                    "Body metrics, the exercise catalog, logged workouts, and daily intake.",
                }
            }

            BodySection { slug }
            ExercisesSection { slug }
            WorkoutsSection { slug }
            IntakeSection { slug }
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

// ── Body ────────────────────────────────────────────────────────────

#[component]
fn BodySection(slug: Memo<Option<String>>) -> Element {
    let mut name = use_signal(String::new);
    let kind = use_signal(|| "weight".to_string());
    let mut unit = use_signal(|| "kg".to_string());

    // The shared store: one AtomResult for the list, optimistic create.
    let result = stores::use_body_metric_list();
    let muts = stores::use_body_metric_mutations();
    let metric_store = stores::use_body_metric_store();

    let mut create = move || {
        let n = name.read().trim().to_string();
        if n.is_empty() {
            return;
        }
        let Some(s) = slug() else { return };
        let k = kind.read().clone();
        let u = unit.read().trim().to_string();
        name.set(String::new());
        muts.create(s, stores::draft_body_metric(n, k, u));
    };

    let rows: Vec<(Id<Uuid>, BodyMetric)> = result.value().cloned().unwrap_or_default();
    let load_err = result.error().cloned();
    let first_load = result.is_waiting() && result.value().is_none();

    rsx! {
        section { class: "flex flex-col gap-3",
            SectionHeader { title: "Body".to_string(), count: rows.len() }

            div { class: "flex flex-col gap-2 rounded-xl border border-border bg-card/40 p-3 sm:flex-row sm:items-center",
                input {
                    class: "{INPUT_CLS} flex-1",
                    placeholder: "Metric name (e.g. Weight)…",
                    value: "{name}",
                    oninput: move |e| name.set(e.value()),
                    onkeydown: move |e| {
                        if e.key() == Key::Enter {
                            create();
                        }
                    },
                }
                Select {
                    value: kind,
                    placeholder: "Kind".to_string(),
                    SelectContent {
                        for (i, k) in BODY_KINDS.iter().enumerate() {
                            SelectItem { key: "{k}", value: "{k}", index: i, "{k}" }
                        }
                    }
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

            if first_load {
                crate::states::LoadingState {}
            } else if rows.is_empty() {
                if let Some(err) = load_err {
                    crate::states::ErrorState {
                        title: "Couldn't load body metrics",
                        message: err,
                        on_retry: move |()| metric_store.reload(),
                    }
                } else {
                    crate::states::EmptyState {
                        title: "No body metrics yet",
                        hint: "Add your first metric above.",
                    }
                }
            } else {
                div { class: "flex flex-col gap-2",
                    for (id, m) in rows {
                        BodyRow { key: "{id}", pending: id.is_temp(), metric: m }
                    }
                }
            }
        }
    }
}

/// One body metric: name + kind badge + latest reading. `pending` dims
/// an optimistic row whose write-through is in flight; failures roll
/// back + notify.
#[component]
fn BodyRow(metric: BodyMetric, pending: bool) -> Element {
    let name = metric.name.clone();
    let kind = metric.kind.clone();
    let unit = metric.unit.clone();
    let latest = metric.latest().map(|e| (e.value, e.date));

    let state_cls = if pending {
        "border-border bg-card/40 opacity-60"
    } else {
        "border-border bg-card/40"
    };

    rsx! {
        div { class: "flex items-center gap-3 rounded-lg border px-3 py-2 {state_cls}",
            div { class: "flex min-w-0 flex-1 flex-col gap-0.5",
                Text { class: "break-words text-sm font-medium", "{name}" }
                if let Some((value, date)) = latest {
                    span { class: "text-[11px] text-muted-foreground", "{value} {unit} · {date}" }
                } else {
                    span { class: "text-[11px] text-muted-foreground", "no readings yet" }
                }
            }
            span { class: "shrink-0 rounded bg-muted px-1.5 py-px text-[11px] text-muted-foreground", "{kind}" }
        }
    }
}

// ── Exercises ───────────────────────────────────────────────────────

#[component]
fn ExercisesSection(slug: Memo<Option<String>>) -> Element {
    let mut name = use_signal(String::new);
    let category = use_signal(|| "chest".to_string());

    // The shared store: one AtomResult for the list, optimistic create.
    let result = stores::use_exercise_list();
    let muts = stores::use_exercise_mutations();
    let exercise_store = stores::use_exercise_store();

    let mut create = move || {
        let n = name.read().trim().to_string();
        if n.is_empty() {
            return;
        }
        let Some(s) = slug() else { return };
        let c = category.read().clone();
        name.set(String::new());
        muts.create(s, stores::draft_exercise(n, c));
    };

    let rows: Vec<(Id<Uuid>, Exercise)> = result.value().cloned().unwrap_or_default();
    let load_err = result.error().cloned();
    let first_load = result.is_waiting() && result.value().is_none();

    rsx! {
        section { class: "flex flex-col gap-3",
            SectionHeader { title: "Exercises".to_string(), count: rows.len() }

            div { class: "flex flex-col gap-2 rounded-xl border border-border bg-card/40 p-3 sm:flex-row sm:items-center",
                input {
                    class: "{INPUT_CLS} flex-1",
                    placeholder: "Exercise name (e.g. Bench Press)…",
                    value: "{name}",
                    oninput: move |e| name.set(e.value()),
                    onkeydown: move |e| {
                        if e.key() == Key::Enter {
                            create();
                        }
                    },
                }
                Select {
                    value: category,
                    placeholder: "Category".to_string(),
                    SelectContent {
                        for (i, c) in EXERCISE_CATEGORIES.iter().enumerate() {
                            SelectItem { key: "{c}", value: "{c}", index: i, "{c}" }
                        }
                    }
                }
                Button { variant: ButtonVariant::Primary, on_click: move |_| create(), "Add" }
            }

            if first_load {
                crate::states::LoadingState {}
            } else if rows.is_empty() {
                if let Some(err) = load_err {
                    crate::states::ErrorState {
                        title: "Couldn't load exercises",
                        message: err,
                        on_retry: move |()| exercise_store.reload(),
                    }
                } else {
                    crate::states::EmptyState {
                        title: "No exercises yet",
                        hint: "Add your first exercise above.",
                    }
                }
            } else {
                div { class: "flex flex-col gap-2",
                    for (id, ex) in rows {
                        ExerciseRow { key: "{id}", pending: id.is_temp(), exercise: ex }
                    }
                }
            }
        }
    }
}

/// One exercise: name + category badge + equipment summary. `pending`
/// dims an optimistic row whose write-through is in flight; failures
/// roll back + notify.
#[component]
fn ExerciseRow(exercise: Exercise, pending: bool) -> Element {
    let name = exercise.name.clone();
    let category = exercise.category.clone();
    let equipment = exercise.equipment.join(", ");

    let state_cls = if pending {
        "border-border bg-card/40 opacity-60"
    } else {
        "border-border bg-card/40"
    };

    rsx! {
        div { class: "flex items-center gap-3 rounded-lg border px-3 py-2 {state_cls}",
            div { class: "flex min-w-0 flex-1 flex-col gap-0.5",
                Text { class: "break-words text-sm font-medium", "{name}" }
                if !equipment.is_empty() {
                    span { class: "text-[11px] text-muted-foreground", "{equipment}" }
                }
            }
            span { class: "shrink-0 rounded bg-muted px-1.5 py-px text-[11px] text-muted-foreground", "{category}" }
        }
    }
}

// ── Workouts ────────────────────────────────────────────────────────

#[component]
fn WorkoutsSection(slug: Memo<Option<String>>) -> Element {
    let sessions = use_resource(move || async move {
        match slug() {
            Some(s) => crate::feeds::fetch_workouts(&s).await,
            None => Ok(Vec::new()),
        }
    });

    let (rows, load_err): (Vec<WorkoutSession>, Option<String>) = match &*sessions.read() {
        Some(Ok(all)) => (all.clone(), None),
        Some(Err(e)) => (Vec::new(), Some(e.clone())),
        None => (Vec::new(), None),
    };

    rsx! {
        section { class: "flex flex-col gap-3",
            SectionHeader { title: "Workouts".to_string(), count: rows.len() }

            if let Some(err) = load_err {
                LoadError { what: "workouts".to_string(), err }
            }

            if rows.is_empty() {
                EmptyState { message: "No logged workouts yet.".to_string() }
            } else {
                div { class: "flex flex-col gap-2",
                    for s in rows {
                        WorkoutRow { key: "{s.id}", session: s }
                    }
                }
            }
        }
    }
}

/// One logged session: name + date + status + set count / volume.
#[component]
fn WorkoutRow(session: WorkoutSession) -> Element {
    let name = session.name.clone();
    let date = session.date;
    let status = session.status.clone();
    let sets = session.logged_sets.len();
    let volume = session.total_volume();

    rsx! {
        div { class: "flex items-center gap-3 rounded-lg border border-border bg-card/40 px-3 py-2",
            div { class: "flex min-w-0 flex-1 flex-col gap-0.5",
                Text { class: "break-words text-sm font-medium", "{name}" }
                span { class: "text-[11px] text-muted-foreground", "{date} · {sets} sets · {volume} kg-reps" }
            }
            span { class: "shrink-0 rounded bg-muted px-1.5 py-px text-[11px] text-muted-foreground", "{status}" }
        }
    }
}

// ── Intake ──────────────────────────────────────────────────────────

#[component]
fn IntakeSection(slug: Memo<Option<String>>) -> Element {
    let logs = use_resource(move || async move {
        match slug() {
            Some(s) => crate::feeds::fetch_intake(&s).await,
            None => Ok(Vec::new()),
        }
    });

    let (rows, load_err): (Vec<IntakeLog>, Option<String>) = match &*logs.read() {
        Some(Ok(all)) => (all.clone(), None),
        Some(Err(e)) => (Vec::new(), Some(e.clone())),
        None => (Vec::new(), None),
    };

    rsx! {
        section { class: "flex flex-col gap-3",
            SectionHeader { title: "Intake".to_string(), count: rows.len() }

            if let Some(err) = load_err {
                LoadError { what: "intake logs".to_string(), err }
            }

            if rows.is_empty() {
                EmptyState { message: "No intake logs yet.".to_string() }
            } else {
                div { class: "flex flex-col gap-2",
                    for log in rows {
                        IntakeRow { key: "{log.id}", log }
                    }
                }
            }
        }
    }
}

/// One daily intake log: date + entry count + calorie total.
#[component]
fn IntakeRow(log: IntakeLog) -> Element {
    let name = log.name.clone();
    let date = log.date;
    let entries = log.entries.len();
    let calories = log.total().and_then(|n| n.calories);

    rsx! {
        div { class: "flex items-center gap-3 rounded-lg border border-border bg-card/40 px-3 py-2",
            div { class: "flex min-w-0 flex-1 flex-col gap-0.5",
                Text { class: "break-words text-sm font-medium", "{name}" }
                span { class: "text-[11px] text-muted-foreground", "{date} · {entries} entries" }
            }
            if let Some(kcal) = calories {
                span { class: "shrink-0 rounded bg-muted px-1.5 py-px text-[11px] text-muted-foreground", "{kcal} kcal" }
            }
        }
    }
}
