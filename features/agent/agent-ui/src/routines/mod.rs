//! Routines — the agent working while you aren't watching.
//!
//! The second tab of the right agent sidebar. A routine is a prompt
//! on a schedule ("every morning at 8, summarize what's due and drop
//! it in my inbox"); the backend runs it and delivers the output. The
//! panel is the whole lifecycle: schedule one, watch when it next
//! fires, run it early, pause it, drop it.
//!
//! Presentation lives here; the decidable parts —
//! [`schedule_hint`], [`relative_when`], [`runs_label`] — are pure
//! and tested in [`logic`].

use agent_proto::service::routines::{NewRoutine, Routine};
use architect_ui::lucide_dioxus::{CalendarClock, Pause, Play, Plus, Trash2};
use architect_ui::prelude::*;
use dioxus::prelude::*;

pub mod logic;
use logic::{relative_when, runs_label, schedule_hint};

/// Ready-made routines, offered in the composer so the common ones
/// are a click rather than an exercise in prompt-writing. A preset
/// only fills the draft fields — the user still reads it and presses
/// Schedule, and can edit anything first.
struct RoutinePreset {
    name: &'static str,
    /// What it's for, in the user's terms — one line under the chip.
    blurb: &'static str,
    schedule: &'static str,
    prompt: &'static str,
}

/// The triage prompt is the one from `apps/task/skills/task-triage.md`
/// — keep them in step. Daily before the workday: hourly burns tokens
/// on an empty queue, weekly lets the strip grow past the point
/// anyone reads it.
const PRESETS: &[RoutinePreset] = &[RoutinePreset {
    name: "Triage unfiled tasks",
    blurb: "Files tasks that belong to nothing, so they rejoin your list",
    schedule: "0 8 * * *",
    prompt: "Run the task-triage skill for this org. Call \
             list_untriaged_tasks, file what you can place confidently \
             with file_task, and leave the rest — an unfiled task is \
             recoverable, a wrongly filed one is invisible. Reply with \
             one line per task: filed (where + why) or skipped (why \
             not). If nothing is untriaged, reply \"nothing to triage\" \
             and stop.",
}];

