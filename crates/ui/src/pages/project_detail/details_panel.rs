//! The right panel — the working column behind the top bar's toggle:
//! the task board, live activity, and the details card.
//!
//! Deliberately NOT here: budget and billing. That is sensitive
//! reference material with its own audience, and it gets its own
//! surface when it comes back — not a card anyone screen-sharing a
//! project page broadcasts by accident.

use agent_proto::session::Session as AgentSession;
use architect_ui::prelude::*;
use dioxus::prelude::*;
use project_proto::ProjectInfo;
use task_proto::TaskInfo as DbTask;
use task_ui::{TaskInfo as UiTask, TaskMutation, TasksApp};
use timer_proto::WorkSession;
use uuid::Uuid;

use crate::stores;

use super::active::{ActiveNowSection, ActiveTaskRow};
use super::{ProjectKind, due_label};

/// The whole working column. Tasks live here — a task filed under the
/// project shows up on the org task list anyway, so the project page's
/// main column stays about what the project IS; the board is one
/// toggle away.
#[component]
pub(super) fn DetailsPanel(
    project: ProjectInfo,
    slug: String,
    snapshot: Option<(Vec<WorkSession>, Vec<AgentSession>)>,
    you: Option<Uuid>,
    tasks: Vec<UiTask>,
    active_tasks: Vec<DbTask>,
    on_task: EventHandler<TaskMutation>,
) -> Element {
    let p = project;
    let project_muts = stores::use_project_mutations();
    let kind = ProjectKind::from_str(&p.project_type);
    // The cover-image editor hides behind its row: a raw URL input at
    // the bottom of a reference card reads as a debug panel.
    let mut cover_edit = use_signal(|| false);

    // Panel sections are labelled in small caps, not page headings —
    // three same-weight H3s in a 384px column flattened the whole
    // hierarchy. The About card leads: who it's for and where it
    // stands is reference material you glance at; the board below is
    // what you work.
    rsx! {
        div { class: "hidden w-96 shrink-0 flex-col gap-6 lg:flex",
            // ── About ───────────────────────────────────────────────
            div { class: "flex flex-col gap-2.5 rounded-xl border border-border bg-card/40 p-4",
                span { class: "text-[11px] font-semibold uppercase tracking-wider text-muted-foreground",
                    "About"
                }
                if !p.details.trim().is_empty() {
                    Text { class: "whitespace-pre-line text-sm leading-relaxed",
                        "{p.details}"
                    }
                }
                DetailRow { label: "Status".to_string(), value: p.status.clone() }
                DetailRow { label: "Priority".to_string(), value: p.priority.clone() }
                if !p.lead.is_empty() {
                    DetailRow { label: "Lead".to_string(), value: p.lead.clone() }
                }
                DetailRow { label: "Due".to_string(), value: due_label(p.target_date) }
                // Editable project type / template.
                div { class: "flex items-center justify-between gap-3 text-sm",
                    span { class: "shrink-0 text-muted-foreground", "Type" }
                    div { class: "flex gap-1",
                        for k in ProjectKind::ALL {
                            {
                                let np_base = p.clone();
                                let type_slug = slug.clone();
                                let is_current = k == kind;
                                rsx! {
                                    Button {
                                        key: "{k.slug()}",
                                        variant: if is_current { ButtonVariant::Secondary } else { ButtonVariant::Ghost },
                                        size: ButtonSize::Small,
                                        on_click: move |_| {
                                            // Optimistic: the badge flips instantly;
                                            // a failed write rolls back + notifies.
                                            let mut np = np_base.clone();
                                            np.project_type = k.slug().to_string();
                                            project_muts.update(type_slug.clone(), np);
                                        },
                                        "{k.label()}"
                                    }
                                }
                            }
                        }
                    }
                }
                // Cover image behind its affordance; provenance as one
                // quiet footer line, not two ledger rows.
                div { class: "flex items-center justify-between text-sm",
                    span { class: "text-muted-foreground", "Cover image" }
                    Button {
                        variant: ButtonVariant::Ghost,
                        size: ButtonSize::Small,
                        on_click: move |_| {
                            let open = *cover_edit.peek();
                            cover_edit.set(!open);
                        },
                        if cover_edit() { "Done" } else if p.image.trim().is_empty() { "Set…" } else { "Edit…" }
                    }
                }
                if cover_edit() {
                    CoverImageEditor { project: p.clone(), slug: slug.clone() }
                }
                if p.date_created.is_some() || p.date_modified.is_some() {
                    span { class: "pt-1 text-[11px] text-muted-foreground/70",
                        if let Some(c) = p.date_created {
                            "Created {c.date_naive()}"
                        }
                        if p.date_created.is_some() && p.date_modified.is_some() {
                            " · "
                        }
                        if let Some(m) = p.date_modified {
                            "Updated {m.date_naive()}"
                        }
                    }
                }
            }

            // ── The board ───────────────────────────────────────────
            div { class: "flex flex-col gap-2",
                span { class: "text-[11px] font-semibold uppercase tracking-wider text-muted-foreground",
                    "Tasks"
                }
                if !active_tasks.is_empty() {
                    div { class: "flex flex-col gap-1 rounded-xl border border-border bg-card/40 p-3",
                        span { class: "text-[11px] uppercase tracking-wide text-muted-foreground",
                            "Active"
                        }
                        for t in active_tasks.iter() {
                            ActiveTaskRow { key: "{t.id}", task: t.clone() }
                        }
                    }
                }
                TasksApp {
                    embedded: true,
                    tasks,
                    on_event: move |mu: TaskMutation| on_task.call(mu),
                }
            }

            ActiveNowSection { snapshot, you }
        }
    }
}

