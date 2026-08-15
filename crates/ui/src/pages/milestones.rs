//! `/milestones` — project checkpoints.
//!
//! Milestones are GitHub-Projects-style checkpoints: each is
//! bound to a project and tasks roll up into it. They live as
//! markdown pages in the vault (`type: milestone`) under the
//! owning project's folder and carry a stable `id`.
//!
//! This page lists the selected org's milestones (title, due
//! date, project, status) and offers a friction-light "Add
//! milestone" form (title + project + optional due date). State is
//! the shared optimistic store ([`crate::stores`]): one `AtomResult`
//! list, typed `Id::Temp` rows for in-flight creates, rollback +
//! tray notification on failure.

use architect::Id;
use architect_ui::prelude::*;
use dioxus::prelude::*;
use milestone_proto::Milestone;
use uuid::Uuid;

use crate::orgs::{OrgMeta, OrgSelection};
use crate::stores;

const INPUT_CLS: &str = "rounded-lg border border-input bg-input/30 px-3 py-2 text-sm transition-colors \
     focus-visible:border-ring focus-visible:outline-none focus-visible:ring-[3px] \
     focus-visible:ring-ring/50 placeholder:text-muted-foreground";

#[component]
pub fn MilestonesView() -> Element {
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();

    // The org we create into (first selected, or home).
    let slug = use_memo(move || {
        crate::orgs::selected_slugs(&selection.read(), &org_list.read())
            .into_iter()
            .next()
    });

    let mut title = use_signal(String::new);
    let project = use_signal(String::new);
    let mut due = use_signal(String::new);

    // The shared store: one AtomResult for the list, optimistic create.
    let result = stores::use_milestone_list();
    let muts = stores::use_milestone_mutations();

    // The org's projects power the picker (project_id is required).
    let projects = use_resource(move || async move {
        match slug() {
            Some(s) => crate::feeds::fetch_projects(&[s]).await.unwrap_or_default(),
            None => Vec::new(),
        }
    });
    let project_options = projects.read().clone().unwrap_or_default();

    let mut create = move || {
        let t = title.read().trim().to_string();
        if t.is_empty() {
            return;
        }
        let Some(s) = slug() else { return };
        let Ok(project_id) = uuid::Uuid::parse_str(project.read().trim()) else {
            return;
        };
        let due_date = {
            let d = due.read().trim().to_string();
            if d.is_empty() {
                None
            } else {
                d.parse::<chrono::NaiveDate>().ok()
            }
        };
        title.set(String::new());
        due.set(String::new());
        muts.create(s, stores::draft_milestone(t, project_id, due_date));
    };

    let store = stores::use_milestone_store();
    let rows: Vec<(Id<Uuid>, Milestone)> = result.value().cloned().unwrap_or_default();
    let load_err = result.error().cloned();
    let first_load = result.is_waiting() && result.value().is_none();

    // Project id → title, for resolving each milestone's owner.
    let project_name = {
        let opts = project_options.clone();
        move |id: uuid::Uuid| -> Option<String> {
            opts.iter().find(|p| p.id == id).map(|p| p.title.clone())
        }
    };

    rsx! {
        div { class: "mx-auto flex max-w-3xl flex-col gap-5 p-4 sm:p-6 lg:p-10",
            div { class: "flex items-center justify-between gap-3",
                Heading { level: HeadingLevel::H1, "Milestones" }
                Text { variant: TextVariant::Muted, class: "text-sm", "{rows.len()} checkpoints" }
            }
            Text {
                variant: TextVariant::Muted,
                class: "text-sm -mt-2",
                "Project checkpoints — tasks roll up into these, GitHub-Projects style.",
            }

            // ── Add milestone ──────────────────────────────────────
            div { class: "flex flex-col gap-2 rounded-xl border border-border bg-card/40 p-3 sm:flex-row sm:items-center",
                input {
                    class: "{INPUT_CLS} flex-1",
                    placeholder: "Milestone title…",
                    value: "{title}",
                    oninput: move |e| title.set(e.value()),
                    onkeydown: move |e| {
                        if e.key() == Key::Enter {
                            create();
                        }
                    },
                }
                Select {
                    value: project,
                    placeholder: "Project…".to_string(),
                    SelectContent {
                        for (i, p) in project_options.iter().enumerate() {
                            SelectItem { key: "{p.id}", value: "{p.id}", index: i, "{p.title}" }
                        }
                    }
                }
                input {
                    class: "{INPUT_CLS}",
                    r#type: "date",
                    value: "{due}",
                    oninput: move |e| due.set(e.value()),
                }
                Button {
                    variant: ButtonVariant::Primary,
                    on_click: move |_| create(),
                    "Add"
                }
            }

            // ── The register ───────────────────────────────────────
            if first_load {
                crate::states::LoadingState {}
            } else if rows.is_empty() {
                if let Some(err) = load_err {
                    crate::states::ErrorState {
                        title: "Couldn't load milestones",
                        message: err,
                        on_retry: move |()| store.reload(),
                    }
                } else {
                    crate::states::EmptyState {
                        title: "No milestones yet",
                        hint: "Add your first checkpoint above.",
                    }
                }
            } else {
                div { class: "flex flex-col gap-2",
                    for (id, ms) in rows {
                        MilestoneRow {
                            key: "{id}",
                            project_label: project_name(ms.project_id),
                            pending: id.is_temp(),
                            ms,
                        }
                    }
                }
            }
        }
    }
}

/// One milestone in the register: title + owning project + due
/// date + status badge. `pending` dims an optimistic row whose
/// write-through is in flight; failures roll back + notify.
#[component]
fn MilestoneRow(ms: Milestone, project_label: Option<String>, pending: bool) -> Element {
    let title = ms.title.clone();
    let due = ms.due_date.map(|d| d.format("%b %-d, %Y").to_string());
    let closed = ms.status.eq_ignore_ascii_case("closed");
    let status_label = if closed { "closed" } else { "open" };

    let state_cls = if pending {
        "border-border bg-card/40 opacity-60"
    } else {
        "border-border bg-card/40"
    };

    rsx! {
        div { class: "flex items-start gap-3 rounded-lg border px-3 py-2 {state_cls}",
            div { class: "flex min-w-0 flex-1 flex-col gap-1",
                Text { class: "break-words text-sm font-medium", "{title}" }
                div { class: "flex flex-wrap items-center gap-2 text-[11px] text-muted-foreground",
                    if let Some(name) = project_label.as_ref() {
                        span { "{name}" }
                    }
                    if let Some(d) = due.as_ref() {
                        span { "due {d}" }
                    }
                }
            }
            div { class: "flex shrink-0 items-center gap-2",
                span { class: "rounded bg-muted px-1.5 py-px text-[11px] text-muted-foreground", "{status_label}" }
            }
        }
    }
}