#[component]
pub fn RoutinesPanel(slug: String) -> Element {
    // Clone up front: the org slug feeds the resource key *and*
    // every mutation closure below.
    let key = slug.clone();
    let mut routines = use_resource(use_reactive!(|(key,)| async move {
        crate::fetch_agent_routines(&key).await
    }));
    let mut error = use_signal(String::new);
    let mut composing = use_signal(|| false);
    let mut draft_name = use_signal(String::new);
    let mut draft_schedule = use_signal(String::new);
    let mut draft_prompt = use_signal(String::new);
    let mut busy_id = use_signal(String::new);

    let snapshot = routines.read().clone();
    let (rows, fetch_err): (Vec<Routine>, String) = match snapshot {
        Some(Ok(rows)) => (rows, String::new()),
        Some(Err(e)) => (Vec::new(), e),
        None => (Vec::new(), String::new()),
    };
    let loading = routines.read().is_none();

    // One mutation path for every row action — each returns the
    // updated routine, so the list just refetches.
    let act = use_callback({
        let slug = slug.clone();
        move |(id, action): (String, RowAction)| {
            let slug = slug.clone();
            busy_id.set(id.clone());
            spawn(async move {
                let res: Result<(), String> = match action {
                    RowAction::Pause(v) => crate::set_agent_routine_paused(&slug, &id, v)
                        .await
                        .map(|_| ()),
                    RowAction::RunNow => crate::run_agent_routine(&slug, &id).await.map(|_| ()),
                    RowAction::Delete => crate::delete_agent_routine(&slug, &id).await,
                };
                busy_id.set(String::new());
                match res {
                    Ok(()) => {
                        error.set(String::new());
                        routines.restart();
                    }
                    Err(e) => error.set(e),
                }
            });
        }
    });

    let create = use_callback({
        let slug = slug.clone();
        move |()| {
            let schedule = draft_schedule.peek().trim().to_string();
            let prompt = draft_prompt.peek().trim().to_string();
            if schedule.is_empty() || prompt.is_empty() {
                error.set("A routine needs both a schedule and a prompt.".to_string());
                return;
            }
            let slug = slug.clone();
            let name = draft_name.peek().trim().to_string();
            spawn(async move {
                let new = NewRoutine {
                    backend_id: String::new(),
                    name,
                    prompt,
                    schedule,
                    deliver: String::new(),
                    skills: Vec::new(),
                    repeat: 0,
                };
                match crate::create_agent_routine(&slug, new).await {
                    Ok(_) => {
                        error.set(String::new());
                        draft_name.set(String::new());
                        draft_schedule.set(String::new());
                        draft_prompt.set(String::new());
                        composing.set(false);
                        routines.restart();
                    }
                    Err(e) => error.set(e),
                }
            });
        }
    });

    rsx! {
        div { class: "flex items-center justify-between gap-2 px-3 pb-1 pt-3",
            div { class: "flex items-center gap-1.5 text-[11px] font-semibold uppercase tracking-[0.14em] text-muted-foreground",
                CalendarClock { size: 12 }
                span { "Routines" }
                if !rows.is_empty() {
                    span { class: "font-normal tabular-nums tracking-normal text-muted-foreground/60",
                        "{rows.len()}"
                    }
                }
            }
            button {
                r#type: "button",
                class: "flex h-9 w-9 items-center justify-center rounded text-muted-foreground hover:bg-accent/40 hover:text-foreground md:h-7 md:w-7",
                title: "Schedule a routine",
                onclick: move |_| {
                    let v = *composing.peek();
                    composing.set(!v);
                },
                Plus { size: 13 }
            }
        }

        div { class: "flex min-h-0 flex-1 flex-col gap-1.5 overflow-y-auto px-2 pb-3",
            if !error.read().is_empty() {
                div { class: "rounded-md border border-destructive/40 bg-destructive/10 px-2 py-1 text-xs",
                    "{error}"
                }
            }
            if !fetch_err.is_empty() {
                task_ui_core::states::InlineError {
                    message: fetch_err.clone(),
                    label: "Routines".to_string(),
                }
            }

            if composing() {
                div { class: "flex flex-col gap-2 rounded-lg border border-border/60 bg-card/40 p-2.5",
                    // Presets first — a blank prompt box is the reason
                    // most routines never get written.
                    div { class: "flex flex-col gap-1",
                        for p in PRESETS.iter() {
                            button {
                                key: "{p.name}",
                                r#type: "button",
                                class: "flex flex-col items-start gap-0.5 rounded-md border border-border/50 bg-card/30 px-2 py-1.5 text-left transition-colors hover:border-primary/50 hover:bg-accent/30",
                                onclick: move |_| {
                                    draft_name.set(p.name.to_string());
                                    draft_schedule.set(p.schedule.to_string());
                                    draft_prompt.set(p.prompt.to_string());
                                },
                                span { class: "text-xs font-medium text-foreground", "{p.name}" }
                                span { class: "text-[11px] leading-snug text-muted-foreground", "{p.blurb}" }
                            }
                        }
                    }
                    input {
                        class: "rounded-md border border-border/60 bg-card/30 px-2 py-2 text-base outline-none focus:border-primary/60 md:py-1 md:text-xs",
                        placeholder: "Name (optional)",
                        value: "{draft_name}",
                        oninput: move |e| draft_name.set(e.value()),
                    }
                    input {
                        class: "rounded-md border border-border/60 bg-card/30 px-2 py-2 font-mono text-base outline-none focus:border-primary/60 md:py-1 md:text-xs",
                        placeholder: "Schedule — every 2h · 0 8 * * * · 30m",
                        value: "{draft_schedule}",
                        oninput: move |e| draft_schedule.set(e.value()),
                    }
                    // The schedule grammar is the one thing people get
                    // wrong here, so echo the interpretation live.
                    if let Some(hint) = schedule_hint(&draft_schedule.read()) {
                        span { class: "px-0.5 text-[11px] text-muted-foreground", "{hint}" }
                    }
                    textarea {
                        class: "min-h-16 resize-y rounded-md border border-border/60 bg-card/30 px-2 py-2 text-base leading-relaxed outline-none focus:border-primary/60 md:py-1 md:text-xs",
                        placeholder: "What should the agent do each time? Write it as a standalone instruction — nobody's in the chair to clarify.",
                        value: "{draft_prompt}",
                        oninput: move |e| draft_prompt.set(e.value()),
                    }
                    div { class: "flex items-center justify-end gap-1.5",
                        Button {
                            variant: ButtonVariant::Ghost,
                            size: ButtonSize::Small,
                            on_click: move |_| composing.set(false),
                            "Cancel"
                        }
                        Button {
                            variant: ButtonVariant::Primary,
                            size: ButtonSize::Small,
                            on_click: move |_| create(()),
                            "Schedule"
                        }
                    }
                }
            }

            if loading {
                div { class: "px-2 py-3 text-xs text-muted-foreground", "Loading routines…" }
            } else if rows.is_empty() && fetch_err.is_empty() && !composing() {
                div { class: "flex flex-col gap-1 px-2 py-3",
                    Text { variant: TextVariant::Muted, class: "text-xs leading-relaxed",
                        "No routines yet. A routine is a prompt the agent runs on its own — a morning brief, a weekly review. Add one with +."
                    }
                }
            }

            for r in rows.iter() {
                {routine_row(r, busy_id.read().as_str() == r.id, act)}
            }
        }
    }
}

