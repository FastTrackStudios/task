//! `/tasks/:id` — one issue, on [`task_ui::IssueView`].
//!
//! Reads the shared task / project stores (live, optimistic), so an
//! edit here shows everywhere at once and a failed write rolls back
//! with a notice. Every edit goes through
//! [`crate::task_board::apply_with_undo`], so completing or deleting
//! from here offers Undo like the list does.
//!
//! The top bar is Linear's: where you are (Tasks › short id), and on a
//! desk ↑ / ↓ (or k / j) step to the neighbouring issue of the list you
//! came from without going back to it.

use dioxus::prelude::*;
use dioxus_primitives::toast::use_toast;
use task_proto::TaskInfo;
use task_ui::{IssueView, TaskMutation};
use uuid::Uuid;

use crate::orgs::{OrgMeta, OrgSelection};
use crate::routes::Route;
use crate::stores;

#[component]
pub fn TaskDetailPage(id: Uuid) -> Element {
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();
    let account = use_context::<crate::auth::AuthCtx>().active;
    let nav = use_navigator();
    let toast = use_toast();

    let result = stores::use_task_list();
    let _projects = stores::use_project_list();
    let task_store = stores::use_task_store();
    let project_store = stores::use_project_store();
    let muts = stores::use_task_mutations();

    let target_org =
        use_memo(move || crate::orgs::create_target(&selection.read(), &org_list.read()));
    let member_names = crate::pages::wiki_access::use_member_names(target_org);

    let entries = task_store.entries();
    let unique = stores::unique_tasks(&entries);
    let Some(row) = unique.iter().find(|r| r.task.id == id).cloned() else {
        return if result.value().is_none() {
            rsx! { crate::states::LoadingState {} }
        } else {
            rsx! {
                div { class: "p-6 text-sm text-muted-foreground",
                    "This task isn't in the selected organizations."
                }
            }
        };
    };
    let slug = row.slug.clone();
    let task = row.task.clone();

    // The signed-in person first: assigning to yourself is the common
    // case, and it is what puts an issue on your Tasks list.
    let me = account
        .read()
        .as_ref()
        .map(|a| (a.user_id.to_string(), a.name.clone()));
    let mut people: Vec<(String, String)> = member_names
        .read()
        .iter()
        .map(|(id, name)| (id.clone(), name.clone()))
        .filter(|(pid, _)| me.as_ref().is_none_or(|(m, _)| m != pid))
        .collect();
    people.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
    if let Some(me) = me {
        people.insert(0, me);
    }

    let projects: Vec<(Uuid, String)> = project_store
        .list()
        .iter()
        .filter(|r| r.slug == slug && !r.project.archived)
        .map(|r| (r.project.id, r.project.title.clone()))
        .collect();
    let mut known_labels: Vec<String> = Vec::new();
    for r in &unique {
        for l in task_ui::labels_of(&r.task) {
            if !known_labels.iter().any(|k| k.eq_ignore_ascii_case(&l)) {
                known_labels.push(l);
            }
        }
    }
    let subtasks: Vec<TaskInfo> = unique
        .iter()
        .filter(|r| r.task.workflow.as_ref().and_then(|w| w.parent) == Some(id))
        .map(|r| r.task.clone())
        .collect();

    // The issue's place in the list it was opened from (open tasks, in
    // the store's order) — what ↑ / ↓ step through.
    let siblings: Vec<Uuid> = unique
        .iter()
        .filter(|r| task_proto::status_is_open(&r.task.status) || r.task.id == id)
        .map(|r| r.task.id)
        .collect();
    let pos = siblings.iter().position(|s| *s == id);
    let prev = pos
        .and_then(|p| p.checked_sub(1))
        .and_then(|p| siblings.get(p))
        .copied();
    let next = pos.and_then(|p| siblings.get(p + 1)).copied();

    let apply = {
        let slug = slug.clone();
        move |mu: TaskMutation| crate::task_board::apply_with_undo(muts, toast, &slug, mu)
    };
    let add_sub = {
        let slug = slug.clone();
        let parent = task.clone();
        move |title: String| {
            let mut sub = task_proto::capture(&title);
            sub.tags.0.push("subtask".into());
            sub.project_id = parent.project_id;
            sub.projects = parent.projects.clone();
            sub.workflow = Some(task_proto::model::WorkflowAttrs {
                parent: Some(parent.id),
                ..Default::default()
            });
            muts.apply(&slug, TaskMutation::Create { task: sub });
        }
    };
    let created = task
        .date_created
        .map(|d| d.with_timezone(&chrono::Local).format("%b %-d").to_string());
    let short = task_ui::views::short_id(&id);

    rsx! {
        div { class: "flex min-h-full flex-col",
            // ── top bar ──
            header { class: "flex h-11 shrink-0 items-center gap-2 border-b border-border/60 px-3 text-[13px] sm:px-5",
                Link {
                    to: Route::TasksRoute {},
                    class: "rounded-md px-1.5 py-1 font-medium text-muted-foreground hover:bg-accent hover:text-foreground",
                    "Tasks"
                }
                span { class: "text-muted-foreground/60", "›" }
                span { class: "truncate font-mono text-[12px] text-muted-foreground", "{short}" }
                crate::favorites::StarButton { kind: "issue", id: id.to_string(), title: task.title.clone() }
                div { class: "ml-auto flex items-center gap-0.5",
                    if let Some(p) = pos {
                        span { class: "hidden pr-1 font-mono text-[11px] tabular-nums text-muted-foreground sm:inline", "{p + 1} / {siblings.len()}" }
                    }
                    button {
                        r#type: "button",
                        class: "flex size-8 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground disabled:opacity-30",
                        title: "Previous issue (k)",
                        aria_label: "Previous issue",
                        disabled: prev.is_none(),
                        onclick: move |_| {
                            if let Some(p) = prev {
                                nav.push(Route::TaskDetailRoute { id: p });
                            }
                        },
                        architect_ui::lucide_dioxus::ChevronUp { size: 16 }
                    }
                    button {
                        r#type: "button",
                        class: "flex size-8 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground disabled:opacity-30",
                        title: "Next issue (j)",
                        aria_label: "Next issue",
                        disabled: next.is_none(),
                        onclick: move |_| {
                            if let Some(n) = next {
                                nav.push(Route::TaskDetailRoute { id: n });
                            }
                        },
                        architect_ui::lucide_dioxus::ChevronDown { size: 16 }
                    }
                }
            }
            IssueView {
                task,
                people,
                projects,
                known_labels,
                subtasks,
                activity: rsx! {
                    ol { class: "flex flex-col gap-2 border-l border-border/60 pl-4 text-[13px] text-muted-foreground",
                        if let Some(d) = created {
                            li { "Created · {d}" }
                        }
                        li { class: "text-muted-foreground/70", "Comments and changes will show here." }
                    }
                },
                on_event: apply,
                on_open: move |sid: Uuid| { nav.push(Route::TaskDetailRoute { id: sid }); },
                on_add_subtask: add_sub,
            }
        }
    }
}