/// Sidebar editor for the project's cover image — writes through the
/// `image:` frontmatter via the optimistic project store, the same
/// idiom as the Type buttons above it (instant preview, rollback +
/// tray notification on failure).
///
/// Smallest honest slice: paste an image URL (or any path the browser
/// can resolve) and save; the /projects cards and the detail banner
/// render it. Uploading bytes through the `AttachmentService` is
/// deliberately *not* wired yet: its download URLs are short-lived
/// signed links, so persisting one into `image:` would go stale — that
/// needs a stable blob URL (or render-time hash → URL resolution)
/// first. The gap is documented on the issue.
#[component]
fn CoverImageEditor(project: ProjectInfo, slug: String) -> Element {
    let muts = stores::use_project_mutations();
    let mut draft = use_signal(|| project.image.clone());
    // Re-sync the draft when a reconciled write lands a new stored value.
    let stored = project.image.clone();
    use_effect(use_reactive!(|(stored,)| draft.set(stored)));

    let preview = draft.read().trim().to_string();
    let dirty = preview != project.image.trim();
    let busy = muts.is_pending();
    let error = muts.error();

    rsx! {
        div { class: "flex flex-col gap-1.5 text-sm",
            span { class: "text-muted-foreground", "Cover image" }
            if !preview.is_empty() {
                div { class: "aspect-video w-full overflow-hidden rounded-md border border-border bg-muted",
                    img {
                        src: "{preview}",
                        alt: "Cover preview",
                        loading: "lazy",
                        class: "h-full w-full object-cover",
                    }
                }
            }
            div { class: "flex items-center gap-2",
                Input {
                    value: draft,
                    placeholder: "https://… image URL",
                    on_change: move |_| {},
                }
                if dirty {
                    Button {
                        variant: ButtonVariant::Secondary,
                        size: ButtonSize::Small,
                        disabled: busy,
                        on_click: {
                            let np_base = project.clone();
                            let save_slug = slug.clone();
                            move |_| {
                                let mut np = np_base.clone();
                                np.image = draft.peek().trim().to_string();
                                muts.update(save_slug.clone(), np);
                            }
                        },
                        if busy { "Saving…" } else { "Save" }
                    }
                }
            }
            if let Some(e) = error.as_ref() {
                span { class: "text-xs text-destructive", "Couldn't save the cover: {e}" }
            }
        }
    }
}

/// A label/value row in the sidebar Details card.
#[component]
fn DetailRow(label: String, value: String) -> Element {
    rsx! {
        div { class: "flex items-baseline justify-between gap-3 text-sm",
            span { class: "shrink-0 text-muted-foreground", "{label}" }
            span { class: "min-w-0 break-words text-right font-medium", "{value}" }
        }
    }
}