#[derive(Clone, PartialEq)]
enum RowAction {
    Pause(bool),
    RunNow,
    Delete,
}

fn routine_row(r: &Routine, busy: bool, act: Callback<(String, RowAction)>) -> Element {
    let paused = !r.enabled || r.state == "paused";
    let failed = !r.last_error.is_empty();
    let name = if r.name.trim().is_empty() {
        "(unnamed routine)".to_string()
    } else {
        r.name.clone()
    };
    let next = relative_when(&r.next_run_at, chrono::Utc::now());
    let last = relative_when(&r.last_run_at, chrono::Utc::now());
    let runs = runs_label(r.runs_completed, r.runs_total);
    let id = r.id.clone();

    let card = if paused {
        "flex flex-col gap-1.5 rounded-lg border border-border/60 bg-card/20 px-2.5 py-2 opacity-60 transition-opacity hover:opacity-90"
    } else if failed {
        "flex flex-col gap-1.5 rounded-lg border border-destructive/40 bg-destructive/5 px-2.5 py-2"
    } else {
        "flex flex-col gap-1.5 rounded-lg border border-border/60 bg-card/30 px-2.5 py-2 transition-colors hover:border-border"
    };

    rsx! {
        div { key: "{r.id}", class: "{card}",
            div { class: "flex items-baseline gap-1.5",
                span { class: "truncate text-sm font-medium text-foreground", title: "{r.prompt}", "{name}" }
                span { class: "ml-auto shrink-0 rounded bg-muted/50 px-1.5 font-mono text-[11px] text-muted-foreground",
                    "{r.schedule}"
                }
            }
            div { class: "flex flex-wrap items-center gap-x-2 gap-y-0.5 text-[11px] text-muted-foreground",
                if paused {
                    span { "paused" }
                } else if let Some(n) = &next {
                    span { title: "{r.next_run_at}", "next {n}" }
                }
                if let Some(l) = &last {
                    span { title: "{r.last_run_at}", "· ran {l}" }
                }
                if let Some(runs) = &runs {
                    span { "· {runs}" }
                }
                if !r.deliver.is_empty() && r.deliver != "local" {
                    span { "· → {r.deliver}" }
                }
            }
            if failed {
                p { class: "line-clamp-2 text-[11px] leading-snug text-destructive", "{r.last_error}" }
            }
            div { class: "-mx-1 flex items-center gap-0.5 border-t border-border/60 pt-1.5",
                button {
                    r#type: "button",
                    class: "flex h-9 w-9 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-accent/40 hover:text-foreground disabled:opacity-40 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary/50 md:h-7 md:w-7",
                    disabled: busy,
                    title: if paused { "Resume" } else { "Pause" },
                    onclick: {
                        let id = id.clone();
                        move |_| act((id.clone(), RowAction::Pause(!paused)))
                    },
                    if paused {
                        Play { size: 12 }
                    } else {
                        Pause { size: 12 }
                    }
                }
                button {
                    r#type: "button",
                    class: "flex h-9 items-center rounded px-2 text-[11px] text-muted-foreground transition-colors hover:bg-accent/40 hover:text-foreground disabled:opacity-40 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary/50 md:h-7",
                    disabled: busy,
                    title: "Run once now, without disturbing the schedule",
                    onclick: {
                        let id = id.clone();
                        move |_| act((id.clone(), RowAction::RunNow))
                    },
                    "Run now"
                }
                button {
                    r#type: "button",
                    class: "ml-auto flex h-9 w-9 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-destructive/10 hover:text-destructive disabled:opacity-40 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary/50 md:h-7 md:w-7",
                    disabled: busy,
                    title: "Delete routine",
                    onclick: {
                        let id = id.clone();
                        move |_| act((id.clone(), RowAction::Delete))
                    },
                    Trash2 { size: 12 }
                }
            }
        }
    }
}
